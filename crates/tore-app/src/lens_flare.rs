//! Source lens-flare circles over a completed world view, before cockpit/UI.
//! Smooth presentation adds continuous optical emission; stepped compatibility
//! retains the imported palette remaps inside the original circles.
//!
//! The circles are scaled by how much of the sun's disc is in view. A small
//! compute pass samples the world depth at 64 points over the disc after the
//! world pass (sky is the only thing left at the cleared depth), keeps the
//! result per camera slot in a GPU buffer, eases it over a few frames, and the
//! composition reads it the same frame: no readback and no stall. Points off
//! the view count by a terrain ray ([`terrain_clear`]).
use crate::gpu_setup::NamedSteps as _;
use crate::{camera::Camera, scenery::Scenery, terrain::Terrain};
use std::time::Instant;

pub struct LensFlare {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    palette: wgpu::Texture,
    maps: wgpu::Texture,
    format: wgpu::TextureFormat,
    /// The world picture under the flare, one per output size: the main
    /// view, mirrors and camera panels alternate sizes within a frame.
    backings: Vec<(wgpu::Texture, wgpu::BindGroup)>,
    /// The backing of the last `prepare`, which `draw` reads.
    current: usize,
    /// The palette and fill bytes last uploaded, so an unchanged view does
    /// not upload them again.
    uploaded: Option<(Vec<u8>, Vec<u8>)>,
    /// How much of the sun is in view, eased, one value per camera slot.
    visibility: wgpu::Buffer,
    measure: wgpu::ComputePipeline,
    /// The sun probe of each camera slot: its uniform and bindings.
    probes: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// The slot of the last `prepare`, which `measure` reads.
    slot: usize,
    /// When each slot last measured, to ease by elapsed time.
    measured: [Option<Instant>; SLOTS],
    /// Slots whose next measurement replaces the eased value outright.
    snapped: [bool; SLOTS],
}
/// Bytes of the probe uniform (`Probe` in `sun_visibility.wgsl`).
const PROBE_BYTES: u64 = 48;
/// Camera slots with their own sun visibility (`Camera::weather_slot`).
const SLOTS: usize = 8;
/// Points over the sun's disc the world depth is sampled at (the same in
/// `sun_visibility.wgsl`).
const PROBE_POINTS: u32 = 64;
/// Time constant of the fade as the sun goes behind or out from behind
/// something, in seconds.
const FADE_SECONDS: f32 = 0.12;
/// A slot not measured for this long starts from the new value, not the old.
const STALE_SECONDS: f32 = 0.5;
/// The least disc radius sampled, in output pixels, so a tiny sun still fades.
const MIN_PROBE_RADIUS: f32 = 8.;
/// Backing sizes kept: the main view, mirrors and the camera panels.
const BACKINGS: usize = 4;
fn texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lens flare image"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
/// Where the sun is in a view, for the visibility probe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    pub direction: [f32; 3],
    /// Its centre in output pixels from the view's top left (off the view
    /// when the flare shows with the sun just beyond the edge).
    pub centre: [f32; 2],
    /// The disc's radius in output pixels.
    pub radius: f32,
    /// The view's size in output pixels.
    pub view: [f32; 2],
}
/// Source 0x4b4990 gates and offsets, normalized to the host drawable size.
/// Native x/y shifts and whole-pixel circle rounding are projection adaptations.
#[cfg(test)]
pub fn circles(
    world: &Terrain,
    scenery: &Scenery,
    camera: &Camera,
    size: [u32; 2],
) -> Vec<[f32; 4]> {
    place(world, scenery, camera, size).map_or_else(Vec::new, |(circles, _)| circles)
}
/// The circles and the sun's place, when the flare shows at all.
fn place(
    world: &Terrain,
    scenery: &Scenery,
    camera: &Camera,
    size: [u32; 2],
) -> Option<(Vec<[f32; 4]>, Sun)> {
    let celestial = scenery.celestial.as_ref()?;
    if !scenery.glare_enabled() {
        return None;
    }
    let layer = world.weather.sample(camera.position[1])?;
    let sun = if scenery.smooth_weather {
        let sun = crate::celestial::visual_sun_direction(&layer, &world.weather)?;
        if crate::celestial::glare_strength(world, scenery, camera.position[1], sun) <= 0. {
            return None;
        }
        sun
    } else {
        let angles = tore_sim::environment::sun_angles(&layer, world.weather.seconds_of_day())?;
        if angles[1] < 182 {
            return None;
        }
        crate::celestial::rotate([0., 0., 1.], angles)
    };
    let basis = camera.uniform(1., [0.; 4], [0; 3]);
    let dot = |i: usize| (0..3).map(|j| sun[j] * basis[i + j]).sum::<f32>();
    let z = dot(12);
    if z <= 0. {
        return None;
    }
    let focal = size[1] as f32 * camera.view_fraction * 0.5 * 1.7320508 * camera.zoom;
    let offset = [dot(4) * focal / z, -dot(8) * focal / z];
    let [w, h] = [size[0] as f32, size[1] as f32 * camera.view_fraction];
    if offset[0].abs() > w * 9. / 16.
        || offset[1].abs() > h * 9. / 16.
        || offset[0].abs() * 320. / w + offset[1].abs() * 240. / h < 10.
    {
        return None;
    }
    let circles = celestial
        .flare
        .circles
        .iter()
        .map(|c| {
            [
                w * 0.5 + offset[0] * f32::from(c.offset_percent) / 100.,
                h * 0.5 + offset[1] * f32::from(c.offset_percent) / 100.,
                f32::from(c.radius) * w / 320.,
                f32::from(c.fill - 265),
            ]
        })
        .collect();
    let tangent = sun_tangent(scenery);
    Some((
        circles,
        Sun {
            direction: sun,
            centre: [w * 0.5 + offset[0], h * 0.5 + offset[1]],
            radius: (tangent * focal).max(MIN_PROBE_RADIUS),
            view: [w, h],
        },
    ))
}
/// The sun disc's radius as a tangent, from its solid circle (a small
/// default when the art has none).
fn sun_tangent(scenery: &Scenery) -> f32 {
    scenery
        .celestial
        .as_ref()
        .and_then(|c| c.solid_sun_radius())
        .map_or(0.02, f32::tan)
}
/// The point `index` of `points` over a disc of `radius` about `centre`: a
/// golden-angle spiral, even over the area. The compute pass
/// (`sun_visibility.wgsl`) places its samples the same way.
pub fn disc_point(index: u32, points: u32, centre: [f32; 2], radius: f32) -> [f32; 2] {
    let r = ((index as f32 + 0.5) / points as f32).sqrt() * radius;
    let a = index as f32 * 2.399_963_2;
    [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
}
/// How far to move toward a new value after `seconds`: the exponential ease
/// with [`FADE_SECONDS`], or a jump for a slot not measured for a while.
fn ease(seconds: Option<f32>) -> f32 {
    match seconds {
        Some(s) if s <= STALE_SECONDS => 1. - (-s.max(0.) / FADE_SECONDS).exp(),
        _ => 1.,
    }
}
/// The fraction of the sun's disc the terrain leaves open along the sun's
/// direction from `eye`: five rays, one through the centre and four through
/// the disc's edge ring. `ground` is the terrain height at a point (feet).
/// Used where the depth cannot say (off the view) and for the whiteout.
pub fn terrain_clear(
    eye: [f64; 3],
    sun: [f32; 3],
    tangent: f32,
    ground: &impl Fn(f64, f64) -> f64,
) -> f32 {
    let d = sun.map(f64::from);
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(length.is_finite() && length > 0.) {
        return 1.;
    }
    let d = d.map(|v| v / length);
    // Two unit vectors across the direction.
    let helper = if d[1].abs() < 0.9 {
        [0., 1., 0.]
    } else {
        [1., 0., 0.]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let unit = |v: [f64; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|x| x / n)
    };
    let u = unit(cross(d, helper));
    let v = cross(d, u);
    let ring = f64::from(tangent) * 0.7;
    let mut open = f32::from(ray_clear(eye, d, ground));
    for (a, b) in [(1., 0.), (-1., 0.), (0., 1.), (0., -1.)] {
        let ray = unit(std::array::from_fn(|i| d[i] + ring * (a * u[i] + b * v[i])));
        open += f32::from(ray_clear(eye, ray, ground));
    }
    open / 5.
}
/// Whether the terrain leaves a ray from `eye` along the unit `d` open.
fn ray_clear(eye: [f64; 3], d: [f64; 3], ground: &impl Fn(f64, f64) -> f64) -> bool {
    use tore_formats::theater::{CELL_FEET, HEIGHT_FEET};
    // A quarter cell is finer than anything the height grid can hide, and
    // no ground is higher than the tallest cell.
    let step = f64::from(CELL_FEET) / 4.;
    let top = 255. * f64::from(HEIGHT_FEET);
    let mut t = step;
    while t <= RAY_RANGE_FEET {
        let p: [f64; 3] = std::array::from_fn(|i| eye[i] + d[i] * t);
        if p[1] > top {
            return true;
        }
        if p[1] <= ground(p[0], p[2]) {
            return false;
        }
        t += step;
    }
    true
}
/// The cosine between the view and the sun above which the palette whitens
/// (the response starts at 0.95; this is a hair under it). The terrain ray is
/// only worked out for a sun this close to the view's centre.
pub const WHITEOUT_FACING: f32 = 0.94;
/// How far a terrain ray looks, in feet (about 130 nautical miles).
const RAY_RANGE_FEET: f64 = 800_000.;
/// The terrain's share of the sun's disc left open from `eye` (see
/// [`terrain_clear`]), for a sun in the direction `sun`.
pub fn terrain_open(world: &Terrain, scenery: &Scenery, eye: [f64; 3], sun: [f32; 3]) -> f32 {
    terrain_clear(eye, sun, sun_tangent(scenery), &|x, z| {
        f64::from(world.height(x as f32, z as f32))
    })
}
impl LensFlare {
    /// `depth_layout` is the bind group layout of the world depth (one
    /// binding, multisampled when `samples` is more than one) that
    /// [`LensFlare::measure`] reads.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth_layout: &wgpu::BindGroupLayout,
        samples: u32,
    ) -> Self {
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Original lens-flare fills"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lens_flare.wgsl").into()),
        });
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sun visibility probe"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let measure = Self::measure_pipeline(device, depth_layout, &probe_layout, samples);
        let visibility = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sun visibility"),
            size: 4 * SLOTS as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let probes = (0..SLOTS)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Sun visibility probe"),
                    size: PROBE_BYTES,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Sun visibility probe"),
                    layout: &probe_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: visibility.as_entire_binding(),
                        },
                    ],
                });
                (buffer, bind)
            })
            .collect();
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Source lens-flare composition"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lens flare circles"),
                size: 272,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            palette: texture(
                device,
                [256, 1],
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
            maps: texture(
                device,
                [256, 1],
                wgpu::TextureFormat::Rg8Uint,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
            format,
            backings: Vec::new(),
            current: 0,
            uploaded: None,
            visibility,
            measure,
            probes,
            slot: 0,
            measured: [None; SLOTS],
            snapped: [false; SLOTS],
        }
    }
    fn measure_pipeline(
        device: &wgpu::Device,
        depth_layout: &wgpu::BindGroupLayout,
        probe_layout: &wgpu::BindGroupLayout,
        samples: u32,
    ) -> wgpu::ComputePipeline {
        let measure_shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sun visibility"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sun_visibility.wgsl").into()),
        });
        let measure_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sun visibility"),
            bind_group_layouts: &[depth_layout, probe_layout],
            push_constant_ranges: &[],
        });
        device.named_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Sun visibility"),
            layout: Some(&measure_layout),
            module: &measure_shader,
            entry_point: Some(if samples > 1 { "measure_ms" } else { "measure" }),
            compilation_options: Default::default(),
            cache: None,
        })
    }

    /// Follow the world's new depth layout without resetting per-camera fades.
    /// Only the measurement pipeline reads multisampled depth. Composition,
    /// backing images, probe bindings and their visibility buffer stay valid.
    pub fn set_samples(
        &mut self,
        device: &wgpu::Device,
        depth_layout: &wgpu::BindGroupLayout,
        samples: u32,
    ) {
        self.measure = Self::measure_pipeline(
            device,
            depth_layout,
            &self.measure.get_bind_group_layout(1),
            samples,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &Terrain,
        scenery: &Scenery,
        camera: &Camera,
        size: [u32; 2],
        depth_scale: [f32; 2],
        palette: &[[u8; 3]; 256],
    ) -> Option<wgpu::TextureView> {
        let (circles, sun) = place(world, scenery, camera, size)?;
        if circles.is_empty() {
            return None;
        }
        if let Some(i) = self
            .backings
            .iter()
            .position(|(t, _)| t.width() == size[0] && t.height() == size[1])
        {
            self.current = i;
        } else {
            let source = texture(
                device,
                size,
                self.format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lens flare inputs"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &source.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &self.palette.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &self.maps.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.visibility.as_entire_binding(),
                    },
                ],
            });
            if self.backings.len() >= BACKINGS {
                self.backings.remove(0);
            }
            self.backings.push((source, bind));
            self.current = self.backings.len() - 1;
        }
        let strength = if scenery.smooth_weather {
            world
                .weather
                .sample(camera.position[1])
                .and_then(|layer| crate::celestial::visual_sun_direction(&layer, &world.weather))
                .map_or(0., |sun| {
                    crate::celestial::glare_strength(world, scenery, camera.position[1], sun)
                })
        } else {
            1.
        };
        self.slot = camera.weather_slot.min(SLOTS - 1);
        self.probe(queue, world, scenery, camera, &sun, depth_scale);
        let mut values = vec![
            circles.len() as f32,
            strength,
            f32::from(scenery.smooth_weather),
            self.slot as f32,
        ];
        values.extend(circles.into_iter().flatten());
        values.resize(68, 0.);
        crate::sim_renderer::write_floats(queue, &self.uniform, 0, &values);
        let pixels: Vec<u8> = palette
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect();
        let maps = world.weather.configuration().flare_fills();
        let maps: Vec<u8> = (0..256).flat_map(|i| [maps[0][i], maps[1][i]]).collect();
        if self
            .uploaded
            .as_ref()
            .is_none_or(|(p, m)| *p != pixels || *m != maps)
        {
            for (t, b, pitch) in [(&self.palette, &pixels, 1024), (&self.maps, &maps, 512)] {
                queue.write_texture(
                    t.as_image_copy(),
                    b,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(pitch),
                        rows_per_image: Some(1),
                    },
                    t.size(),
                );
            }
            self.uploaded = Some((pixels, maps));
        }
        Some(
            self.backings[self.current]
                .0
                .create_view(&Default::default()),
        )
    }
    /// The next view of each slot takes its measurement at once.
    pub fn snap(&mut self) {
        self.snapped = [true; SLOTS];
    }
    /// Writes the sun probe of this view's slot: where to sample the depth,
    /// what the points off the view count as, and how far to ease.
    fn probe(
        &mut self,
        queue: &wgpu::Queue,
        world: &Terrain,
        scenery: &Scenery,
        camera: &Camera,
        sun: &Sun,
        depth_scale: [f32; 2],
    ) {
        let now = Instant::now();
        let ease = if std::mem::take(&mut self.snapped[self.slot]) {
            1.
        } else {
            ease(self.measured[self.slot].map(|then| now.duration_since(then).as_secs_f32()))
        };
        self.measured[self.slot] = Some(now);
        let inside =
            |p: [f32; 2]| p[0] >= 0. && p[1] >= 0. && p[0] < sun.view[0] && p[1] < sun.view[1];
        let all_inside =
            (0..PROBE_POINTS).all(|i| inside(disc_point(i, PROBE_POINTS, sun.centre, sun.radius)));
        // The depth says nothing off the view: the terrain's share stands in.
        let outside = if all_inside {
            1.
        } else {
            terrain_open(world, scenery, camera.position, sun.direction)
        };
        crate::sim_renderer::write_floats(
            queue,
            &self.probes[self.slot].0,
            0,
            &[
                sun.centre[0],
                sun.centre[1],
                sun.radius,
                outside,
                sun.view[0],
                sun.view[1],
                depth_scale[0],
                depth_scale[1],
                ease,
                self.slot as f32,
                0.,
                0.,
            ],
        );
    }
    /// Measures how much of the sun's disc is in view, from the world depth
    /// of the pass just drawn. Call after the world pass and before `draw`,
    /// when `prepare` returned a picture.
    pub fn measure(&self, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::BindGroup) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Sun visibility"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.measure);
        pass.set_bind_group(0, depth, &[]);
        pass.set_bind_group(1, &self.probes[self.slot].1, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Lens flare after world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.backings[self.current].1, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn disc_points_cover_the_disc_evenly() {
        let (centre, radius) = ([100., 50.], 20.);
        let points: Vec<[f32; 2]> = (0..PROBE_POINTS)
            .map(|i| disc_point(i, PROBE_POINTS, centre, radius))
            .collect();
        for p in &points {
            let d = (p[0] - centre[0]).hypot(p[1] - centre[1]);
            assert!(d <= radius, "{p:?} is {d} from the centre");
        }
        // Half the disc on each side of a line through the centre, and a
        // quarter in each quadrant, give or take the spiral's lumps.
        let left = points.iter().filter(|p| p[0] < centre[0]).count();
        assert!((28..=36).contains(&left), "{left} of 64 left of the centre");
        let inner = points
            .iter()
            .filter(|p| (p[0] - centre[0]).hypot(p[1] - centre[1]) < radius * 0.5)
            .count();
        // A quarter of the area lies inside half the radius.
        assert!(
            (12..=20).contains(&inner),
            "{inner} of 64 in the inner half"
        );
    }
    /// The sun's share of the disc a depth image leaves open, sampled the way
    /// the compute pass does: a stand-in for the shader, to check it against.
    fn reference_fraction(
        depth: impl Fn(i32, i32) -> f32,
        centre: [f32; 2],
        radius: f32,
        view: [f32; 2],
        scale: [f32; 2],
        outside: f32,
    ) -> f32 {
        let mut total = 0.;
        for i in 0..PROBE_POINTS {
            let p = disc_point(i, PROBE_POINTS, centre, radius);
            total += if p[0] >= 0. && p[1] >= 0. && p[0] < view[0] && p[1] < view[1] {
                f32::from(depth((p[0] * scale[0]) as i32, (p[1] * scale[1]) as i32) <= 0.)
            } else {
                outside
            };
        }
        total / PROBE_POINTS as f32
    }
    #[test]
    fn the_disc_fraction_follows_what_covers_it() {
        let view = [200., 100.];
        let sky_above = |row: f32| move |_: i32, y: i32| if (y as f32) < row { 0. } else { 0.5 };
        // Nothing covers the sun, or everything does.
        let clear = reference_fraction(sky_above(1e9), [100., 50.], 10., view, [1.; 2], 1.);
        let hidden = reference_fraction(sky_above(-1e9), [100., 50.], 10., view, [1.; 2], 1.);
        assert_eq!((clear, hidden), (1., 0.));
        // A ridge across the middle of the disc leaves about half; across
        // its top or bottom edge, little or most.
        let half = reference_fraction(sky_above(50.), [100., 50.], 10., view, [1.; 2], 1.);
        assert!((half - 0.5).abs() < 0.1, "{half}");
        // The lower the ridge's top row, the more of the disc shows.
        let mut last = 0.;
        for row in (30..=70).step_by(2) {
            let f = reference_fraction(sky_above(row as f32), [100., 50.], 10., view, [1.; 2], 1.);
            assert!(f >= last, "row {row}: {f} after {last}");
            last = f;
        }
    }
    #[test]
    fn points_off_the_view_count_as_the_terrain_ray_says() {
        let view = [200., 100.];
        let all_sky = |_: i32, _: i32| 0.;
        // Centred on the right edge: half the disc is off the view.
        let open = reference_fraction(all_sky, [200., 50.], 10., view, [1.; 2], 1.);
        let blocked = reference_fraction(all_sky, [200., 50.], 10., view, [1.; 2], 0.);
        let part = reference_fraction(all_sky, [200., 50.], 10., view, [1.; 2], 0.5);
        assert_eq!(open, 1.);
        assert!((blocked - 0.5).abs() < 0.1, "{blocked}");
        assert!((part - (open + blocked) / 2.).abs() < 1e-5);
        // Wholly off the view: only the ray decides.
        assert_eq!(
            reference_fraction(all_sky, [400., 50.], 10., view, [1.; 2], 0.),
            0.
        );
    }
    #[test]
    fn the_depth_scale_maps_output_pixels_to_depth_pixels() {
        // Depth drawn at half the output size: the output pixel (100, 40) is
        // the depth pixel (50, 20).
        let seen = std::cell::RefCell::new(Vec::new());
        reference_fraction(
            |x, y| {
                seen.borrow_mut().push((x, y));
                0.
            },
            [100.5, 40.5],
            0.4,
            [200., 100.],
            [0.5, 0.5],
            1.,
        );
        assert!(seen.borrow().iter().all(|&p| p == (50, 20)));
    }
    #[test]
    fn the_fade_eases_by_elapsed_time_and_snaps_when_stale() {
        // First use, or a long gap: jump to the measured value.
        assert_eq!(ease(None), 1.);
        assert_eq!(ease(Some(STALE_SECONDS + 0.01)), 1.);
        // No time, no move; one time constant, 63 percent.
        assert_eq!(ease(Some(0.)), 0.);
        assert!((ease(Some(FADE_SECONDS)) - (1. - (-1_f32).exp())).abs() < 1e-6);
        // A 60 Hz frame moves part of the way, never all of it.
        let frame = ease(Some(1. / 60.));
        assert!(frame > 0.05 && frame < 0.2, "{frame}");
        // Frames of 1/60 s take the visibility from 1 to under 5 percent in
        // well under a second, and never overshoot.
        let mut value = 1_f32;
        let mut frames = 0;
        while value > 0.05 {
            value += (0. - value) * frame;
            assert!(value >= 0.);
            frames += 1;
        }
        assert!((10..=40).contains(&frames), "{frames} frames");
    }
    /// Flat ground at sea level with a wall `top` feet high from `at` feet
    /// north on.
    fn ridge(at: f64, top: f64) -> impl Fn(f64, f64) -> f64 {
        move |_x, z| if z >= at { top } else { 0. }
    }
    #[test]
    fn terrain_leaves_a_high_sun_open_and_a_hidden_one_shut() {
        let eye = [100_000., 3_000., 0.];
        let tangent = 0.02;
        // The sun 30 degrees up due north.
        let sun = [0., 0.5, 0.866_025_4];
        assert_eq!(terrain_clear(eye, sun, tangent, &ridge(1e9, 0.)), 1.);
        // A 30,000 ft wall 50,000 ft away blocks a 30 degree sun (its ray is
        // at 31,000 ft there) only when the wall is higher.
        assert_eq!(
            terrain_clear(eye, sun, tangent, &ridge(50_000., 20_000.)),
            1.
        );
        assert_eq!(
            terrain_clear(eye, sun, tangent, &ridge(50_000., 40_000.)),
            0.
        );
        // Straight overhead, a wall to the north is out of the way.
        assert_eq!(
            terrain_clear(eye, [0., 1., 0.], tangent, &ridge(50_000., 30_000.)),
            1.
        );
        // Over the tallest ground there is, the ray cannot hit anything.
        assert!(ray_clear([0., 70_000., 0.], [0., 0., 1.], &|_, _| 65_000.,));
    }
    #[test]
    fn a_sun_half_behind_a_ridge_is_partly_open() {
        let eye = [0., 1_000., 0.];
        let tangent = 0.05;
        // The sun 3 degrees up due north; a ridge 100,000 ft away whose top
        // sits at the height of the sun's centre ray there, with the disc's
        // radius (0.05 rad, 5,000 ft at that range) above and below it.
        let sun_y = 3_f64.to_radians().sin() as f32;
        let sun = [0., sun_y, (1. - sun_y * sun_y).sqrt()];
        let centre_height = 1_000. + 100_000. * f64::from(sun_y / sun[2]);
        let open = terrain_clear(eye, sun, tangent, &ridge(100_000., centre_height));
        assert!(open > 0.1 && open < 0.9, "{open}");
        // Lower the ridge and more of the disc shows.
        let lower = terrain_clear(eye, sun, tangent, &ridge(100_000., centre_height - 3_500.));
        let higher = terrain_clear(eye, sun, tangent, &ridge(100_000., centre_height + 3_500.));
        assert!(higher < open && open < lower, "{higher} {open} {lower}");
    }

    fn depth_layout(device: &wgpu::Device, samples: u32) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Test world depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: u32::from(samples > 1),
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: samples > 1,
                },
                count: None,
            }],
        })
    }
    fn gpu() -> (wgpu::Device, wgpu::Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap()
        })
    }
    /// Runs the sun measurement over a `size` depth image the test fills with
    /// `depth(x, y)`, the world pass's way (a render pass clears it to 0 and
    /// draws the covered part), `samples` per pixel, and returns the slot's
    /// value after the probe; `previous` is what the slot held before.
    fn measure_on_gpu(
        samples: u32,
        size: [u32; 2],
        covered: impl Fn(u32, u32) -> bool,
        probe: [f32; 12],
        previous: f32,
    ) -> f32 {
        let (device, queue) = gpu();
        let layout = depth_layout(&device, samples);
        let flare = LensFlare::new(&device, wgpu::TextureFormat::Rgba8Unorm, &layout, samples);
        queue.write_buffer(&flare.visibility, 0, &previous.to_le_bytes());
        queue.write_buffer(&flare.probes[0].0, 0, &probe.map(f32::to_le_bytes).concat());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Test depth"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = depth.create_view(&Default::default());
        // Draw one point per covered pixel at depth 0.5, over a clear to 0.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                "@vertex fn v(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{
                    let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
                    return vec4(p[i],0.5,1.);}
                 @fragment fn f()->@location(0) vec4<f32>{return vec4(0.);}"
                    .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("v"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
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
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            // One scissored full-screen triangle per covered run of a row.
            for y in 0..size[1] {
                let mut x = 0;
                while x < size[0] {
                    if covered(x, y) {
                        let start = x;
                        while x < size[0] && covered(x, y) {
                            x += 1;
                        }
                        pass.set_scissor_rect(start, y, x - start, 1);
                        pass.draw(0..3, 0..1);
                    } else {
                        x += 1;
                    }
                }
            }
        }
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: u32::from(samples > 1),
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        flare.measure(&mut encoder, &bind);
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4 * SLOTS as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&flare.visibility, 0, &read, 0, 4 * SLOTS as u64);
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = read.slice(..).get_mapped_range();
        f32::from_le_bytes(data[..4].try_into().unwrap())
    }
    fn probe(
        centre: [f32; 2],
        radius: f32,
        view: [f32; 2],
        scale: f32,
        outside: f32,
        ease: f32,
    ) -> [f32; 12] {
        [
            centre[0], centre[1], radius, outside, view[0], view[1], scale, scale, ease, 0., 0., 0.,
        ]
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_sun_visibility_matches_the_disc_fraction_with_and_without_msaa() {
        for samples in [1, 4] {
            // Sky above row 50 of a 200 by 100 depth image, ground below.
            let ground = |_x: u32, y: u32| y >= 50;
            let reference = |centre: [f32; 2], radius: f32, scale: f32, outside: f32| {
                reference_fraction(
                    |_, y| if y >= 50 { 0.5 } else { 0. },
                    centre,
                    radius,
                    [200. * scale, 100. * scale],
                    [1. / scale; 2],
                    outside,
                )
            };
            for (centre, radius) in [
                ([100., 50.], 10.),
                ([100., 20.], 10.),
                ([100., 90.], 10.),
                ([100., 46.], 10.),
            ] {
                let got = measure_on_gpu(
                    samples,
                    [200, 100],
                    ground,
                    probe(centre, radius, [200., 100.], 1., 1., 1.),
                    0.,
                );
                let want = reference(centre, radius, 1., 1.);
                // MSAA picks one sample per point, and a point at an edge may
                // fall either side of it.
                assert!(
                    (got - want).abs() <= if samples > 1 { 0.05 } else { 1e-6 },
                    "{samples} samples, disc at {centre:?}: gpu {got}, reference {want}"
                );
            }
            // Half the disc off the right edge counts as the ray says.
            for outside in [0., 1.] {
                let got = measure_on_gpu(
                    samples,
                    [200, 100],
                    ground,
                    probe([200., 20.], 10., [200., 100.], 1., outside, 1.),
                    0.,
                );
                let want = 0.5 + 0.5 * outside;
                assert!((got - want).abs() < 0.1, "outside {outside}: {got}");
            }
            // The depth is half the size of the output: output (100, 40) is
            // depth pixel (50, 20).
            let got = measure_on_gpu(
                samples,
                [100, 50],
                |_x, y| y >= 25,
                probe([100., 40.], 6., [200., 100.], 0.5, 1., 1.),
                0.,
            );
            assert_eq!(got, 1., "{samples} samples, half-size depth");
            // The value eases from what the slot held.
            let got = measure_on_gpu(
                samples,
                [200, 100],
                |_x, _y| true,
                probe([100., 50.], 10., [200., 100.], 1., 1., 0.25),
                1.,
            );
            assert!((got - 0.75).abs() < 1e-6, "eased to {got}");
        }
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_smooth_glare_preserves_gradient_without_palette_bands() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let layout = depth_layout(&device, 1);
            let mut flare = LensFlare::new(&device, wgpu::TextureFormat::Rgba8Unorm, &layout, 1);
            // The whole sun is in view.
            queue.write_buffer(
                &flare.visibility,
                0,
                &[1_f32; 8].map(f32::to_le_bytes).concat(),
            );
            let input = texture(
                &device,
                [64, 1],
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let pixels: Vec<u8> = (0..64)
                .flat_map(|i| [20 + i * 3, 80, 220 - i * 2, 255])
                .collect();
            queue.write_texture(
                input.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
                input.size(),
            );
            // Deliberately useless maps: smooth glare must not consult them.
            queue.write_texture(
                flare.maps.as_image_copy(),
                &[0; 512],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(512),
                    rows_per_image: Some(1),
                },
                flare.maps.size(),
            );
            let circles = [[16., 0.5, 9., 0.], [38., 0.5, 13., 1.]];
            let mut uniform = vec![2., 1., 1., 0.];
            uniform.extend(circles.into_iter().flatten());
            uniform.resize(68, 0.);
            let bytes: Vec<u8> = uniform.into_iter().flat_map(f32::to_le_bytes).collect();
            queue.write_buffer(&flare.uniform, 0, &bytes);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &flare.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &input.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &flare.palette.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &flare.maps.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: flare.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: flare.visibility.as_entire_binding(),
                    },
                ],
            });
            flare.backings = vec![(input, bind)];
            flare.current = 0;
            let output = texture(
                &device,
                [64, 1],
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            flare.draw(&mut encoder, &output.create_view(&Default::default()));
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                output.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let actual = buffer.slice(..).get_mapped_range();
            for x in 0..64 {
                let mut emission = [0_f32; 3];
                for (circle, tint, strength) in [
                    (circles[0], [1., 0.55, 0.22], 0.12),
                    (circles[1], [1., 0.32, 0.18], 0.08),
                ] {
                    let d = (x as f32 + 0.5 - circle[0]).abs();
                    let t = ((d - circle[2] + 1.5) / 1.5).clamp(0., 1.);
                    let coverage = 1. - t * t * (3. - 2. * t);
                    for channel in 0..3 {
                        emission[channel] += tint[channel] * strength * coverage;
                    }
                }
                for channel in 0..3 {
                    let base = f32::from(pixels[x * 4 + channel]) / 255.;
                    let expected = ((base + (1. - base) * (1. - (-emission[channel]).exp())) * 255.)
                        .round() as i32;
                    assert!(
                        (i32::from(actual[x * 4 + channel]) - expected).abs() <= 1,
                        "gradient pixel {x}, channel {channel}"
                    );
                }
                assert_eq!(actual[x * 4 + 3], 255);
            }
        });
    }
}
