//! Opinionated chaff and flare presentation: burning flares with their glare
//! and light, and shimmering chaff clouds. Simulation owns motion and life;
//! rules and constants are in docs/spec/countermeasures.md.
use crate::gpu_setup::NamedSteps as _;
use tore_sim::combat::countermeasures::{Devices, MAX_CHAFF, MAX_FLARES};

/// Flares that light the scene at once, nearest and brightest first.
pub const MAX_FLARE_LIGHTS: usize = 16;
/// A full-intensity flare lights a surface facing it as brightly as full
/// sun at about 63 feet (strength / distance², in feet²).
pub const FLARE_STRENGTH: f64 = 4000.;
// Mirrors of FLARE_RANGE, FLARE_SOFTENING and FLARE_COLOR in
// surface_lighting.wgsl. A light stronger than a flare reaches farther, by
// the square root of its strength in flares (nr-fx3), FLARE_UNIT there being
// FLARE_STRENGTH.
const FLARE_RANGE: f64 = 1500.;
const FLARE_SOFTENING: f64 = 25.;
const FLARE_COLOR: [f64; 3] = [1.0, 0.75, 0.45];
/// An afterburning aircraft lights the scene at an eighth of a flare's
/// strength, split across its engines: John asked for a quarter on
/// 2026-09-26, then halved it the same day.
pub const AFTERBURNER_SHARE: f64 = 0.125;
/// Each engine's light sits in its flame, this far behind the outlet.
pub const AFTERBURNER_BEHIND_FEET: f64 = 3.;
/// A flak burst's light, in the flare light's units (opinionated, agent,
/// 2026-10-10, from John's request that flak explosions flash light at
/// night and dusk): the 85 mm shell's is a flare and a half at its peak, the
/// 100 mm shell's twice a flare. It holds for two ticks, falls away over
/// [`FLAK_LIGHT_TICKS`] and, like every flare light, counts up to four times
/// as much against the night scene.
pub const FLAK_LIGHT_SMALL: f64 = 2.5 * FLARE_STRENGTH;
pub const FLAK_LIGHT_LARGE: f64 = 3.5 * FLARE_STRENGTH;
pub const FLAK_LIGHT_TICKS: f64 = 14.;
const FLAK_LIGHT_HOLD: f64 = 2.;

/// The strength of the light of a flak burst of explosion `blast` (27 the
/// 85 mm shell, 28 the 100 mm) `age` ticks after it went off; none before it
/// or after [`FLAK_LIGHT_TICKS`].
pub fn flak_light(blast: u8, age: f64) -> f64 {
    if !(0. ..FLAK_LIGHT_TICKS).contains(&age) {
        return 0.;
    }
    let peak = if blast == 27 {
        FLAK_LIGHT_SMALL
    } else {
        FLAK_LIGHT_LARGE
    };
    let left = 1. - (age - FLAK_LIGHT_HOLD).max(0.) / (FLAK_LIGHT_TICKS - FLAK_LIGHT_HOLD);
    peak * left * left
}
/// Foil strips drawn for each chaff cartridge.
pub const CHAFF_STRIPS: u32 = 600;
const FLARE_BYTES: usize = 9 * 4;
const CHAFF_BYTES: usize = 6 * 4;

/// One lit afterburner flame as a light: its position in world feet and its
/// share of a flare's strength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Afterburner {
    pub position: [f64; 3],
    pub share: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlareLight {
    /// Persistent fire, reserved ahead of transient flashes at night.
    pub sustained: bool,
    /// Source extent in feet. Falloff stays the flare point law. A positive
    /// radius is a fire, fireball or impact flash and reflects mostly surface
    /// brightness; zero is a point source and keeps the art colour.
    pub radius: f64,
    pub position: [f64; 3],
    pub strength: f64,
}

/// Every burning flare and lit afterburner as a light source.
pub fn lights(devices: &Devices, afterburners: &[Afterburner]) -> Vec<FlareLight> {
    devices
        .flares
        .iter()
        .filter(|f| f.intensity() > 0.)
        .map(|f| FlareLight {
            sustained: false,
            radius: 0.,
            position: f.position,
            strength: FLARE_STRENGTH * f.intensity(),
        })
        .chain(afterburners.iter().map(|a| FlareLight {
            sustained: false,
            radius: 0.,
            position: a.position,
            strength: FLARE_STRENGTH * a.share,
        }))
        .collect()
}

/// The flares that matter most at `origin`, relative to it, for the shared
/// lighting uniform. Subtracting in f64 keeps precision far from the origin.
pub fn nearest(lights: &[FlareLight], origin: [f64; 3]) -> Vec<[f32; 8]> {
    let mut ranked: Vec<_> = lights
        .iter()
        .map(|light| {
            let offset: [f64; 3] = std::array::from_fn(|i| light.position[i] - origin[i]);
            let d2 = offset.iter().map(|v| v * v).sum::<f64>();
            (
                light.strength / d2.max(1.),
                offset,
                light.strength,
                light.radius,
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
        .into_iter()
        .take(MAX_FLARE_LIGHTS)
        .map(|(_, offset, strength, radius)| {
            [
                offset[0] as f32,
                offset[1] as f32,
                offset[2] as f32,
                strength as f32,
                radius as f32,
                0.,
                0.,
                0.,
            ]
        })
        .collect()
}

/// Brightest flare light a smoke puff takes, so the puffs at a flare's head
/// glow without washing out to a flat white sheet.
const GLOW_LIMIT: f64 = 1.;
/// Flare light reaching a smoke puff from every side, as `flare_light` with
/// a zero normal computes it on the GPU, limited to `GLOW_LIMIT`.
pub fn glow(lights: &[FlareLight], point: [f64; 3]) -> [f32; 3] {
    let total: f64 = lights
        .iter()
        .map(|light| {
            let d2: f64 = (0..3).map(|i| (light.position[i] - point[i]).powi(2)).sum();
            let range2 = FLARE_RANGE.powi(2) * (light.strength / FLARE_STRENGTH).max(1.);
            let edge = (1. - d2 * d2 / range2.powi(2)).clamp(0., 1.);
            light.strength * edge * edge / (d2 + FLARE_SOFTENING)
        })
        .sum();
    FLARE_COLOR.map(|c| (c * total.min(GLOW_LIMIT)) as f32)
}

pub struct Pipelines {
    flare: wgpu::RenderPipeline,
    chaff: wgpu::RenderPipeline,
    glare: wgpu::RenderPipeline,
    /// AC-130 muzzle flashes (`gun_flash.rs`).
    flash: wgpu::RenderPipeline,
}
impl Pipelines {
    /// `surface` holds the world material and shared lighting groups; `glare`
    /// adds the world depth, as the spotting aid reads it.
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        surface: &wgpu::PipelineLayout,
        glare: &wgpu::PipelineLayout,
    ) -> Self {
        let additive = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let flare_buffers = [wgpu::VertexBufferLayout {
            array_stride: FLARE_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32,2=>Float32x3,3=>Float32,4=>Uint32],
        }];
        let flash_buffers = [wgpu::VertexBufferLayout {
            array_stride: crate::gun_flash::FLASH_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32,2=>Float32x3,3=>Float32,4=>Uint32,5=>Uint32],
        }];
        let chaff_buffers = [wgpu::VertexBufferLayout {
            array_stride: CHAFF_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32,2=>Uint32,3=>Float32],
        }];
        // Tested against the world depth without writing it, so devices sit
        // behind terrain and aircraft without hiding one another.
        let depth = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Greater,
            stencil: Default::default(),
            bias: Default::default(),
        });
        let pipeline = |label,
                        layout,
                        vertex,
                        fragment,
                        buffers: &[wgpu::VertexBufferLayout],
                        blend,
                        depth: Option<wgpu::DepthStencilState>,
                        count| {
            device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };
        Self {
            flare: pipeline(
                "Burning flares",
                surface,
                "flare_vertex",
                "flare_fragment",
                &flare_buffers,
                additive,
                depth.clone(),
                samples,
            ),
            flash: pipeline(
                "AC-130 muzzle flashes",
                surface,
                "flash_vertex",
                "flash_fragment",
                &flash_buffers,
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                depth.clone(),
                samples,
            ),
            chaff: pipeline(
                "Chaff strips",
                surface,
                "chaff_vertex",
                "chaff_fragment",
                &chaff_buffers,
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                depth,
                samples,
            ),
            // Over the finished image at output resolution, like the spotting
            // aid, so glare can spill over the aircraft that released it.
            glare: pipeline(
                "Flare glare",
                glare,
                if samples > 1 {
                    "glare_vertex_ms"
                } else {
                    "glare_vertex"
                },
                "glare_fragment",
                &flare_buffers,
                additive,
                None,
                1,
            ),
        }
    }
}

/// This frame's devices and muzzle flashes, uploaded once and drawn in every
/// world view, and the lights of flares, afterburners and gunfire.
pub struct Instances {
    flares: wgpu::Buffer,
    chaff: wgpu::Buffer,
    flashes: wgpu::Buffer,
    flare_count: u32,
    chaff_count: u32,
    flash_count: u32,
    pub lights: Vec<FlareLight>,
}
impl Instances {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            flares: buffer("Burning flare instances", MAX_FLARES * FLARE_BYTES),
            chaff: buffer("Chaff cloud instances", MAX_CHAFF * CHAFF_BYTES),
            flashes: buffer(
                "Muzzle flash instances",
                crate::gun_flash::MAX_FLASHES * crate::gun_flash::FLASH_BYTES,
            ),
            flare_count: 0,
            chaff_count: 0,
            flash_count: 0,
            lights: Vec::new(),
        }
    }
    pub fn upload(
        &mut self,
        queue: &wgpu::Queue,
        devices: &Devices,
        afterburners: &[Afterburner],
        guns: &crate::gun_flash::Drawn,
    ) {
        self.lights = lights(devices, afterburners);
        self.lights.extend(guns.lights.iter().copied());
        let flashes = crate::gun_flash::instances(&guns.flashes);
        self.flash_count = (flashes.len() / crate::gun_flash::FLASH_BYTES) as u32;
        if !flashes.is_empty() {
            queue.write_buffer(&self.flashes, 0, &flashes);
        }
        let flares = flare_instances(devices);
        self.flare_count = (flares.len() / FLARE_BYTES) as u32;
        if !flares.is_empty() {
            queue.write_buffer(&self.flares, 0, &flares);
        }
        let chaff = chaff_instances(devices);
        self.chaff_count = (chaff.len() / CHAFF_BYTES) as u32;
        if !chaff.is_empty() {
            queue.write_buffer(&self.chaff, 0, &chaff);
        }
    }
    pub fn has_flares(&self) -> bool {
        self.flare_count > 0
    }
    /// Chaff strips, flare bodies, then muzzle flashes, inside the world pass. The caller has
    /// bound the world material (group 0) and shared lighting (group 1).
    pub fn draw_world(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        if self.chaff_count > 0 {
            pass.set_pipeline(&pipelines.chaff);
            pass.set_vertex_buffer(0, self.chaff.slice(..));
            pass.draw(0..CHAFF_STRIPS * 6, 0..self.chaff_count);
        }
        if self.flare_count > 0 {
            pass.set_pipeline(&pipelines.flare);
            pass.set_vertex_buffer(0, self.flares.slice(..));
            pass.draw(0..6, 0..self.flare_count);
        }
        if self.flash_count > 0 {
            pass.set_pipeline(&pipelines.flash);
            pass.set_vertex_buffer(0, self.flashes.slice(..));
            pass.draw(0..6, 0..self.flash_count);
        }
    }
    /// Glare over the resolved image. The caller has bound groups 0 and 1 and
    /// the world depth as group 2.
    pub fn draw_glare(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        if self.flare_count > 0 {
            pass.set_pipeline(&pipelines.glare);
            pass.set_vertex_buffer(0, self.flares.slice(..));
            pass.draw(0..6, 0..self.flare_count);
        }
    }
}

fn flare_instances(devices: &Devices) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(devices.flares.len() * FLARE_BYTES);
    for flare in devices.flares.iter().filter(|f| f.intensity() > 0.) {
        for value in flare
            .position
            .into_iter()
            .chain([flare.intensity()])
            .chain(flare.motion)
            .chain([f64::from(flare.age) / 120.])
        {
            bytes.extend((value as f32).to_le_bytes());
        }
        bytes.extend(flare.seed().to_le_bytes());
    }
    bytes
}

fn chaff_instances(devices: &Devices) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(devices.chaff.len() * CHAFF_BYTES);
    for chaff in &devices.chaff {
        for value in chaff.position.into_iter().chain([chaff.seconds()]) {
            bytes.extend((value as f32).to_le_bytes());
        }
        bytes.extend(chaff.seed.to_le_bytes());
        bytes.extend(chaff.opacity().to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_sim::attitude::Basis;
    use tore_sim::combat::countermeasures::Release;

    #[test]
    fn a_flak_burst_flashes_hard_then_falls_away_in_a_few_ticks() {
        // The 100 mm shell's flash is stronger than the 85 mm's, both at
        // least a flare's.
        assert!(flak_light(28, 0.) > flak_light(27, 0.) && flak_light(27, 0.) >= FLARE_STRENGTH);
        for (kind, peak) in [(27, FLAK_LIGHT_SMALL), (28, FLAK_LIGHT_LARGE)] {
            assert_eq!(flak_light(kind, 0.), peak);
            assert_eq!(flak_light(kind, 2.), peak, "held for two ticks");
            let mut last = peak;
            for tick in 3..14 {
                let now = flak_light(kind, f64::from(tick));
                assert!(now < last && now > 0., "falls at tick {tick}");
                last = now;
            }
            assert_eq!(flak_light(kind, FLAK_LIGHT_TICKS), 0.);
            assert_eq!(flak_light(kind, -1.), 0.);
        }
        // Lit by the flare law: a burst a hundred feet away lights a facing
        // surface as a flare at 40 feet does, four times as much at night.
        let lights = [FlareLight {
            sustained: false,
            radius: 0.,
            position: [0., 0., 100.],
            strength: flak_light(27, 0.),
        }];
        let near = nearest(&lights, [0.; 3]);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0][3] as f64, FLAK_LIGHT_SMALL);
    }

    fn released(flares: usize, chaff: usize) -> Devices {
        let mut devices = Devices::default();
        for n in 0..flares {
            devices.release_flare(Release {
                position: [n as f64 * 100., 5000., 0.],
                velocity: [0., 0., 600.],
                basis: Basis::new(0., 0., 0.),
            });
        }
        for _ in 0..chaff {
            devices.release_chaff(Release {
                position: [0., 5000., 0.],
                velocity: [0., 0., 600.],
                basis: Basis::new(0., 0., 0.),
            });
        }
        for _ in 0..30 {
            devices.step(&|_, _| 0.);
        }
        devices
    }

    #[test]
    fn the_nearest_sixteen_flares_light_the_view_relative_to_it() {
        assert_eq!(lights(&released(12, 0), &[]).len(), 24);
        let origin = [1_000_000.5, 5000., 0.];
        let mut all: Vec<_> = (0..20)
            .map(|n| FlareLight {
                sustained: false,
                radius: 0.,
                position: [origin[0] + 10. + f64::from(n) * 100., 5000., 0.],
                strength: FLARE_STRENGTH,
            })
            .collect();
        // A brighter flare farther away can outrank a dim one nearby.
        all[19].strength *= 1000.;
        let chosen = nearest(&all, origin);
        assert_eq!(chosen.len(), MAX_FLARE_LIGHTS);
        assert_eq!(
            chosen[0],
            [10., 0., 0., FLARE_STRENGTH as f32, 0., 0., 0., 0.]
        );
        assert_eq!(chosen[1][0], 1910.);
        let offsets: Vec<f32> = chosen[2..].iter().map(|c| c[0]).collect();
        assert_eq!(
            offsets,
            (1..15).map(|n| 10. + n as f32 * 100.).collect::<Vec<_>>()
        );
    }

    #[test]
    fn flare_glow_is_inverse_square_and_ends_at_fifteen_hundred_feet() {
        let light = [FlareLight {
            sustained: false,
            radius: 0.,
            position: [0.; 3],
            strength: FLARE_STRENGTH,
        }];
        let at = |d: f64| glow(&light, [d, 0., 0.])[0];
        assert_eq!(at(50.), GLOW_LIMIT as f32);
        assert!((at(80.) - 0.62).abs() < 0.01);
        assert!((at(100.) - 0.4).abs() < 0.01);
        assert!((at(200.) - 0.1).abs() < 0.005);
        assert!(at(1000.) > 0.);
        assert_eq!(at(1500.), 0.);
        assert_eq!(glow(&light, [0., 0., 0.])[2], (0.45 * GLOW_LIMIT) as f32);
    }

    #[test]
    fn afterburners_light_the_scene_without_glare() {
        let devices = released(1, 0);
        let flame = Afterburner {
            position: [5., 6., 7.],
            share: AFTERBURNER_SHARE / 2.,
        };
        let all = lights(&devices, &[flame, flame]);
        assert_eq!(all.len(), 4);
        // Two engines together light at an eighth of a flare.
        assert_eq!(
            all[2..].iter().map(|l| l.strength).sum::<f64>(),
            FLARE_STRENGTH / 8.
        );
        assert_eq!(all[2].position, [5., 6., 7.]);
        // Only flares are drawn as bodies and glare.
        assert_eq!(flare_instances(&devices).len(), 2 * FLARE_BYTES);
    }

    #[test]
    fn instances_pack_every_burning_flare_and_cloud() {
        let devices = released(3, 2);
        let flares = flare_instances(&devices);
        assert_eq!(flares.len(), 6 * FLARE_BYTES);
        let x = f32::from_le_bytes(flares[..4].try_into().unwrap());
        assert_eq!(x, devices.flares[0].position[0] as f32);
        let seed = u32::from_le_bytes(flares[32..36].try_into().unwrap());
        assert_eq!(seed, devices.flares[0].seed());
        let chaff = chaff_instances(&devices);
        assert_eq!(chaff.len(), 2 * CHAFF_BYTES);
        let seconds = f32::from_le_bytes(chaff[12..16].try_into().unwrap());
        assert_eq!(seconds, 0.25);
    }
}
