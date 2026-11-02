//! Experiment AP1: the lights of redrawn airports (`terrain::redrawn::lights`),
//! drawn as additive points in the world pass, tested against the world depth
//! without writing it, like burning flares. Steady lights are uploaded once
//! per scene; the sequenced flashers are rewritten from the world's tick
//! clock whenever one turns on or off, so a replay flashes as the flight did. Presentation is
//! `fitted`, agent choices of 2026-10-10: colours, how far each kind shows at
//! night and how much of it shows by day (only the PAPI, at full strength
//! and a larger point, so it reads on a daylight final from 2 to 3 nm; John,
//! 2026-10-10).
//! `TORE_AIRFIELD_LIGHTS=0` turns them off, to measure their cost.
use crate::gpu_setup::NamedSteps as _;
use crate::terrain::{
    Terrain,
    redrawn::lights::{Light, LightKind},
};

const INSTANCE_BYTES: usize = 12 * 4;
/// How the flashers sequence: a pass from the farthest to the threshold
/// every half second, each lit for a twentieth of it.
const FLASH_CYCLE_S: f64 = 0.5;
const FLASH_ON: f64 = 0.05;

/// Colour (linear radiance), reach in feet at night and the share shown in
/// full daylight, by kind.
fn look(kind: LightKind) -> ([f32; 3], f32, f32) {
    const NM: f32 = 6_076.;
    match kind {
        LightKind::Edge => ([1.5, 1.38, 1.17], 6. * NM, 0.),
        LightKind::Threshold => ([0.25, 1.0, 0.4], 6. * NM, 0.),
        LightKind::End => ([1.0, 0.12, 0.06], 4. * NM, 0.),
        LightKind::Approach => ([1.0, 0.95, 0.85], 10. * NM, 0.),
        LightKind::ApproachSide => ([1.0, 0.1, 0.06], 6. * NM, 0.),
        LightKind::Flasher => ([2.2, 2.3, 2.6], 12. * NM, 0.),
        LightKind::Papi => ([1.0, 1.0, 1.0], 8. * NM, 1.),
        LightKind::Taxiway => ([0.15, 0.3, 1.0], 2. * NM, 0.),
    }
}

fn kind_code(kind: LightKind) -> u32 {
    match kind {
        LightKind::Edge => 0,
        LightKind::Threshold => 1,
        LightKind::End => 2,
        LightKind::Approach => 3,
        LightKind::ApproachSide => 4,
        LightKind::Flasher => 5,
        LightKind::Papi => 6,
        LightKind::Taxiway => 7,
    }
}

fn push(bytes: &mut Vec<u8>, light: &Light, param: f64) {
    let (color, range, day) = look(light.kind);
    let facing = light.facing.unwrap_or([0., 0.]);
    for value in light
        .position
        .map(|v| v as f32)
        .into_iter()
        .chain(color)
        .chain(facing.map(|v| v as f32))
        .chain([param as f32, range, day])
    {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(kind_code(light.kind).to_le_bytes());
}

/// Whether flasher `phase` (0 first, toward 1 last) is lit at `seconds`.
pub fn flasher_lit(phase: f64, seconds: f64) -> bool {
    let at = (seconds / FLASH_CYCLE_S).rem_euclid(1.);
    let start = phase * (1. - FLASH_ON);
    (start..start + FLASH_ON).contains(&at)
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
}

impl Pipeline {
    /// `surface` holds the world material and shared lighting groups, as
    /// the flares use them.
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        surface: &wgpu::PipelineLayout,
    ) -> Self {
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Airfield lights"),
            layout: Some(surface),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("airfield_light_vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32,4=>Float32,5=>Float32,6=>Uint32],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("airfield_light_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
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
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Greater,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });
        Self { pipeline }
    }
}

/// This scene's light instances.
pub struct Lights {
    enabled: bool,
    /// The redrawn airports the buffers were built from.
    key: Option<(usize, usize, usize)>,
    steady: Option<(wgpu::Buffer, u32)>,
    flashers: Vec<Light>,
    flash: Option<wgpu::Buffer>,
    /// Which flashers the flash buffer holds lit, so an unchanged sequence
    /// (the same tick, or another view of it) is not written again.
    flash_lit: Option<Vec<bool>>,
    /// The weather tick `flash_lit` was worked out for: the pattern depends
    /// only on it, so other views of the same tick skip the work.
    flash_tick: Option<i64>,
}

impl Default for Lights {
    fn default() -> Self {
        Self::new()
    }
}

impl Lights {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var("TORE_AIRFIELD_LIGHTS").as_deref() != Ok("0"),
            key: None,
            steady: None,
            flashers: Vec::new(),
            flash: None,
            flash_lit: None,
            flash_tick: None,
        }
    }

    /// Uploads the world's lights when its airports changed, and this
    /// frame's flashers.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, world: &Terrain) {
        if !self.enabled {
            return;
        }
        let count: usize = world.redrawn.iter().map(|b| b.lights.len()).sum();
        let key = (world.redrawn.as_ptr() as usize, world.redrawn.len(), count);
        if self.key != Some(key) {
            self.key = Some(key);
            let mut steady = Vec::new();
            self.flashers.clear();
            self.flash_lit = None;
            self.flash_tick = None;
            for light in world.redrawn.iter().flat_map(|b| &b.lights) {
                if light.kind == LightKind::Flasher {
                    self.flashers.push(*light);
                } else {
                    push(&mut steady, light, light.param);
                }
            }
            let buffer = |label, bytes: &[u8]| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: bytes.len().max(INSTANCE_BYTES) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, bytes);
                buffer
            };
            self.steady = (!steady.is_empty()).then(|| {
                (
                    buffer("Airfield lights", &steady),
                    (steady.len() / INSTANCE_BYTES) as u32,
                )
            });
            self.flash = (!self.flashers.is_empty()).then(|| {
                buffer(
                    "Airfield flashers",
                    &vec![0; self.flashers.len() * INSTANCE_BYTES],
                )
            });
        }
        let tick = world.weather.ticks();
        if let Some(flash) = &self.flash
            && self.flash_tick != Some(tick)
        {
            self.flash_tick = Some(tick);
            let seconds = tick as f64 / 120.;
            let lit: Vec<bool> = self
                .flashers
                .iter()
                .map(|light| flasher_lit(light.param, seconds))
                .collect();
            if self.flash_lit.as_ref() != Some(&lit) {
                let mut bytes = Vec::with_capacity(self.flashers.len() * INSTANCE_BYTES);
                for (light, on) in self.flashers.iter().zip(&lit) {
                    push(&mut bytes, light, f64::from(u8::from(*on)));
                }
                queue.write_buffer(flash, 0, &bytes);
                self.flash_lit = Some(lit);
            }
        }
    }

    /// Inside the world pass, after the flares. The caller has bound the
    /// world material (group 0) and shared lighting (group 1).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &Pipeline) {
        if !self.enabled {
            return;
        }
        if let Some((buffer, count)) = &self.steady {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
        if let Some(buffer) = &self.flash {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..self.flashers.len() as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flashers_run_in_sequence_toward_the_threshold() {
        // The first flasher lights at the start of each cycle, the last at
        // its end, and each only for its share.
        assert!(flasher_lit(0., 0.01));
        assert!(!flasher_lit(0., 0.1));
        assert!(flasher_lit(1., 0.49));
        assert!(flasher_lit(0., 0.51));
        let lit = (0..100)
            .filter(|i| flasher_lit(0.5, f64::from(*i) * 0.005))
            .count();
        assert_eq!(lit, 5);
    }

    #[test]
    fn an_instance_packs_position_colour_beam_and_kind() {
        let light = Light {
            position: [1., 2., 3.],
            kind: LightKind::Papi,
            facing: Some([0., -1.]),
            param: 2.83,
        };
        let mut bytes = Vec::new();
        push(&mut bytes, &light, light.param);
        assert_eq!(bytes.len(), INSTANCE_BYTES);
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2)], [1., 2., 3.]);
        assert_eq!(f(7), -1.);
        assert_eq!(f(8), 2.83);
        assert_eq!(u32::from_le_bytes(bytes[44..48].try_into().unwrap()), 6);
    }
}
