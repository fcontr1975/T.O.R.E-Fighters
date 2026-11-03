//! Shared, opinionated surface lighting and geometric shadows.
//! Player-visible rules: docs/spec/surface-lighting.md.
use crate::gpu_setup::NamedSteps as _;
use crate::{camera::Camera, scenery::Scenery, terrain::Terrain};
use wgpu::util::DeviceExt;

const NEAR_EXTENT: f32 = 256.;
const MIDDLE_EXTENT: f32 = 8192.;
const DEPTH: f32 = 262144.;
/// Shadow projections, sun, moon, parameters and the shadow quality, then
/// the flare lights.
const UNIFORM_BYTES: usize = 68 * 4 + crate::countermeasure_renderer::MAX_FLARE_LIGHTS * 32;

/// What one [`crate::graphics::ShadowQuality`] draws. Every level that draws
/// shadows has the terrain, the aircraft and every ground object (static
/// placements and moving units) cast. Quality sets detail and caster classes;
/// distance sets reach independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Geometric shadows at all.
    pub enabled: bool,
    /// Each cascade map's side, texels.
    pub map: u32,
    /// How many cascades are drawn, nearest first.
    pub cascades: usize,
    /// Light-space half-widths in feet. Unused entries are zero.
    pub extents: [f32; 3],
    /// Soft-edge disc samples per receiver.
    pub disc_samples: u32,
    /// City buildings nearer than this cast their box (nautical miles; 0
    /// none).
    pub buildings_nm: f64,
    /// Trees nearer than this cast a sun-facing card, feet (0 none).
    pub trees_ft: f32,
}

impl Settings {
    /// Resolve run-only overrides on both initial creation and live changes.
    pub fn from_options(options: &crate::graphics::Options) -> Self {
        Self::of(
            options.shadow_quality.effective(),
            options
                .shadow_distance
                .effective()
                .nautical_miles(options.vegetation_distance_nm),
        )
    }

    /// Quality controls map detail and caster classes, distance controls reach.
    /// Keep near detail fixed and bring the middle map in for short reaches.
    fn of(quality: crate::graphics::ShadowQuality, distance_nm: u32) -> Self {
        use crate::graphics::ShadowQuality as Q;
        let reach = (f64::from(distance_nm) * crate::vegetation::NM) as f32;
        let medium = Self {
            enabled: true,
            map: 2048,
            cascades: 3,
            extents: [NEAR_EXTENT, MIDDLE_EXTENT.min(reach * 0.5), reach],
            disc_samples: 32,
            buildings_nm: f64::from(distance_nm),
            trees_ft: 0.,
        };
        match quality {
            Q::Off => Self {
                enabled: false,
                extents: [0.; 3],
                buildings_nm: 0.,
                ..medium
            },
            Q::Low => Self {
                map: 1024,
                cascades: 2,
                extents: [NEAR_EXTENT, reach, 0.],
                disc_samples: 16,
                buildings_nm: 0.,
                ..medium
            },
            Q::Medium => medium,
            Q::High => Self {
                trees_ft: reach,
                ..medium
            },
            Q::Ultra => Self {
                map: 4096,
                trees_ft: reach,
                ..medium
            },
        }
    }
}

/// A cascade a level does not draw: every point projects outside it, so
/// receivers skip it as they skip a point beyond the maps.
const UNUSED_CASCADE: [f32; 16] = [
    0., 0., 0., 4., 0., 0., 0., 4., 0., 0., 0., 0.5, 1., 1., DEPTH, 0.,
];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[cfg(test)]
fn unit(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

// Each projection is three rows plus scale/bias parameters. Coordinates are
// relative to the view origin to retain precision in million-foot theaters.
fn projection(origin: [f32; 3], sun: [f32; 3], extent: f32, map: u32) -> [f32; 16] {
    // Continuous orthonormal basis on the sun's upper hemisphere. Switching
    // reference axes at an elevation threshold rotates the entire shadow grid.
    let sign = if sun[1] >= 0. { 1. } else { -1. };
    let a = -1. / (sign + sun[1]);
    let right = [
        1. + sign * sun[0] * sun[0] * a,
        -sign * sun[0],
        sign * sun[0] * sun[2] * a,
    ];
    let up = cross(sun, right);
    let texel = 2. * extent / map as f32;
    let snap = |axis| {
        let p = dot(origin, axis);
        (p - (p / texel).round() * texel) / extent
    };
    [
        right[0] / extent,
        right[1] / extent,
        right[2] / extent,
        snap(right),
        up[0] / extent,
        up[1] / extent,
        up[2] / extent,
        snap(up),
        -sun[0] / (2. * DEPTH),
        -sun[1] / (2. * DEPTH),
        -sun[2] / (2. * DEPTH),
        0.5,
        texel,
        extent,
        DEPTH,
        0.,
    ]
}

/// Sun and moon inputs shared by outside surfaces and cockpit ambient light.
pub(crate) fn sky_directions(
    world: &Terrain,
    scenery: &Scenery,
    altitude: f64,
) -> ([f32; 3], [f32; 3]) {
    let layer = world.weather.sample(altitude);
    let sun = layer
        .as_ref()
        .and_then(|l| crate::celestial::visual_sun_direction(l, &world.weather))
        .unwrap_or([0., -1., 0.]);
    // Smooth weather's moon moves across the sky; its light follows it
    // and fades as it sets ([`crate::night_sky`]).
    let moon = match scenery.smooth_weather {
        true => crate::night_sky::NightSky::for_view(
            world,
            1.,
            altitude,
            scenery
                .celestial
                .as_ref()
                .map_or([0., 0., 1.], |c| c.moon_center()),
        )
        .map(|sky| sky.moon_light()),
        false => layer
            .as_ref()
            .map(|l| crate::celestial::rotate([0., 0., 1.], [l.moon_azimuth, l.moon_elevation])),
    }
    .unwrap_or([0., 1., 0.]);
    (sun, moon)
}

// Visible circular-segment area and centroid, matching the rendered solid disc.
pub(crate) fn visible_sun(sun: [f32; 3], radius: f32) -> ([f32; 3], f32) {
    let elevation = sun[1].clamp(-1., 1.).asin();
    let x = (elevation / radius).clamp(-1., 1.);
    if x <= -1. {
        return (sun, 0.);
    }
    if x >= 1. {
        return (sun, 1.);
    }
    let root = (1. - x * x).sqrt();
    let area = (-x).acos() + x * root;
    if area <= 1e-6 {
        return (sun, 0.);
    }
    let center = (elevation + (2. / 3.) * radius * root.powi(3) / area).max(0.);
    let horizontal = sun[0].hypot(sun[2]).max(1e-6);
    (
        [
            sun[0] / horizontal * center.cos(),
            center.sin(),
            sun[2] / horizontal * center.cos(),
        ],
        area / std::f32::consts::PI,
    )
}

/// Area-weighted normals shared across terrain triangle and material boundaries.
/// The separate stream leaves geometry and shadow-caster positions untouched.
pub(crate) fn terrain_normals(vertices: &[f32]) -> Vec<f32> {
    use std::collections::HashMap;
    let key = |v: &[f32]| [v[0], v[1], v[2]].map(|x| if x == 0. { 0 } else { x.to_bits() });
    let mut sums = HashMap::<[u32; 3], [f64; 3]>::new();
    for triangle in vertices.chunks_exact(30) {
        let a: [f64; 3] =
            std::array::from_fn(|i| f64::from(triangle[10 + i]) - f64::from(triangle[i]));
        let b: [f64; 3] =
            std::array::from_fn(|i| f64::from(triangle[20 + i]) - f64::from(triangle[i]));
        let mut normal = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        if normal[1] < 0. {
            normal = normal.map(|v| -v);
        }
        for vertex in triangle.chunks_exact(10) {
            let sum = sums.entry(key(vertex)).or_default();
            for i in 0..3 {
                sum[i] += normal[i];
            }
        }
    }
    vertices
        .chunks_exact(10)
        .flat_map(|v| {
            let n = sums.get(&key(v)).copied().unwrap_or([0., 1., 0.]);
            let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
            if length > 0. {
                n.map(|v| (v / length) as f32)
            } else {
                [0., 1., 0.]
            }
        })
        .collect()
}

pub struct SurfaceLighting {
    pub layout: wgpu::BindGroupLayout,
    pub bind: wgpu::BindGroup,
    caster_bind: wgpu::BindGroup,
    /// The shadow casting pipeline layout (material, projection), for
    /// casters drawn by other renderers (the city fill, the trees).
    pub caster_layout: wgpu::PipelineLayout,
    uniform: wgpu::Buffer,
    views: Vec<wgpu::TextureView>,
    comparison: wgpu::Sampler,
    point: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    /// Terrain whose texture has no cut-out texel: depth only, no fragment
    /// stage (optimization S13a).
    opaque_terrain_pipeline: wgpu::RenderPipeline,
    /// Object triangles whose layer no fragment test discards: depth only
    /// (round 2).
    opaque_object_pipeline: wgpu::RenderPipeline,
    /// Static ground objects (airports, buildings, sites, bridges): world
    /// vertices placed as the terrain's are, with the object bias; textured
    /// triangles keep their cut-out test, the rest draw depth only.
    static_pipeline: wgpu::RenderPipeline,
    opaque_static_pipeline: wgpu::RenderPipeline,
    /// Each cascade's projection rows from the last [`Self::prepare`], for
    /// culling casters.
    cascades: [[f32; 16]; 3],
    settings: Settings,
}

/// The cascade maps (one layer each) at `size` texels square, and the
/// receivers' bind group over them.
fn maps(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    comparison: &wgpu::Sampler,
    point: &wgpu::Sampler,
    size: u32,
) -> (Vec<wgpu::TextureView>, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("World geometric shadow maps"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 3,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let views = (0..3)
        .map(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let all = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("World shadow receivers"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&all),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(comparison),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(point),
            },
        ],
    });
    (views, bind)
}

impl SurfaceLighting {
    pub fn new(
        device: &wgpu::Device,
        material: &wgpu::BindGroupLayout,
        shader: &wgpu::ShaderModule,
        settings: Settings,
    ) -> Self {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Surface light and shadow projections"),
            contents: &[0; UNIFORM_BYTES],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let comparison = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Filtered shadow comparison"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Gathers the four depths around a shadow tap (round 2): nearest,
        // clamped, so it returns stored values unfiltered.
        let point = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow depth gather"),
            ..Default::default()
        });
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shared surface lighting"),
            entries: &[
                uniform_entry,
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let (views, bind) = maps(device, &layout, &uniform, &comparison, &point, settings.map);
        // Casters must not bind the sampled depth texture while writing it.
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow projection only"),
            entries: &[uniform_entry],
        });
        let caster_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shadow caster projection"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shadow casting"),
            bind_group_layouts: &[material, &caster_layout],
            push_constant_ranges: &[],
        });
        // Terrain raster bias is constant 2 / slope 3, objects' constant 1 /
        // slope 1 (docs/spec/surface-lighting.md).
        let make = |label: &str, vertex: &str, fragment: Option<&str>, terrain: bool| {
            device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 40,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32],
                    }],
                },
                fragment: fragment.map(|entry| wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: if terrain { 2 } else { 1 },
                        slope_scale: if terrain { 3. } else { 1. },
                        clamp: 0.,
                    },
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        Self {
            layout,
            bind,
            caster_bind,
            caster_layout: pipeline_layout.clone(),
            uniform,
            views,
            comparison,
            point,
            pipeline: make(
                "shadow_fragment",
                "shadow_object_vertex",
                Some("shadow_fragment"),
                false,
            ),
            terrain_pipeline: make(
                "terrain_shadow_fragment",
                "shadow_vertex",
                Some("terrain_shadow_fragment"),
                true,
            ),
            opaque_terrain_pipeline: make("opaque terrain shadow", "shadow_vertex", None, true),
            opaque_object_pipeline: make(
                "opaque object shadow",
                "shadow_object_vertex",
                None,
                false,
            ),
            static_pipeline: make(
                "static object shadow",
                "shadow_vertex",
                Some("terrain_shadow_fragment"),
                false,
            ),
            opaque_static_pipeline: make(
                "opaque static object shadow",
                "shadow_vertex",
                None,
                false,
            ),
            cascades: [[0.; 16]; 3],
            settings,
        }
    }

    /// The shadow quality's settings in use.
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// Changes the shadow quality: new maps when their size changes.
    pub fn set_settings(&mut self, device: &wgpu::Device, settings: Settings) {
        if settings.map != self.settings.map {
            (self.views, self.bind) = maps(
                device,
                &self.layout,
                &self.uniform,
                &self.comparison,
                &self.point,
                settings.map,
            );
        }
        self.settings = settings;
    }
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
        flares: &[crate::countermeasure_renderer::FlareLight],
    ) -> bool {
        let (sun, moon) = sky_directions(world, scenery, camera.position[1]);
        let radius = scenery
            .celestial
            .as_ref()
            .and_then(|c| c.solid_sun_radius())
            .unwrap_or(0.5_f32.to_radians());
        let (light, strength) = visible_sun(sun, radius);
        let active = scenery.smooth_weather && strength > 0. && self.settings.enabled;
        let mut values = Vec::with_capacity(68);
        for (cascade, extent) in self.settings.extents.into_iter().enumerate() {
            self.cascades[cascade] = if self.settings.enabled && cascade < self.settings.cascades {
                projection(
                    camera.position.map(|v| v as f32),
                    light,
                    extent,
                    self.settings.map,
                )
            } else {
                UNUSED_CASCADE
            };
            values.extend(self.cascades[cascade]);
        }
        values.extend([
            camera.position[0] as f32,
            camera.position[1] as f32,
            camera.position[2] as f32,
            f32::from(active),
        ]);
        values.extend([
            light[0],
            light[1],
            light[2],
            f32::from(scenery.smooth_weather),
        ]);
        values.extend([moon[0], moon[1], moon[2], 0.]);
        let origin = camera.position;
        let flares = crate::countermeasure_renderer::nearest(flares, origin);
        values.extend([strength, sun[1], radius.tan(), flares.len() as f32]);
        values.extend([
            self.settings.map as f32,
            self.settings.disc_samples as f32,
            f32::from(self.settings.trees_ft > 0.),
            0.,
        ]);
        let count = flares.len();
        values.extend(flares.into_iter().flatten());
        values.resize(
            values.len() + (crate::countermeasure_renderer::MAX_FLARE_LIGHTS - count) * 8,
            0.,
        );
        debug_assert_eq!(values.len() * 4, UNIFORM_BYTES);
        crate::sim_renderer::write_floats(queue, &self.uniform, 0, &values);
        active
    }

    /// Draws the shadow casters into the level's cascades. Each cascade
    /// draws only the terrain chunks, static object chunks and object
    /// batches inside its volume (exact: the rest are wholly clipped), and
    /// the triangles that cannot cut out without a fragment stage. A
    /// depth-only pass keeps the nearest caster in any order, so the
    /// ranges' order does not matter.
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        terrain: TerrainCasters<'_>,
        statics: Option<TerrainCasters<'_>>,
        objects: &[Caster<'_>],
    ) {
        let (mut opaque, mut cutout) = (Vec::new(), Vec::new());
        for (cascade, view) in self.views.iter().enumerate().take(self.settings.cascades) {
            let clip = crate::terrain_chunks::Clip::orthographic(&self.cascades[cascade]);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("World geometry shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            let instance = cascade as u32..cascade as u32 + 1;
            pass.set_bind_group(1, &self.caster_bind, &[]);
            let chunked = std::iter::once((
                &terrain,
                &self.opaque_terrain_pipeline,
                &self.terrain_pipeline,
            ))
            .chain(
                statics
                    .iter()
                    .map(|s| (s, &self.opaque_static_pipeline, &self.static_pipeline)),
            );
            for (set, opaque_pipeline, cutout_pipeline) in chunked {
                set.chunks.casters(set.eye, &clip, &mut opaque, &mut cutout);
                if opaque.is_empty() && cutout.is_empty() {
                    continue;
                }
                pass.set_bind_group(0, set.bind, &[]);
                pass.set_vertex_buffer(0, set.buffer.slice(..));
                pass.set_pipeline(opaque_pipeline);
                for range in &opaque {
                    pass.draw(range.clone(), instance.clone());
                }
                pass.set_pipeline(cutout_pipeline);
                for range in &cutout {
                    pass.draw(range.clone(), instance.clone());
                }
            }
            let kept: Vec<&Caster<'_>> = objects
                .iter()
                .filter(|o| o.count > 0 && !clip.rejects(&o.bounds))
                .collect();
            for (pipeline, tested) in [
                (&self.opaque_object_pipeline, false),
                (&self.pipeline, true),
            ] {
                pass.set_pipeline(pipeline);
                for object in &kept {
                    let ranges = if tested { object.tested } else { object.opaque };
                    let mut bound = false;
                    for range in ranges {
                        let range = range.start.min(object.count)..range.end.min(object.count);
                        if range.is_empty() {
                            continue;
                        }
                        if !bound {
                            pass.set_bind_group(0, object.bind, &[]);
                            pass.set_vertex_buffer(0, object.buffer.slice(..));
                            bound = true;
                        }
                        pass.draw(range, instance.clone());
                    }
                }
            }
        }
    }
}

/// The terrain or the static ground objects for [`SurfaceLighting::draw`]:
/// a chunked buffer and the eye its world-space vertices are taken
/// relative to.
pub struct TerrainCasters<'a> {
    pub bind: &'a wgpu::BindGroup,
    pub buffer: &'a wgpu::Buffer,
    pub chunks: &'a crate::terrain_chunks::Chunks,
    pub eye: [f64; 3],
}

/// One object batch for [`SurfaceLighting::draw`], with the box of its
/// vertices relative to the eye as `shadow_object_vertex` places them, and
/// its depth-only and tested vertex ranges
/// ([`crate::terrain_chunks::CasterShape`], clipped to `count`).
pub struct Caster<'a> {
    pub bind: &'a wgpu::BindGroup,
    pub buffer: &'a wgpu::Buffer,
    pub count: u32,
    pub bounds: crate::terrain_chunks::Bounds,
    pub opaque: &'a [std::ops::Range<u32>],
    pub tested: &'a [std::ops::Range<u32>],
}

impl SurfaceLighting {
    /// More casters after [`SurfaceLighting::draw`]: `cast` draws into each
    /// cascade's map in turn (keeping what is there), with the projection
    /// bound at group 1 and the cascade index given.
    pub fn draw_more(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        mut cast: impl FnMut(&mut wgpu::RenderPass<'_>, usize),
    ) {
        for (cascade, view) in self.views.iter().enumerate().take(self.settings.cascades) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Extra shadow casters pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(1, &self.caster_bind, &[]);
            cast(&mut pass, cascade);
        }
    }
}

/// A cascade projection for the culling tests in `terrain_chunks`.
#[cfg(test)]
pub(crate) fn test_projection(sun: [f32; 3], extent: f32) -> [f32; 16] {
    projection([1_000.; 3], sun, extent, 2048)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terrain_normals_join_shared_edges_across_materials() {
        let positions = [
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 0., 1.],
            [1., 0., 0.],
            [1., 1., 1.],
            [0., 0., 1.],
        ];
        let mut vertices = Vec::new();
        for (i, p) in positions.into_iter().enumerate() {
            vertices.extend([
                p[0] + 1_000_000.,
                p[1],
                p[2] + 600_000.,
                0.,
                0.,
                (i / 3) as f32,
                0.,
                0.,
                0.,
                100.,
            ]);
        }
        let normals = terrain_normals(&vertices);
        assert_eq!(&normals[3..6], &normals[9..12]);
        assert_eq!(&normals[6..9], &normals[15..18]);
        for normal in normals.chunks_exact(3) {
            assert!((normal.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 1e-6);
            assert!(normal[1] > 0.);
        }
        assert!(normals[3] < 0. && normals[4] < 1.);
    }

    #[test]
    fn distance_controls_reach_without_changing_quality_or_near_detail() {
        use crate::graphics::{ShadowDistance as D, ShadowQuality as Q};
        for quality in Q::ALL {
            let baseline = Settings::of(quality, 4);
            for distance in D::ALL {
                for trees in crate::graphics::VEGETATION_DISTANCES {
                    let nm = distance.nautical_miles(trees);
                    let settings = Settings::of(quality, nm);
                    assert_eq!(settings.map, baseline.map);
                    assert_eq!(settings.cascades, baseline.cascades);
                    assert_eq!(settings.disc_samples, baseline.disc_samples);
                    if quality == Q::Off {
                        assert_eq!(settings, baseline, "Off ignores the distance");
                        continue;
                    }
                    let reach = (f64::from(nm) * crate::vegetation::NM) as f32;
                    assert_eq!(settings.extents[settings.cascades - 1], reach);
                    assert_eq!(settings.extents[0], 256.);
                    assert_eq!(
                        settings.buildings_nm,
                        if quality == Q::Low { 0. } else { f64::from(nm) }
                    );
                    assert_eq!(
                        settings.trees_ft,
                        if matches!(quality, Q::High | Q::Ultra) {
                            reach
                        } else {
                            0.
                        }
                    );
                    if settings.cascades == 3 {
                        assert_eq!(settings.extents[1], 8192_f32.min(reach / 2.));
                    }
                    for pair in settings.extents[..settings.cascades].windows(2) {
                        assert!(pair[1] > pair[0]);
                    }
                    // The actual projection must use the chosen half-width.
                    let m = projection([0.; 3], [0., 1., 0.], reach, settings.map);
                    assert!((m[0].hypot(m[2]) * reach - 1.).abs() < 1e-6);
                }
            }
        }
        let default = Settings::of(Q::Medium, crate::graphics::DEFAULT_VEGETATION_DISTANCE);
        assert!((default.extents[2] - 24_304.462).abs() < 0.01);
        assert_eq!(default.extents[..2], [256., 8192.]);
    }

    #[test]
    fn shadow_basis_does_not_switch_axes_near_overhead_sun() {
        let direction = |y: f32| [(1. - y * y).sqrt(), y, 0.];
        let a = projection([0.; 3], direction(0.94999), 256., 2048);
        let b = projection([0.; 3], direction(0.95001), 256., 2048);
        for i in [0, 1, 2, 4, 5, 6] {
            assert!((a[i] - b[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn disc_fraction_preserves_one_and_thirty_percent_shadow_strength() {
        let radius = 0.5_f32.to_radians();
        for (height, expected) in [(-0.93433, 0.01), (-0.31969, 0.30), (0., 0.50), (1., 1.)] {
            let elevation = height * radius;
            let (_, fraction) = visible_sun([elevation.cos(), elevation.sin(), 0.], radius);
            assert!((fraction - expected).abs() < 0.00002);
        }
    }

    #[test]
    fn visible_disc_keeps_sunset_shadows_until_the_last_limb_sets() {
        let radius = 0.5_f32.to_radians();
        for (degrees, expected) in [(1., 1.), (0., 0.5), (-1., 0.)] {
            let angle = degrees * std::f32::consts::PI / 180.;
            let (direction, strength) = visible_sun([angle.cos(), angle.sin(), 0.], radius);
            assert!((strength - expected).abs() < 1e-5);
            if strength > 0. {
                assert!(direction[1] > 0.);
            }
        }
        let mut previous = 1.;
        for step in -100..=100 {
            let angle = -(step as f32) / 100. * radius;
            let (direction, strength) = visible_sun([angle.cos(), angle.sin(), 0.], radius);
            assert!(strength <= previous + 1e-5);
            assert!(previous - strength < 0.02);
            assert!(direction.iter().all(|v| v.is_finite()));
            if strength > 0. {
                assert!(direction[1] >= 0.);
            }
            previous = strength;
        }
    }

    #[test]
    fn shadow_projection_follows_light_and_stays_stable_in_large_worlds() {
        for sun in [[0., 1., 0.], unit([1., 0.1, 0.]), unit([-1., 1., 1.])] {
            for extent in Settings::of(crate::graphics::ShadowQuality::Medium, 4).extents {
                let origin = [1_000_000., 5000., 600_000.];
                let m = projection(origin, sun, extent, 2048);
                let project = |v| {
                    [
                        dot(m[0..3].try_into().unwrap(), v) + m[3],
                        dot(m[4..7].try_into().unwrap(), v) + m[7],
                        dot(m[8..11].try_into().unwrap(), v) + m[11],
                    ]
                };
                let center = project([0.; 3]);
                let toward = project(sun.map(|v| v * 100.));
                assert!((center[0] - toward[0]).abs() < 1e-5);
                assert!((center[1] - toward[1]).abs() < 1e-5);
                assert!(toward[2] < center[2], "the sunward occluder must win depth");
                assert!(center[0].abs() <= 1. / 2048. + 1e-5);
                assert!(center[1].abs() <= 1. / 2048. + 1e-5);
                assert!(m.iter().all(|v| v.is_finite()));
            }
        }
    }

    /// Shader `fire_reflectance`: mostly brightness, 30 percent of the hue.
    fn fire_reflectance(albedo: [f32; 3]) -> [f32; 3] {
        let l = 0.2126 * albedo[0] + 0.7152 * albedo[1] + 0.0722 * albedo[2];
        std::array::from_fn(|i| (l + (albedo[i] - l) * 0.3).max(0.04))
    }

    #[test]
    fn a_fire_on_green_ground_stays_warm() {
        // The orange flare colour. Area lights (a wreck fire, a fireball)
        // multiply this by fire_reflectance, not by the grass itself.
        let flare = [1.0_f32, 0.75, 0.45];
        let grass = [0.25, 0.72, 0.22];
        let raw: [f32; 3] = std::array::from_fn(|i| grass[i] * flare[i]);
        assert!(
            raw[1] > raw[0] + 0.1,
            "raw green albedo times the flare colour is a green glow: {raw:?}"
        );
        let warm: [f32; 3] = {
            let reflected = fire_reflectance(grass);
            std::array::from_fn(|i| reflected[i] * flare[i])
        };
        assert!(warm[0] > warm[1], "the fire pool is warm: {warm:?}");
        assert!(warm[1] > warm[2], "the fire pool is not blue: {warm:?}");
    }

    #[test]
    fn a_fire_on_snow_sand_and_blue_stays_warm() {
        // Same mix as green grass. Pale snow, tan sand and a blue water-like
        // albedo all come back orange, not a tinted pool of their own hue.
        let flare = [1.0_f32, 0.75, 0.45];
        for (name, albedo) in [
            ("snow", [0.90, 0.92, 0.95]),
            ("sand", [0.72, 0.55, 0.32]),
            ("blue water", [0.05, 0.12, 0.35]),
        ] {
            let warm: [f32; 3] = {
                let reflected = fire_reflectance(albedo);
                std::array::from_fn(|i| reflected[i] * flare[i])
            };
            assert!(
                warm[0] > warm[1] && warm[1] > warm[2],
                "{name} stays warm: {warm:?}"
            );
        }
        let shader = include_str!("surface_lighting.wgsl");
        assert!(shader.contains("mix(vec3<f32>(l),albedo,0.3)"));
        assert!(shader.contains("albedo*FLARE_COLOR*parts.x"));
        // Water, clouds and smoke do not take this mix. Water keeps its
        // fitted sea tint; clouds and smoke keep their own art.
        let terrain = include_str!("terrain.wgsl");
        assert!(terrain.contains("vec3<f32>(0.06,0.09,0.11)*flare_light(relative,up)"));
        assert!(terrain.contains("flares=bare*flare_light(in.direction,vec3<f32>(0.0));"));
        assert!(!terrain.contains("fire_reflectance"));
    }
}
