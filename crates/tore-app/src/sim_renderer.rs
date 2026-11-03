//! Extensible 3D pass. World data/camera are independent of wgpu; UI composites afterward.
use crate::gpu_setup::NamedSteps as _;
use crate::terrain_chunks::CasterShape;
pub use crate::vegetation_renderer::ViewKind;
use crate::{
    camera::Camera,
    scenery::{Scenery, StaticGeometry},
    terrain::Terrain,
};
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Material-local packing flag. World and weather art use sixteen independent
/// 256-square pages per physical layer, within portable texture-array limits.
pub(crate) fn tile_layout(device: &wgpu::Device, packed: bool) -> wgpu::Buffer {
    let mut value = [0u8; 16];
    value[0] = u8::from(packed);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Indexed material layout"),
        contents: &value,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn pack_world_pages(indices: &[u8]) -> Vec<u8> {
    assert!(indices.len().is_multiple_of(65536));
    let count = indices.len() / 65536;
    assert!(count <= 4096, "world artwork exceeds portable page budget");
    let mut packed = vec![255; count.div_ceil(16).max(1) * 1024 * 1024];
    for (page, bytes) in indices.chunks_exact(65536).enumerate() {
        let origin = (page / 16) * 1024 * 1024 + (page % 16 / 4) * 256 * 1024 + (page % 4) * 256;
        for y in 0..256 {
            packed[origin + y * 1024..origin + y * 1024 + 256]
                .copy_from_slice(&bytes[y * 256..y * 256 + 256]);
        }
    }
    packed
}
/// The thermal picture's reference brightness: the mean linear luminance of
/// the weather's daylight palette. A night weather darkens its art itself,
/// so the thermal shading divides by this to keep the ground's texture
/// contrast the same by night and by day (`thermal_luma_of` in terrain.wgsl).
pub(crate) fn thermal_reference(palette: &[[u8; 3]; 256]) -> f32 {
    let linear = |v: u8| {
        let c = f32::from(v) / 255.;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let sum: f32 = palette
        .iter()
        .map(|c| 0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2]))
        .sum();
    (sum / 256.).max(1e-4)
}
/// `values` as little-endian bytes, as the GPU reads them.
pub(crate) fn bytes(values: &[f32]) -> Vec<u8> {
    let mut out = vec![0; values.len() * 4];
    put_floats(&mut out, values);
    out
}
/// Writes `values` little-endian into `out`, which holds exactly four bytes
/// each. A plain loop over fixed-size pieces, which the compiler turns into
/// a straight copy on little-endian targets (no per-byte iterator).
pub(crate) fn put_floats(out: &mut [u8], values: &[f32]) {
    assert_eq!(out.len(), values.len() * 4);
    for (to, value) in out.chunks_exact_mut(4).zip(values) {
        to.copy_from_slice(&value.to_le_bytes());
    }
}
/// Uploads `values` at `offset` of `buffer` straight into the queue's
/// staging memory, without building a byte vector first.
pub(crate) fn write_floats(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    offset: u64,
    values: &[f32],
) {
    let Some(size) = wgpu::BufferSize::new(values.len() as u64 * 4) else {
        return;
    };
    match queue.write_buffer_with(buffer, offset, size) {
        Some(mut view) => put_floats(&mut view, values),
        // Validation failed (already reported by wgpu); fall back to the
        // checked path so the error matches `write_buffer`.
        None => queue.write_buffer(buffer, offset, &bytes(values)),
    }
}
#[cfg(test)]
mod upload_tests {
    /// The sky and the celestial sprites are drawn after the opaque world
    /// at depth 0, so they reach exactly the samples nothing opaque wrote
    /// (reversed Z clears to 0 and every opaque write passed `Greater` or
    /// `GreaterEqual` against it, so it is above 0).
    #[test]
    fn sky_fills_only_samples_still_at_the_clear_depth() {
        let shader = include_str!("terrain.wgsl");
        assert!(shader.contains("out.clip=vec4<f32>(p[i],0.0,1.0)"));
        assert!(shader.contains("dot(position,scene.up.xyz)*f,0.0,z)"));
        let passes = |fragment: f32, stored: f32| match super::SKY_DEPTH_COMPARE {
            wgpu::CompareFunction::GreaterEqual => fragment >= stored,
            wgpu::CompareFunction::Equal => fragment == stored,
            other => panic!("unexpected sky depth test {other:?}"),
        };
        assert!(passes(0., 0.));
        for covered in [f32::from_bits(1), 1e-9, 0.5, 1.] {
            assert!(!passes(0., covered));
        }
    }
    /// The copy helper gives exactly the bytes of the per-byte iterator it
    /// replaced, for every kind of value (signed zero, infinities, NaN
    /// payloads, subnormals) and every length.
    #[test]
    fn float_bytes_match_the_old_per_byte_copy() {
        let mut values = vec![
            0.,
            -0.,
            1.,
            -1.5,
            f32::MAX,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::from_bits(0x7fc0_1234),
            f32::from_bits(0xffa0_0001),
        ];
        let mut state = 0x1234_5678_u32;
        for _ in 0..4099 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            values.push(f32::from_bits(state));
        }
        for len in [0, 1, 2, 3, 7, 10, 30, values.len()] {
            let slice = &values[..len];
            let old: Vec<u8> = slice.iter().flat_map(|v| v.to_le_bytes()).collect();
            assert_eq!(super::bytes(slice), old);
            let mut out = vec![0xaa; len * 4];
            super::put_floats(&mut out, slice);
            assert_eq!(out, old);
        }
    }
}
/// The sky and the celestial sprites sit at depth 0 and are drawn after the
/// opaque world: they pass only where the depth is still the clear value.
const SKY_DEPTH_COMPARE: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;
/// The world pass's far plane (`world_depth_clip` in terrain.wgsl).
const WORLD_FAR_FEET: f32 = 2_200_000.;
/// The shared `Scene` uniform, ending with the graphics `quality` and
/// `viewport` vectors.
pub(crate) const UNIFORM_BYTES: u64 = 2080;
/// Rows of the live palette texture: the palette, ten fog rows, the
/// daylight palette, then `LUT_ROWS` rows of band haze colours.
const PALETTE_ROWS: u32 = 128;
const LUT_ROWS: usize = PALETTE_ROWS as usize - 12;
type AircraftBatch = (wgpu::BindGroup, wgpu::Buffer, u32);
/// One other aircraft inside a geometry batch, for the spotting aid: its
/// vertex range, presented position and airframe extent in feet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub first: u32,
    pub count: u32,
    pub center: [f32; 3],
    pub extent: f32,
}
impl Contact {
    /// The aircraft whose vertices span `first..end`; none when empty.
    pub fn new(first: usize, end: usize, center: [f64; 3], extent: f32) -> Option<Self> {
        (end > first).then(|| Self {
            first: first as u32,
            count: (end - first) as u32,
            center: center.map(|v| v as f32),
            extent,
        })
    }
}
/// Combat geometry drawn with the ownship airframe: targets when no dummy
/// models load, then debris, weapons, tracers and effects.
#[derive(Default)]
pub struct CombatGeometry {
    pub vertices: Vec<f32>,
    pub contacts: Vec<Contact>,
}
/// Spotting-aid copies per aircraft. Each contact's center, extent and first
/// vertex are repeated for its eight instance slots; the instance index picks
/// the copy.
const SPOT_INSTANCES: u64 = 8;
const SPOT_INSTANCE_FLOATS: usize = 8;
const SPOT_CONTACT_BYTES: u64 = SPOT_INSTANCES * SPOT_INSTANCE_FLOATS as u64 * 4;
fn spot_instances(contacts: &[&Contact]) -> Vec<f32> {
    contacts
        .iter()
        .flat_map(|c| {
            std::iter::repeat_n(
                [
                    c.center[0],
                    c.center[1],
                    c.center[2],
                    c.extent,
                    c.first as f32,
                    0.,
                    0.,
                    0.,
                ],
                SPOT_INSTANCES as usize,
            )
        })
        .flatten()
        .collect()
}
pub struct SimRenderer {
    lighting: crate::surface_lighting::SurfaceLighting,
    aircraft_visible: bool,
    escapees: Option<AircraftBatch>,
    lens_flare: crate::lens_flare::LensFlare,
    smoke: crate::smoke_renderer::SmokeRenderer,
    effects: crate::effect_renderer::EffectRenderer,
    /// The mission's climate, from its scenery (slice B12).
    climate: crate::blast_climate::Climate,
    countermeasures: crate::countermeasure_renderer::Instances,
    /// Experiment AP1: the redrawn airports' lights.
    airfield_lights: crate::airfield_lights::Lights,
    /// The night glow of explosions and fires (nr-fx3).
    glow_points: crate::night_glow::Points,
    battle: Option<(wgpu::Buffer, u32)>,
    battle_contacts: Vec<Contact>,
    airports: Option<(wgpu::Buffer, u32)>,
    airport_lines: Option<(wgpu::Buffer, u32)>,
    /// The immutable batch currently in both static buffers. Fresh renderers
    /// have no batch, even if their scenery already has cached geometry.
    airport_geometry: Option<Arc<StaticGeometry>>,
    /// The static ground objects' shadow casters, in chunks
    /// ([`crate::terrain_chunks::placements`]): a copy of the static batch
    /// without the triangles that cannot cast.
    static_casters: Option<(wgpu::Buffer, crate::terrain_chunks::Chunks)>,
    /// The surface units drawn every frame, relative to the render origin.
    surface_units: Option<(wgpu::Buffer, u32)>,
    /// Per-model formation batches, with each aircraft's vertex range.
    dummies: Vec<(
        tore_formats::aircraft::AircraftId,
        AircraftBatch,
        Vec<Contact>,
        CasterShape,
    )>,
    spot_instances: wgpu::Buffer,
    vapor: Option<(wgpu::Buffer, u32)>,
    p: Pipelines,
    shader: wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    material_layout: wgpu::BindGroupLayout,
    vapor_bind: wgpu::BindGroup,
    palette: wgpu::Texture,
    /// The palette rows last uploaded. Views of one weather share them, so
    /// an unchanged palette is not uploaded again.
    palette_uploaded: Vec<u8>,
    weather_tiles: wgpu::TextureView,
    seam_records: wgpu::TextureView,
    seam_tints: wgpu::TextureView,
    seam_sampler: wgpu::Sampler,
    canopy_visible: bool,
    aircraft: Option<(wgpu::BindGroup, wgpu::Buffer, u32)>,
    bind: wgpu::BindGroup,
    celestial_vertices: wgpu::Buffer,
    cloud_vertices: wgpu::Buffer,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    terrain_normals: wgpu::Buffer,
    /// The terrain buffer's spatial chunks (optimization S13a).
    chunks: crate::terrain_chunks::Chunks,
    /// This view's visible terrain chunks, nearest first (reused).
    chunk_order: Vec<std::ops::Range<u32>>,
    /// Vertex boxes of the shadow-casting object batches, in the space each
    /// batch's vertices are uploaded in.
    bounds: CasterBounds,
    targets: Vec<Targets>,
    resample: wgpu::RenderPipeline,
    options: crate::graphics::Options,
    samples: u32,
    /// CF0 city fill buildings, when the scenery has them.
    pub fill: Option<crate::fill_renderer::FillRenderer>,
    /// Exploratory vegetation, when the scenery has an atlas.
    pub vegetation: Option<crate::vegetation_renderer::Renderer>,
    /// The fires of the fill's knocked-down buildings
    /// ([`crate::city_fires`]).
    city_fires: crate::city_fires::CityFires,
    /// A replay's clock for the burning city (nr-fx4): columns age on the
    /// playhead's ticks, not the wall clock. `None` in live flight.
    city_clock: Option<crate::city_fires::Clock>,
    /// The burning city as the last draw had it, in numbers (nr-fx4).
    city_report: crate::city_fires::Report,
    /// The main view's last burning-city frame and wall clock, which the
    /// mirror shows (optimization S10).
    city_main: Option<(crate::city_fires::Frame, f64)>,
    /// Whether they are drawn (`TORE_NO_CITY_FIRES=1` turns them off, a
    /// developer aid like `TORE_CITY_FILL=0`; the stress measurement flips
    /// it).
    city_fires_on: bool,
}
/// The vertex boxes of the object batches, kept beside their buffers so a
/// shadow cascade can skip a batch wholly outside it.
/// Each defaults to never culled and every triangle tested, until an
/// upload measures the batch.
#[derive(Clone, Default)]
struct CasterBounds {
    aircraft: CasterShape,
    battle: CasterShape,
    surface_units: CasterShape,
}
/// Whether a terrain triangle on texture `layer` always casts its whole
/// shadow: untextured terrain, or a page with no cut-out (255) texel, so
/// `terrain_shadow_fragment` could never discard. Anything else, including a
/// layer past the ground pages, keeps the cut-out test.
fn opaque_caster_layers(texture_indices: &[u8]) -> impl Fn(f32) -> bool + '_ {
    let pages: Vec<bool> = texture_indices
        .chunks_exact(65536)
        .map(|page| !page.contains(&255))
        .collect();
    move |layer| layer < 0. || pages.get(layer as usize).copied().unwrap_or(false)
}
/// World-pass attachments for one output size. The main view, mirrors and
/// instrument previews alternate sizes within a frame, so a few are cached.
struct Targets {
    output: [u32; 2],
    size: [u32; 2],
    samples: u32,
    depth: wgpu::TextureView,
    /// The world depth as the spotting aid reads it.
    rim_depth: wgpu::BindGroup,
    /// Multisampled color, resolved at the end of the world pass.
    color: Option<wgpu::TextureView>,
    /// The render-scale image and its resample bindings, when the world is
    /// drawn at a different size than the output.
    scaled: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}
/// Every pipeline that draws into the world pass. They are rebuilt together
/// when the anti-aliasing sample count changes.
struct Pipelines {
    vapor_pipeline: wgpu::RenderPipeline,
    tracer_pipeline: wgpu::RenderPipeline,
    pipeline: wgpu::RenderPipeline,
    airport_pipeline: wgpu::RenderPipeline,
    scenery_line_pipeline: wgpu::RenderPipeline,
    airport_decal_pipeline: wgpu::RenderPipeline,
    moving_pipeline: wgpu::RenderPipeline,
    moving_decal_pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    canopy_depth_pipeline: wgpu::RenderPipeline,
    canopy_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    celestial_pipeline: wgpu::RenderPipeline,
    cloud_pipeline: wgpu::RenderPipeline,
    /// The spotting-aid pass: single-sample, after the world image resolves,
    /// reading the world depth through `rim_depth_layout`.
    rim_dark_pipeline: wgpu::RenderPipeline,
    rim_light_pipeline: wgpu::RenderPipeline,
    rim_depth_layout: wgpu::BindGroupLayout,
    countermeasures: crate::countermeasure_renderer::Pipelines,
    airfield_lights: crate::airfield_lights::Pipeline,
    night_glow: crate::night_glow::Pipeline,
}
impl Pipelines {
    fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material_layout: &wgpu::BindGroupLayout,
        lighting_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let multisample = wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        };
        let surface_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shared world surface layout"),
            bind_group_layouts: &[material_layout, lighting_layout],
            push_constant_ranges: &[],
        });
        let mut surface_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("Simulation terrain"),
            layout: Some(&surface_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 40,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Greater,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample,
            multiview: None,
            cache: None,
        };
        // Aircraft, pilots, weapons, debris and tracers are built relative to
        // the render origin; static airports and terrain stay in world space.
        surface_descriptor.label = Some("Moving objects");
        surface_descriptor.vertex.entry_point = Some("object_vertex");
        let pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.vertex.entry_point = Some("vertex");
        surface_descriptor.label = Some("Static airport surfaces");
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("airport_solid_fragment");
        // Rendered terrain is recessed below the fixed airport plane. Neither
        // slope nor constant depth bias may pull pavement over aircraft, so
        // the solid pass uses none; the texture detail pass below is the one
        // exception. Later coplanar artwork wins equal-depth samples without
        // a world lift.
        surface_descriptor
            .depth_stencil
            .as_mut()
            .unwrap()
            .depth_compare = wgpu::CompareFunction::GreaterEqual;
        let airport_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.label = Some("Static airport coplanar texture details");
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("airport_fragment");
        // One reversed-depth ULP resolves texture-on-pavement ties. This is
        // the only surface pass with a bias: no slope bias, and no geometry or
        // contact lift.
        surface_descriptor
            .depth_stencil
            .as_mut()
            .unwrap()
            .bias
            .constant = 1;
        let airport_decal_pipeline = device.named_render_pipeline(&surface_descriptor);
        // The surface units drawn every frame (columns, men, deck crew,
        // parked aircraft pieces) use the static art and its two passes, but
        // are built relative to the render origin like the aircraft.
        surface_descriptor.vertex.entry_point = Some("object_vertex");
        surface_descriptor.label = Some("Moving scenery coplanar texture details");
        let moving_decal_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.label = Some("Moving scenery surfaces");
        surface_descriptor.depth_stencil.as_mut().unwrap().bias = Default::default();
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("airport_solid_fragment");
        let moving_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.vertex.entry_point = Some("vertex");
        surface_descriptor
            .depth_stencil
            .as_mut()
            .unwrap()
            .depth_compare = wgpu::CompareFunction::Greater;
        surface_descriptor.depth_stencil.as_mut().unwrap().bias = Default::default();
        surface_descriptor.primitive.topology = wgpu::PrimitiveTopology::LineList;
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("scenery_line_fragment");
        let scenery_line_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.primitive.topology = wgpu::PrimitiveTopology::TriangleList;
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("fragment");
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sky layout"),
            bind_group_layouts: &[material_layout, lighting_layout],
            push_constant_ranges: &[],
        });
        surface_descriptor.label = Some("Shoreline terrain");
        surface_descriptor.layout = Some(&sky_layout);
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("terrain_fragment");
        let surface_buffers = surface_descriptor.vertex.buffers;
        let terrain_buffers = [
            surface_buffers[0].clone(),
            wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![5=>Float32x3],
            },
        ];
        surface_descriptor.vertex.entry_point = Some("terrain_vertex");
        surface_descriptor.vertex.buffers = &terrain_buffers;
        let terrain_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.vertex.entry_point = Some("object_vertex");
        surface_descriptor.vertex.buffers = surface_buffers;
        // Share the aircraft material bindings. First select the nearest glass
        // without changing color, then blend that surface once over opaque art.
        surface_descriptor.label = Some("Canopy nearest depth");
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("canopy_fragment");
        let depth_target = [Some(wgpu::ColorTargetState {
            format,
            blend: None,
            write_mask: wgpu::ColorWrites::empty(),
        })];
        surface_descriptor.fragment.as_mut().unwrap().targets = &depth_target;
        let canopy_depth_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.label = Some("Canopy transparency");
        let glass_target = [Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        surface_descriptor.fragment.as_mut().unwrap().targets = &glass_target;
        let depth = surface_descriptor.depth_stencil.as_mut().unwrap();
        depth.depth_write_enabled = false;
        depth.depth_compare = wgpu::CompareFunction::Equal;
        let canopy_pipeline = device.named_render_pipeline(&surface_descriptor);
        surface_descriptor.label = Some("Additive luminous gun tracers");
        surface_descriptor.fragment.as_mut().unwrap().entry_point = Some("tracer_fragment");
        let tracer_target = [Some(wgpu::ColorTargetState {
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
        })];
        surface_descriptor.fragment.as_mut().unwrap().targets = &tracer_target;
        surface_descriptor
            .depth_stencil
            .as_mut()
            .unwrap()
            .depth_compare = wgpu::CompareFunction::Greater;
        let tracer_pipeline = device.named_render_pipeline(&surface_descriptor);
        // Spotting aid: aircraft vertices plus per-aircraft instance data,
        // drawn at output resolution without multisampling so every rim pixel
        // is whole. Occlusion comes from reading the world depth.
        let spot_buffers = [
            surface_buffers[0].clone(),
            wgpu::VertexBufferLayout {
                array_stride: SPOT_INSTANCE_FLOATS as u64 * 4,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![10=>Float32x4, 11=>Float32x4],
            },
        ];
        let rim_depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spotting aid world depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: u32::from(samples > 1),
                // Flare glare tests its core against depth per vertex, and
                // the lens flare measures the sun's disc against it.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: samples > 1,
                },
                count: None,
            }],
        });
        let rim_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Spotting aid layout"),
            bind_group_layouts: &[material_layout, lighting_layout, &rim_depth_layout],
            push_constant_ranges: &[],
        });
        // Min for a dark rim and max for a light one: overlapping copies are
        // idempotent, and the rim never lightens (or darkens) the background.
        let rim = |label, fragment: &str, operation| {
            let fragment = format!("{fragment}{}", if samples > 1 { "_ms" } else { "" });
            device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&rim_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("rim_vertex"),
                    compilation_options: Default::default(),
                    buffers: &spot_buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(&fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let rim_dark_pipeline = rim(
            "Spotting aid dark rim",
            "rim_dark_fragment",
            wgpu::BlendOperation::Min,
        );
        let rim_light_pipeline = rim(
            "Spotting aid light rim",
            "rim_light_fragment",
            wgpu::BlendOperation::Max,
        );
        let countermeasures = crate::countermeasure_renderer::Pipelines::new(
            device,
            shader,
            format,
            samples,
            &surface_layout,
            &rim_layout,
        );
        let airfield_lights =
            crate::airfield_lights::Pipeline::new(device, shader, format, samples, &surface_layout);
        let night_glow =
            crate::night_glow::Pipeline::new(device, shader, format, samples, &surface_layout);
        let sky_pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Retail sky preview"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("sky_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("sky_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            // Drawn after the opaque world at depth 0, so it fills only the
            // samples still at the clear depth (reversed Z clears to 0).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: SKY_DEPTH_COMPARE,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample,
            multiview: None,
            cache: None,
        });
        let celestial_pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Original celestial primitives"), layout: Some(&sky_layout),
            vertex: wgpu::VertexState { module:shader,entry_point:Some("celestial_vertex"),compilation_options:Default::default(),
                buffers:&[wgpu::VertexBufferLayout {array_stride:40,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32]}]},
            fragment:Some(wgpu::FragmentState {module:shader,entry_point:Some("celestial_fragment"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {format,blend:Some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:Default::default(),depth_stencil:Some(wgpu::DepthStencilState {format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:false,depth_compare:SKY_DEPTH_COMPARE,stencil:Default::default(),bias:Default::default()}),multisample,multiview:None,cache:None,
        });
        let cloud_pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:Some("Original cloud sheets"),layout:Some(&sky_layout),
            vertex:wgpu::VertexState {module:shader,entry_point:Some("vertex"),compilation_options:Default::default(),buffers:&[wgpu::VertexBufferLayout {array_stride:40,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32]}]},
            fragment:Some(wgpu::FragmentState {module:shader,entry_point:Some("cloud_fragment"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:wgpu::PrimitiveState {cull_mode:None,..Default::default()},depth_stencil:Some(wgpu::DepthStencilState {format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:true,depth_compare:wgpu::CompareFunction::Greater,stencil:Default::default(),bias:Default::default()}),multisample,multiview:None,cache:None,
        });
        // Wing vapor is five one-pixel line segments per side, exactly as
        // `_DrawStreamer@12` draws them, so it needs its own blended pipeline.
        let vapor_pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wing vapor"),
            layout: None,
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vapor_vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 28,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("vapor_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
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
            multisample,
            multiview: None,
            cache: None,
        });
        Self {
            vapor_pipeline,
            tracer_pipeline,
            pipeline,
            airport_pipeline,
            scenery_line_pipeline,
            airport_decal_pipeline,
            moving_pipeline,
            moving_decal_pipeline,
            terrain_pipeline,
            canopy_depth_pipeline,
            canopy_pipeline,
            sky_pipeline,
            celestial_pipeline,
            cloud_pipeline,
            rim_dark_pipeline,
            rim_light_pipeline,
            rim_depth_layout,
            countermeasures,
            airfield_lights,
            night_glow,
        }
    }
}
impl SimRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        scenery: &Scenery,
        options: crate::graphics::Options,
        samples: u32,
    ) -> Self {
        crate::vegetation_limits::validate(scenery, &device.limits())
            .expect("scene resources must fit device limits before upload");
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Simulation terrain"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                    include_str!("surface_lighting.wgsl"),
                    include_str!("terrain.wgsl"),
                    include_str!("spotting.wgsl"),
                    include_str!("countermeasures.wgsl"),
                    include_str!("gun_flash.wgsl"),
                    include_str!("airfield_lights.wgsl"),
                    include_str!("night_glow.wgsl"),
                    include_str!("grass_blades.wgsl")
                )
                .into(),
            ),
        });
        // Compute sees the material too: the blade grass candidate pass.
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("World surface material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        .union(wgpu::ShaderStages::COMPUTE),
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let lighting = crate::surface_lighting::SurfaceLighting::new(
            device,
            &material_layout,
            &shader,
            crate::surface_lighting::Settings::from_options(&options),
        );
        let pipelines = Pipelines::new(
            device,
            &shader,
            format,
            samples,
            &material_layout,
            &lighting.layout,
        );
        let fill = scenery.fill.as_ref().map(|art| {
            crate::fill_renderer::FillRenderer::new(
                device,
                &shader,
                format,
                samples,
                &material_layout,
                &lighting,
                art,
            )
        });
        let mut fill = fill;
        if let Some(fill) = &mut fill {
            fill.set_options(&options);
            fill.set_shadows(&lighting.settings());
        }
        let vegetation = scenery
            .vegetation
            .as_ref()
            .zip(scenery.vegetation_meshes.as_ref())
            .map(|(atlas, library)| {
                let mut vegetation = crate::vegetation_renderer::Renderer::new(
                    device,
                    &shader,
                    format,
                    samples,
                    &material_layout,
                    &lighting,
                    Arc::clone(atlas),
                    Arc::clone(library),
                    queue,
                    &scenery.grass,
                );
                vegetation.set_options(device, &options);
                vegetation.set_shadows(&lighting.settings());
                vegetation
            });
        let celestial_vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Celestial vertices"),
            size: 256 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cloud_vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloud vertices"),
            size: 256 * 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera and atmosphere"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = pack_world_pages(
            &[
                scenery.texture_indices.as_slice(),
                scenery.sky_indices.as_slice(),
            ]
            .concat(),
        );
        let size = wgpu::Extent3d {
            width: 1024,
            height: 1024,
            depth_or_array_layers: (indices.len() / (1024 * 1024)) as u32,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Original world artwork pages"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &indices,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1024),
                rows_per_image: Some(1024),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let palette = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Live weather palette"),
            // Weather palette, ten fog rows, the daylight palette that
            // flare light shows at night (BASE_ROW in terrain.wgsl), then
            // the bands' haze colours (LUT_ROW).
            size: wgpu::Extent3d {
                width: 256,
                height: PALETTE_ROWS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette_view = palette.create_view(&wgpu::TextureViewDescriptor::default());
        let empty_seams = crate::terrain_seams::Map::default();
        let seams = scenery
            .vegetation
            .as_ref()
            .filter(|a| a.seams.has_effects())
            .map_or(&empty_seams, |a| &a.seams);
        let record_size = seams.gpu_size();
        let records = if seams.baked.records.is_empty() {
            &[0_u8; 8][..]
        } else {
            &seams.baked.records
        };
        let seam_size = wgpu::Extent3d {
            width: record_size[0],
            height: record_size[1],
            depth_or_array_layers: 1,
        };
        let seam_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Packed terrain seam records"),
            size: seam_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            seam_texture.as_image_copy(),
            records,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(record_size[0] * 8),
                rows_per_image: None,
            },
            seam_size,
        );
        let seam_records = seam_texture.create_view(&Default::default());
        let neutral = crate::terrain_seams::baked::Baked::build(
            &crate::terrain_seams::Map::default(),
            &[],
            &[[0; 3]; 256],
        );
        let baked = scenery
            .vegetation
            .as_ref()
            .filter(|a| !a.seams.baked.pixels.is_empty())
            .map_or(&neutral, |a| &a.seams.baked);
        let seam_tints_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cached seam tints"),
            size: wgpu::Extent3d {
                width: baked.size[0],
                height: baked.size[1],
                depth_or_array_layers: baked.size[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            seam_tints_texture.as_image_copy(),
            &baked.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(baked.size[0] * 8),
                rows_per_image: Some(baked.size[1]),
            },
            seam_tints_texture.size(),
        );
        let seam_tints = seam_tints_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let seam_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Seam tint interpolation"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let material_storage = tile_layout(device, true);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Simulation scene bindings"),
            layout: &pipelines.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&palette_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&palette_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: material_storage.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&seam_records),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&seam_tints),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&seam_sampler),
                },
            ],
        });
        // Vapor reads this view's camera and palette, with its own derived layout.
        let vapor_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Wing vapor bindings"),
            layout: &pipelines.vapor_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&palette_view),
                },
            ],
        });
        // Normals from the source order (their sums are order-dependent),
        // then both streams regrouped into chunks.
        let (terrain, normals, chunks) = crate::terrain_chunks::build(
            &scenery.vertices,
            &crate::surface_lighting::terrain_normals(&scenery.vertices),
            opaque_caster_layers(&scenery.texture_indices),
        );
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Retail T2 terrain mesh"),
            contents: &bytes(&terrain),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let terrain_normals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shared terrain lighting normals"),
            contents: &bytes(&normals),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            chunks,
            chunk_order: Vec::new(),
            bounds: CasterBounds::default(),
            terrain_normals,
            lighting,
            escapees: None,
            aircraft_visible: true,
            lens_flare: crate::lens_flare::LensFlare::new(
                device,
                format,
                &pipelines.rim_depth_layout,
                samples,
            ),
            smoke: crate::smoke_renderer::SmokeRenderer::new(device, format, &shader, samples),
            effects: crate::effect_renderer::EffectRenderer::new(device, format, &shader, samples),
            climate: scenery.climate,
            countermeasures: crate::countermeasure_renderer::Instances::new(device),
            airfield_lights: crate::airfield_lights::Lights::new(),
            glow_points: crate::night_glow::Points::new(device),
            battle: None,
            battle_contacts: Vec::new(),
            airports: None,
            airport_lines: None,
            airport_geometry: None,
            static_casters: None,
            surface_units: None,
            vapor: None,
            p: pipelines,
            shader,
            format,
            material_layout,
            vapor_bind,
            palette,
            palette_uploaded: Vec::new(),
            weather_tiles: view,
            seam_records,
            seam_tints,
            seam_sampler,
            canopy_visible: false,
            aircraft: None,
            dummies: Vec::new(),
            spot_instances: Self::spot_buffer(device, 16),
            celestial_vertices,
            cloud_vertices,
            bind,
            uniform,
            vertices,
            targets: Vec::new(),
            resample: Self::resample_pipeline(device, format),
            options,
            samples,
            fill,
            vegetation,
            city_fires: crate::city_fires::CityFires::new(),
            city_clock: None,
            city_report: crate::city_fires::Report::default(),
            city_main: None,
            city_fires_on: std::env::var_os("TORE_NO_CITY_FIRES").is_none(),
        }
    }
    /// Draws (or stops drawing) the burning city fill buildings.
    pub fn set_city_fires(&mut self, on: bool) {
        self.city_fires_on = on;
    }
    /// The burning city the last draw showed, in numbers (nr-fx4).
    pub fn city_report(&self) -> crate::city_fires::Report {
        self.city_report
    }
    /// A replay's clock for the burning city: the playhead and the tick
    /// each building fell on. Their smoke columns then stand at the height
    /// they had at the playhead however it got there (nr-fx4).
    pub fn set_city_clock(&mut self, clock: Option<crate::city_fires::Clock>) {
        self.city_clock = clock;
    }
    pub fn smoke(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        art: &tore_formats::Pic,
        smoke: [&tore_sim::combat::smoke::Smoke; 2],
        devices: &tore_sim::combat::countermeasures::Devices,
    ) {
        self.smoke.prepare(
            device,
            queue,
            &self.uniform,
            (&self.palette, &self.weather_tiles),
            art,
            smoke,
        );
        self.smoke.flare_smoke(devices);
    }
    /// This frame's explosions, craters and crash-site fires.
    pub fn effects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        art: &crate::effect_renderer::Art,
        effects: &[crate::snapshot::EffectPose],
        marks: &[crate::snapshot::MarkPose],
    ) {
        self.effects.prepare(
            device,
            queue,
            &self.uniform,
            (&self.palette, &self.weather_tiles),
            art,
            effects,
            marks,
            self.climate,
        );
    }
    /// This frame's light and glare sources: burning flares, chaff, lit
    /// afterburners and AC-130 gunfire, whose blast smoke joins the smoke.
    /// Call before drawing, after `smoke`.
    pub fn emitters(
        &mut self,
        queue: &wgpu::Queue,
        devices: &tore_sim::combat::countermeasures::Devices,
        afterburners: &[crate::countermeasure_renderer::Afterburner],
        guns: &crate::gun_flash::Drawn,
    ) {
        self.countermeasures
            .upload(queue, devices, afterburners, guns);
        self.smoke.gun_smoke(&guns.puffs);
        // The blasts' vapor and lingering dust, by the mission's climate.
        self.smoke
            .gun_smoke(&crate::blast_climate::puffs(&guns.raised, self.climate));
        self.effects.fit_fires(&guns.fires);
    }
    pub fn combat(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        geometry: &CombatGeometry,
    ) {
        let vertices = geometry.vertices.as_slice();
        if self.battle.is_none() {
            self.battle = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Bounded combat geometry"),
                    size: 8 * 1024 * 1024,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                0,
            ));
        }
        if let Some((buffer, count)) = &mut self.battle {
            let length = vertices.len().min((8 * 1024 * 1024 / 4 / 30) * 30);
            if length > 0 {
                write_floats(queue, buffer, 0, &vertices[..length]);
            }
            *count = (length / 10) as u32;
            self.bounds.battle = CasterShape::of(&vertices[..length]);
            // Aircraft cut off by the buffer budget are not drawn at all.
            let drawn = *count;
            self.battle_contacts.clear();
            self.battle_contacts.extend(
                geometry
                    .contacts
                    .iter()
                    .filter(|c| c.first + c.count <= drawn),
            );
        }
    }
    pub fn airports(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        geometry: &Arc<StaticGeometry>,
    ) {
        if self
            .airport_geometry
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, geometry))
        {
            return;
        }
        self.airport_vertices(device, queue, &geometry.vertices);
        self.airport_lines(device, queue, &geometry.lines);
        let (casters, chunks) =
            crate::terrain_chunks::placements(&geometry.vertices, &geometry.placements);
        self.static_casters = (!casters.is_empty()).then(|| {
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Static ground object shadow casters"),
                    contents: &crate::vegetation_renderer::bytes_of_f32(&casters),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                chunks,
            )
        });
        self.airport_geometry = Some(Arc::clone(geometry));
    }
    /// This frame's moving surface geometry
    /// ([`crate::scenery::Scenery::surface_vertices`]): drawn with the static
    /// art, relative to the render origin.
    pub fn surface_units(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[f32]) {
        let needed = (vertices.len() * 4).max(40) as u64;
        if self
            .surface_units
            .as_ref()
            .is_none_or(|(buffer, _)| buffer.size() < needed)
        {
            self.surface_units = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Moving surface units"),
                    size: needed.next_power_of_two().max(64 * 1024),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                0,
            ));
        }
        let (buffer, count) = self.surface_units.as_mut().unwrap();
        if !vertices.is_empty() {
            write_floats(queue, buffer, 0, vertices);
        }
        *count = (vertices.len() / 10) as u32;
        self.bounds.surface_units = CasterShape::of(vertices);
    }
    fn airport_vertices(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[f32]) {
        if self.airports.is_none() {
            self.airports = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Imported static airport geometry"),
                    size: 32 * 1024 * 1024,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                0,
            ));
        }
        if let Some((buffer, count)) = &mut self.airports {
            assert!(
                vertices.len() * 4 <= 32 * 1024 * 1024,
                "validated static scene exceeds GPU budget"
            );
            let length = vertices.len();
            if length > 0 {
                write_floats(queue, buffer, 0, &vertices[..length]);
            }
            *count = (length / 10) as u32;
        }
    }
    fn airport_lines(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[f32]) {
        let needed = (vertices.len() * 4).max(40) as u64;
        if self
            .airport_lines
            .as_ref()
            .is_none_or(|(buffer, _)| buffer.size() < needed)
        {
            self.airport_lines = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Source scenery line geometry"),
                    size: needed,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                0,
            ));
        }
        let (buffer, count) = self.airport_lines.as_mut().unwrap();
        if !vertices.is_empty() {
            write_floats(queue, buffer, 0, vertices);
        }
        *count = (vertices.len() / 10) as u32;
    }
    /// Seven floats per vertex: position then premultiplied-free RGBA.
    pub fn vapor(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[f32]) {
        if self.vapor.is_none() {
            self.vapor = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Wing vapor"),
                    size: 64 * 1024,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                0,
            ));
        }
        if let Some((buffer, count)) = &mut self.vapor {
            let length = vertices.len().min((64 * 1024 / 4 / 14) * 14);
            if length > 0 {
                write_floats(queue, buffer, 0, &vertices[..length]);
            }
            *count = (length / 7) as u32;
        }
    }

    pub fn dummies(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        geometry: Vec<(&crate::aircraft::Airframe, Vec<f32>, Vec<Contact>)>,
    ) {
        let ownship = self.aircraft.take();
        let ownship_bounds = std::mem::take(&mut self.bounds.aircraft);
        let visible = self.aircraft_visible;
        let canopy = self.canopy_visible;
        let mut old = std::mem::take(&mut self.dummies);
        for (model, vertices, contacts) in geometry {
            self.aircraft = old
                .iter()
                .position(|(id, _, _, _)| *id == model.profile.id)
                .map(|i| old.swap_remove(i).1);
            self.aircraft(device, queue, model, &vertices);
            self.dummies.push((
                model.profile.id,
                self.aircraft.take().unwrap(),
                contacts,
                std::mem::take(&mut self.bounds.aircraft),
            ));
        }
        self.aircraft = ownship;
        self.bounds.aircraft = ownship_bounds;
        self.aircraft_visible = visible;
        self.canopy_visible = canopy;
    }
    pub fn clear_aircraft(&mut self) {
        self.escapees = None;
        self.aircraft = None;
        self.canopy_visible = false;
    }
    pub fn aircraft(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        hornet: &crate::aircraft::Airframe,
        vertices: &[f32],
    ) {
        self.textured_model(
            device,
            queue,
            &hornet.atlas,
            hornet.engine_material.as_ref(),
            vertices,
        );
    }
    pub fn escapees(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        art: &crate::ejection_art::Art,
        vertices: &[f32],
    ) {
        let ownship = self.aircraft.take();
        let ownship_bounds = std::mem::take(&mut self.bounds.aircraft);
        let visible = self.aircraft_visible;
        let canopy = self.canopy_visible;
        self.aircraft = self.escapees.take();
        self.textured_model(device, queue, &art.atlas, None, vertices);
        self.escapees = self.aircraft.take();
        self.aircraft = ownship;
        self.bounds.aircraft = ownship_bounds;
        self.aircraft_visible = visible;
        self.canopy_visible = canopy;
    }
    fn textured_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pic: &tore_formats::Pic,
        engine: Option<&crate::engine_material::Image>,
        vertices: &[f32],
    ) {
        self.aircraft_visible = true;
        if self.aircraft.is_none() {
            // The aircraft atlas takes the same index-plus-palette path as the
            // terrain, with its own airframe palette rather than the weather one.
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Retail aircraft atlas"),
                size: wgpu::Extent3d {
                    width: pic.width as u32,
                    height: pic.height as u32,
                    depth_or_array_layers: 2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            // write_texture needs a 256-byte row pitch; pad each source row.
            let pitch = pic.width.div_ceil(256) * 256;
            let mut indices = Vec::with_capacity(pitch * pic.height * 2);
            for row in 0..pic.height {
                for column in 0..pitch {
                    let at = row * pic.width + column;
                    indices.push(match pic.pixels.get(at) {
                        Some(index) if column < pic.width && pic.mask[at] => *index,
                        _ => 255,
                    });
                }
            }
            indices.extend_from_within(..);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &indices,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch as u32),
                    rows_per_image: Some(pic.height as u32),
                },
                wgpu::Extent3d {
                    width: pic.width as u32,
                    height: pic.height as u32,
                    depth_or_array_layers: 2,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            // Both reviewed exterior atlases have no embedded palette. Native
            // Remap and the palette worker share the world's indexed palette.
            assert!(pic.palette.is_empty(), "unreviewed aircraft atlas palette");
            let palette_view = self.palette.create_view(&Default::default());
            let engine_view = engine.map(|image| image.upload(device, queue));
            let material_storage = tile_layout(device, false);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Aircraft textures"),
                layout: &self.p.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&palette_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&self.weather_tiles),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            engine_view.as_ref().unwrap_or(&palette_view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: material_storage.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&self.seam_records),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&self.seam_tints),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&self.seam_sampler),
                    },
                ],
            });
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Aircraft pose"),
                size: 2 * 1024 * 1024,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.aircraft = Some((bind, buffer, 0));
        }
        if let Some((_, buffer, count)) = &mut self.aircraft {
            if vertices.len() as u64 * 4 > buffer.size() {
                *buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Aircraft formation poses"),
                    size: (vertices.len() as u64 * 4).next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !vertices.is_empty() {
                write_floats(queue, buffer, 0, vertices);
            }
            *count = (vertices.len() / 10) as u32;
            self.canopy_visible = vertices.chunks_exact(10).any(|v| v[5] == -5.);
            self.bounds.aircraft = CasterShape::of(vertices);
        }
    }
    pub fn update_aircraft_vertices(&mut self, queue: &wgpu::Queue, vertices: &[f32]) {
        self.aircraft_visible = true;
        assert!(vertices.len() * 4 <= 2 * 1024 * 1024);
        if let Some((_, buffer, count)) = &mut self.aircraft {
            if !vertices.is_empty() {
                write_floats(queue, buffer, 0, vertices);
            }
            *count = (vertices.len() / 10) as u32;
            self.canopy_visible = vertices.chunks_exact(10).any(|v| v[5] == -5.);
            self.bounds.aircraft = CasterShape::of(vertices);
        }
    }
    /// The next view of each camera takes the sun's measured visibility at
    /// once instead of easing toward it (a still capture).
    pub fn snap_sun_visibility(&mut self) {
        self.lens_flare.snap();
    }
    pub fn hide_aircraft(&mut self) {
        self.canopy_visible = false;
        self.aircraft_visible = false;
    }
    /// Apply new graphics choices. A different sample count rebuilds every
    /// world pipeline; the caller clamps it to what the adapter supports.
    pub fn set_graphics(
        &mut self,
        device: &wgpu::Device,
        options: crate::graphics::Options,
        samples: u32,
    ) {
        self.options = options;
        let shadows = crate::surface_lighting::Settings::from_options(&options);
        self.lighting.set_settings(device, shadows);
        if let Some(fill) = &mut self.fill {
            fill.set_options(&options);
            fill.set_shadows(&shadows);
        }
        if let Some(vegetation) = &mut self.vegetation {
            vegetation.set_options(device, &options);
            vegetation.set_shadows(&shadows);
        }
        if samples == self.samples {
            return;
        }
        self.samples = samples;
        self.p = Pipelines::new(
            device,
            &self.shader,
            self.format,
            samples,
            &self.material_layout,
            &self.lighting.layout,
        );
        if let Some(vegetation) = &mut self.vegetation {
            vegetation.rebuild(
                device,
                &self.shader,
                self.format,
                samples,
                &self.material_layout,
                &self.lighting,
            );
        }
        if let Some(fill) = &mut self.fill {
            fill.rebuild(
                device,
                &self.shader,
                self.format,
                samples,
                &self.material_layout,
                &self.lighting,
            );
        }
        self.vapor_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Wing vapor bindings"),
            layout: &self.p.vapor_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &self.palette.create_view(&Default::default()),
                    ),
                },
            ],
        });
        self.lens_flare
            .set_samples(device, &self.p.rim_depth_layout, samples);
        self.smoke
            .set_samples(device, self.format, &self.shader, samples);
        self.effects
            .set_samples(device, self.format, &self.shader, samples);
        self.targets.clear();
    }
    fn spot_buffer(device: &wgpu::Device, contacts: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spotting aid instances"),
            size: contacts * SPOT_CONTACT_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    fn resample_pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render scale resample"),
            source: wgpu::ShaderSource::Wgsl(include_str!("resample.wgsl").into()),
        });
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render scale resample"),
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
        })
    }
    /// The cached attachments for an output size, created on first use.
    fn targets(&mut self, device: &wgpu::Device, output: [u32; 2]) -> usize {
        let size = self.options.scaled_size(output);
        if let Some(i) = self
            .targets
            .iter()
            .position(|t| t.output == output && t.size == size && t.samples == self.samples)
        {
            return i;
        }
        let texture = |label, samples, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let scaled = (size != output).then(|| {
            let view = texture(
                "Render scale image",
                1,
                self.format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Render scale resample"),
                layout: &self.resample.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            (view, bind)
        });
        let depth = texture(
            "Simulation depth",
            self.samples,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let rim_depth = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Spotting aid world depth"),
            layout: &self.p.rim_depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: u32::from(self.samples > 1),
                resource: wgpu::BindingResource::TextureView(&depth),
            }],
        });
        let targets = Targets {
            output,
            size,
            samples: self.samples,
            depth,
            rim_depth,
            color: (self.samples > 1).then(|| {
                texture(
                    "Multisampled world",
                    self.samples,
                    self.format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT,
                )
            }),
            scaled,
        };
        // Main view, mirrors and up to three instrument previews.
        if self.targets.len() >= 5 {
            self.targets.remove(0);
        }
        self.targets.push(targets);
        self.targets.len() - 1
    }
    /// Draws the main view ([`ViewKind::Main`]).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) {
        self.draw_view(
            device,
            queue,
            encoder,
            ViewKind::Main,
            target,
            size,
            camera,
            world,
            scenery,
        );
    }
    /// Draws one camera's view. Only the main view moves the vegetation
    /// stream and the burning city's frame; the mirror reuses both, and a
    /// camera panel far from the main eye streams its own vegetation
    /// ([`ViewKind`]).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        kind: ViewKind,
        target: &wgpu::TextureView,
        size: [u32; 2],
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Render);
        // The world renders at the render-scale size; the lens flare and the
        // resample work at the output size.
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Sky);
        let output = size;
        self.airfield_lights.update(device, queue, world);
        let slot = self.targets(device, output);
        let size = self.targets[slot].size;
        let weather = scenery.sample_view(world, camera.position[1], camera.weather_slot);
        // The recovered haze color the visibility ramp blends toward.
        let sky = weather.haze;
        let mut uniform = camera.uniform(
            size[0] as f32 / (size[1] as f32 * camera.view_fraction),
            weather.fog,
            sky,
        );
        uniform[7] = (scenery.texture_indices.len() / (256 * 256)) as f32;
        uniform[15] = weather.fog_palette.len() as f32;
        uniform.extend(weather.decks.into_iter().flatten());
        // Smooth weather's turning night sky: starfield and moving moon.
        let night_sky = scenery
            .smooth_weather
            .then(|| {
                crate::night_sky::NightSky::for_view(
                    world,
                    weather.sky.night,
                    camera.position[1],
                    scenery
                        .celestial
                        .as_ref()
                        .map_or([0., 0., 1.], |c| c.moon_center()),
                )
            })
            .flatten();
        let mut celestial_count = 0;
        let mut haze_rows = [-1., 0., 0., 0.];
        if let Some(celestial) = &scenery.celestial {
            let data = celestial.vertices(world, camera, size[1], night_sky.as_ref());
            assert!(data.len() * 4 <= 256 * 1024);
            celestial_count = (data.len() / 10) as u32;
            if !data.is_empty() {
                write_floats(queue, &self.celestial_vertices, 0, &data);
            }
            uniform[27] = celestial.sun_remap as f32;
            if let Some(layer) = world.weather.sample(camera.position[1]) {
                let shade = world.weather.configuration().shade_remap(layer.shade);
                uniform[31] = celestial.shade_rows[&shade.color] as f32;
            }
            // Smooth weather: the two heaviest records' own rows, crossed.
            let rows = weather
                .sky
                .shades
                .iter()
                .filter(|_| scenery.smooth_weather)
                .map(|(shade, weight)| {
                    let shade = world.weather.configuration().shade_remap(*shade);
                    (celestial.shade_rows[&shade.color], *weight)
                });
            if let Some((row, second)) = crate::sky_mix::cross_rows(rows) {
                uniform[31] = row as f32;
                if let Some((other, share)) = second {
                    haze_rows = [other as f32, share, 0., 0.];
                }
            }
            uniform.extend(celestial.sun_uniform(world, scenery, camera.position[1]));
        } else {
            uniform.extend([0.; 36]);
        }
        // Band rows refer to the imported remap atlas; absent assets disable them.
        uniform.extend([0.; 4]);
        let mut band_shades: Vec<f32> = Vec::with_capacity(32 * 4);
        let mut band_colours: Vec<[[u8; 3]; 256]> = Vec::new();
        if let Some(celestial) = &scenery.celestial {
            let bands = if scenery.smooth_weather {
                weather.visual_bands.as_slice()
            } else {
                world.weather.active()
            };
            uniform[68] = bands.len() as f32;
            // Each band's haze colours go to the palette texture's LUT rows;
            // smooth weather crosses a band's records' own shade tables there
            // (docs/spec/sun-glow.md, "Continuous dawn and dusk"), so the
            // ground's haze does not switch tables part way through.
            let configuration = world.weather.configuration();
            let records = configuration.layers();
            let crossed = if scenery.smooth_weather {
                crate::sky_mix::band_shares(records, world.weather.visual_seconds_of_day())
            } else {
                Vec::new()
            };
            let crossed = (crossed.len() == bands.len()).then_some(crossed);
            for (index, layer) in bands.iter().enumerate() {
                let mut first = configuration.shade_remap(layer.shade);
                let mut second = None;
                let pair = crossed.as_ref().and_then(|crossed| {
                    crate::sky_mix::cross_rows(crossed[index].iter().map(|&(record, weight)| {
                        (
                            configuration.shade_remap(records[record].shade).color,
                            weight as f32,
                        )
                    }))
                });
                if let Some((color, other)) = pair {
                    first = configuration.shade_remap(color);
                    second = other.map(|(c, share)| (configuration.shade_remap(c), share));
                }
                let colours = crate::sky_mix::band_colours(&weather.palette, first, second);
                let start = band_colours.len();
                let lut = if start + colours.len() <= LUT_ROWS {
                    band_colours.extend(colours);
                    start as f32
                } else {
                    -1.
                };
                band_shades.extend(second.map_or([0., 0., 0., lut], |(table, share)| {
                    [
                        celestial.shade_rows[&table.color] as f32,
                        table.levels.len() as f32,
                        share,
                        lut,
                    ]
                }));
                let row = celestial.shade_rows[&first.color];
                let levels = first.levels.len();
                uniform.extend([
                    layer.low_feet as f32,
                    layer.high_feet as f32,
                    row as f32,
                    levels as f32,
                    layer.fog_near as f32,
                    layer.fog_near_density as f32,
                    layer.fog_far as f32,
                    layer.fog_far_density as f32,
                ]);
            }
        }
        uniform.resize(328, 0.);
        if let Some(layer) = world.weather.sample(camera.position[1]) {
            let horizon = tore_sim::environment::horizon::Horizon::new(&layer, camera.position[1]);
            uniform[70] = horizon.lower_extent as f32;
            uniform[71] = (u16::from(horizon.flags())
                | ((layer.flags & 0x40) >> 4)
                | (u16::from(scenery.smooth_weather) << 3)) as f32;
            let roll = (camera.roll.rem_euclid(std::f32::consts::TAU) * 65536.
                / std::f32::consts::TAU)
                .round() as i32 as i16;
            uniform[19] =
                2. * f32::from(tore_sim::environment::horizon::lower_solid_offset(
                    size, roll,
                )) / 32767.;
        }
        let ocean_decks: [bool; 2] = std::array::from_fn(|i| weather.sky.decks[i].is_ocean());
        // Approximate angular width of a pixel, shared by the surface filtering.
        let pixel_angle = 2. / (1.732_050_8 * camera.zoom * size[1].max(1) as f32);
        uniform.extend(scenery.ocean_motion.uniform(
            world.weather.ticks(),
            ocean_decks,
            pixel_angle,
        ));
        let dense = weather
            .visual_bands
            .iter()
            .find(|band| band.fog_far_density >= 256 && band.fog_far * 256 <= 8000);
        let mut reflection = match (&scenery.clouds, dense) {
            (Some(clouds), Some(band)) if scenery.smooth_weather => [
                clouds.reflection_texture() as f32,
                (band.low_feet as f32 + 250.).max(500.),
                scenery
                    .ocean_motion
                    .uniform(world.weather.ticks(), [true, false], pixel_angle)[1],
                0.,
            ],
            _ => [-1., 0., 0., 0.],
        };
        reflection[2] =
            scenery
                .ocean_motion
                .uniform(world.weather.ticks(), [true, false], pixel_angle)[1];
        reflection[3] = scenery.ocean_motion.environment_reflection;
        uniform.extend(reflection);
        // Match SurfaceLighting::prepare even without celestial art or with
        // legacy weather: scene.sun can use a different direction in that mode.
        let night_sun = world
            .weather
            .sample(camera.position[1])
            .and_then(|layer| crate::celestial::visual_sun_direction(&layer, &world.weather))
            .unwrap_or([0., -1., 0.]);
        let night = crate::night_glow::night_factor(night_sun[1]);
        uniform.extend([
            camera.near_clip,
            f32::from(camera.weather_slot == 4),
            night,
            0.,
        ]);
        // Graphics options: spotting aid strength, terrain filtering and the
        // thermal sensor picture (`Camera::thermal`); then the world image
        // size in pixels, samples and scale.
        uniform.extend([
            self.options.spotting_aid.strength(),
            f32::from(self.options.terrain_filtering),
            f32::from(u8::from(camera.thermal)),
            if camera.thermal {
                thermal_reference(&world.weather.daylight_palette())
            } else {
                0.
            },
            size[0] as f32,
            size[1] as f32 * camera.view_fraction,
            self.samples as f32,
            self.options.scale(),
        ]);
        // The render origin, and the eye relative to it at full precision.
        let [x, y, z] = scenery.origin.map(|v| v as f32);
        let [ex, ey, ez] = scenery.local(camera.position);
        uniform.extend([x, y, z, 0., ex, ey, ez, 0.]);
        // Ground detail: mode, distance in feet, unused, theater climate.
        // Objects are a separate pass. `TORE_VEGETATION=0` leaves both off
        // because the scenery then has no atlas and the mode stays 0.
        let veg_mode = if scenery.vegetation.is_none() {
            0.
        } else {
            self.options.ground_texture.mode()
        };
        let veg_reach = self.options.ground_texture_distance_nm as f32 * 6_076.115_5;
        uniform.extend([
            veg_mode,
            veg_reach,
            self.options.vegetation_cards.effective() as u8 as f32,
            crate::vegetation::climate_id(&world.layout) as f32,
        ]);
        uniform.extend(scenery.canopy.area);
        // The canopy vector and the detail reach after it, which the
        // vegetation hand-off below fills in from this frame's stream.
        let canopy_slot = uniform.len();
        let visible = self.vegetation.as_ref().is_some_and(|v| !v.suppressed());
        uniform.extend([
            scenery.canopy.first as f32,
            scenery.canopy.pages_x as f32,
            self.vegetation
                .as_ref()
                .map_or(0., |v| v.draw_reach())
                .max(1.),
            if visible {
                self.options.vegetation_density.scale()
            } else {
                0.
            },
        ]);
        uniform.extend([
            self.vegetation.as_ref().map_or(0., |v| v.detail_reach()),
            scenery.seams[0],
            scenery.seams[1],
            scenery.seams[2],
        ]);
        uniform.extend(weather.sky.uniform());
        let (stars, moon) = night_sky.map_or(([0.; 4], [0., 1., 0., 0.]), |n| (n.stars, n.moon));
        uniform.extend(stars);
        uniform.extend(moon);
        uniform.extend(haze_rows);
        // Unused bands: no second table and no colour rows.
        while band_shades.len() < 32 * 4 {
            band_shades.extend([0., 0., 0., -1.]);
        }
        uniform.extend(band_shades);
        debug_assert_eq!(uniform.len() * 4, UNIFORM_BYTES as usize);
        let view_clip = crate::terrain_chunks::Clip::perspective(
            [uniform[4], uniform[5], uniform[6]],
            [uniform[8], uniform[9], uniform[10]],
            [uniform[12], uniform[13], uniform[14]],
            uniform[11],
            uniform[3],
            camera.near_clip,
            WORLD_FAR_FEET,
        );
        // The burning city fill buildings: the nearest fires and columns of
        // the camera, replaced every draw (`crate::city_fires`).
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        match world.city_fill.as_ref().filter(|_| self.city_fires_on) {
            Some(fill) => {
                // Flames flicker on the wall clock; a replay's columns age
                // on the playhead.
                let wall = self.city_fires.now();
                let (now, falls) = match &self.city_clock {
                    Some(clock) => (clock.tick, Some(&*clock.falls)),
                    None => (wall, None),
                };
                self.city_fires.observe_with(fill, now, falls);
                // The mirror's eye is about 12 ft from the main one: it
                // shows the main view's last fires and columns.
                let (frame, wall) = match (&self.city_main, kind) {
                    (Some(main), ViewKind::Mirror) => main.clone(),
                    _ => (self.city_fires.frame(camera.position, now), wall),
                };
                if kind == ViewKind::Main {
                    self.city_report = frame.report(self.city_fires.blazes().len());
                }
                self.effects.set_city_fires(&frame.fires, wall as u64);
                self.effects.set_far_city(frame.glows.clone());
                self.smoke.set_city_smoke(&frame.puffs);
                if kind == ViewKind::Main {
                    self.city_main = Some((frame, wall));
                }
            }
            None => {
                self.city_main = None;
                self.city_report = crate::city_fires::Report::default();
                self.effects.set_city_fires(&[], 0);
                self.effects.set_far_city(Vec::new());
                self.smoke.set_city_smoke(&[]);
            }
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        self.effects.update(queue, camera, world);
        // The night glow of every explosion and fire, and the ground light
        // of the nearest fires beside the flares' (nr-fx3).
        self.glow_points
            .update(queue, self.effects.glows(), camera.position);
        let mut lights = self.countermeasures.lights.clone();
        lights.extend(self.effects.sustained_lights(camera.position, night > 0.));
        self.smoke.update(
            queue,
            camera,
            size[0] as f32 / (size[1] as f32 * camera.view_fraction),
            &lights,
        );
        if night > 0. {
            // Reserve fires through the final scene budget, not just the effect budget.
            lights = crate::night_glow::night_lights(
                &lights,
                camera.position,
                crate::countermeasure_renderer::MAX_FLARE_LIGHTS,
            );
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Sky);
        let mut entries = Vec::with_capacity(12 * 1024);
        for row in std::iter::once(&weather.palette).chain(weather.fog_palette.iter()) {
            for rgb in row {
                entries.extend([rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        entries.resize(11 * 1024, 0);
        for rgb in world.weather.daylight_palette() {
            entries.extend([rgb[0], rgb[1], rgb[2], 255]);
        }
        for row in &band_colours {
            for rgb in row {
                entries.extend([rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        // Only the rows in use; rows past them are never read this frame.
        let rows = (entries.len() / 1024) as u32;
        if entries != self.palette_uploaded {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.palette,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &entries,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1024),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: 256,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            self.palette_uploaded = entries;
        }
        let cloud_data = scenery
            .clouds
            .as_ref()
            .map_or_else(Vec::new, |clouds| clouds.vertices(camera));
        assert!(cloud_data.len() * 4 <= 256 * 1024);
        let cloud_count = (cloud_data.len() / 10) as u32;
        if cloud_count > 0 {
            write_floats(queue, &self.cloud_vertices, 0, &cloud_data);
        }
        let linear = |v: u8| ((v as f64 / 255.0 + 0.055) / 1.055).powf(2.4);
        // A thermal picture has no lens flare: the sun is just a hot spot. No
        // sensor picture has one.
        let flare_target = if camera.thermal || camera.sensor {
            None
        } else {
            self.lens_flare.prepare(
                device,
                queue,
                world,
                scenery,
                camera,
                output,
                [
                    size[0] as f32 / output[0] as f32,
                    size[1] as f32 / output[1] as f32,
                ],
                &weather.palette,
            )
        };
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        // The fill's cull runs first: its lists feed the shadow maps too.
        if let Some(fill) = &mut self.fill {
            fill.prepare(queue, encoder, camera, scenery.origin, size);
        }
        if let Some(vegetation) = &mut self.vegetation {
            vegetation.prepare(
                device,
                queue,
                encoder,
                kind,
                camera,
                world,
                scenery.origin,
                size,
            );
            // Use this frame's uploaded circle for the terrain hand-off too.
            uniform[canopy_slot + 2] = vegetation.draw_reach().max(1.);
            uniform[canopy_slot + 4] = vegetation.detail_reach();
            // The grass candidate pass reads this view's world uniform,
            // written below before the submission runs.
            vegetation.encode_blades(encoder, &self.bind);
        }
        write_floats(queue, &self.uniform, 0, &uniform);
        // Terrain vertices are in world feet and the shader takes them
        // relative to the eye; object batches are relative to the render
        // origin and the shader subtracts the eye's local position.
        let local_eye = scenery.local(camera.position).map(f64::from);
        if self
            .lighting
            .prepare(queue, camera, world, scenery, &lights)
        {
            use crate::surface_lighting::Caster;
            fn caster<'a>(
                bind: &'a wgpu::BindGroup,
                buffer: &'a wgpu::Buffer,
                count: u32,
                shape: &'a CasterShape,
                eye: [f64; 3],
            ) -> Caster<'a> {
                Caster {
                    bind,
                    buffer,
                    count,
                    bounds: shape.bounds.relative(eye),
                    opaque: &shape.opaque,
                    tested: &shape.tested,
                }
            }
            let caster = |bind, buffer, count, shape| caster(bind, buffer, count, shape, local_eye);
            let mut objects = Vec::new();
            if let Some((buffer, count)) = &self.surface_units
                && *count > 0
            {
                objects.push(caster(
                    &self.bind,
                    buffer,
                    *count,
                    &self.bounds.surface_units,
                ));
            }
            if let Some((bind, buffer, count)) = &self.aircraft {
                objects.push(caster(bind, buffer, *count, &self.bounds.aircraft));
                if let Some((buffer, count)) = &self.battle {
                    objects.push(caster(bind, buffer, *count, &self.bounds.battle));
                }
            }
            for (_, (bind, buffer, count), _, shape) in &self.dummies {
                objects.push(caster(bind, buffer, *count, shape));
            }
            self.lighting.draw(
                encoder,
                crate::surface_lighting::TerrainCasters {
                    bind: &self.bind,
                    buffer: &self.vertices,
                    chunks: &self.chunks,
                    eye: camera.position,
                },
                self.static_casters.as_ref().map(|(buffer, chunks)| {
                    crate::surface_lighting::TerrainCasters {
                        bind: &self.bind,
                        buffer,
                        chunks,
                        eye: camera.position,
                    }
                }),
                &objects,
            );
            if let Some(fill) = &self.fill {
                fill.draw_shadows(&self.lighting, encoder, &self.bind);
            }
            if let Some(vegetation) = &self.vegetation {
                vegetation.draw_shadows(&self.lighting, encoder, &self.bind);
            }
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        // The spotting aid covers other aircraft in the main view and the
        // mirrors, not the instrument camera panels. The ownship batch is
        // excluded; combat targets share its airframe when no dummies load.
        let rim = camera.weather_slot <= 1
            && !camera.thermal
            && !camera.sensor
            && self.options.spotting_aid.strength() > 0.;
        let mut others: Vec<(&wgpu::BindGroup, &wgpu::Buffer, &[Contact])> = Vec::new();
        if rim {
            if let (Some((bind, _, _)), Some((buffer, _))) = (&self.aircraft, &self.battle) {
                others.push((bind, buffer, &self.battle_contacts));
            }
            for (_, (bind, buffer, _), contacts, _) in &self.dummies {
                others.push((bind, buffer, contacts));
            }
            others.retain(|(_, _, contacts)| !contacts.is_empty());
        }
        let contacts: Vec<&Contact> = others.iter().flat_map(|o| o.2.iter()).collect();
        if !contacts.is_empty() {
            let needed = contacts.len() as u64 * SPOT_CONTACT_BYTES;
            if self.spot_instances.size() < needed {
                self.spot_instances =
                    Self::spot_buffer(device, (contacts.len() as u64).next_power_of_two());
            }
            write_floats(queue, &self.spot_instances, 0, &spot_instances(&contacts));
        }
        if crate::frame_log::enabled() {
            crate::frame_log::count(
                crate::frame_log::Count::Aircraft,
                self.battle_contacts.len()
                    + self
                        .dummies
                        .iter()
                        .map(|(_, _, contacts, _)| contacts.len())
                        .sum::<usize>()
                    + usize::from(self.aircraft_visible),
            );
            crate::frame_log::count(
                crate::frame_log::Count::Vegetation,
                self.vegetation.as_ref().map_or(0, |v| v.placed() as usize),
            );
        }
        let destination = flare_target.as_ref().unwrap_or(target);
        let targets = &self.targets[slot];
        // Multisampled color resolves into the render-scale image when there
        // is one, otherwise straight into the destination.
        let direct = targets
            .scaled
            .as_ref()
            .map_or(destination, |(view, _)| view);
        let (view, resolve_target) = match &targets.color {
            Some(color) => (color, Some(direct)),
            None => (direct, None),
        };
        // Flare glare, the spotting aid and the lens flare's sun measurement
        // are the only later readers of the world depth; all are decided
        // before the pass.
        let flares = self.countermeasures.has_flares();
        let spotting = rim && !contacts.is_empty();
        let (color_store, depth_store) = world_store_ops(
            resolve_target.is_some(),
            flares || spotting || flare_target.is_some(),
        );
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Simulation world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: linear(sky[0]),
                        g: linear(sky[1]),
                        b: linear(sky[2]),
                        a: if camera.weather_slot == 4 { 0. } else { 1. },
                    }),
                    store: color_store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: depth_store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_viewport(
            0.,
            0.,
            size[0] as f32,
            size[1] as f32 * camera.view_fraction,
            0.,
            1.,
        );
        pass.set_bind_group(1, &self.lighting.bind, &[]);
        // City fill buildings go before the terrain, so the depth test
        // spares the terrain's costly texture filtering behind them.
        if let Some(fill) = &self.fill {
            fill.draw(&mut pass, &self.bind);
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Terrain);
        // The terrain chunks in view, nearest first, so nearer hills hide
        // farther ground before it is shaded.
        self.chunks
            .front_to_back(camera.position, &view_clip, &mut self.chunk_order);
        pass.set_pipeline(&self.p.terrain_pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_vertex_buffer(1, self.terrain_normals.slice(..));
        for range in &self.chunk_order {
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(range.clone(), 0..1);
        }
        crate::frame_log::count(crate::frame_log::Count::Terrain, self.chunk_order.len());
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Sky);
        // The sky after the opaque fill and terrain, only where neither
        // wrote depth (the clear value 0): each sample takes exactly the
        // colour it took when the sky went first and was drawn over, but
        // covered samples skip its shader. The sun, moon and stars blend
        // onto the sky under the same test. Everything before here writes
        // colour only with depth, without blending.
        pass.set_pipeline(&self.p.sky_pipeline);
        crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
        pass.draw(0..3, 0..1);
        if celestial_count > 0 {
            pass.set_pipeline(&self.p.celestial_pipeline);
            pass.set_vertex_buffer(0, self.celestial_vertices.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..celestial_count, 0..1);
        }
        if let Some(vegetation) = &self.vegetation {
            vegetation.draw(&mut pass, &self.bind);
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        if let Some((bind, buffer, count)) = &self.escapees {
            pass.set_pipeline(&self.p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        if let Some((buffer, count)) = &self.airports {
            pass.set_pipeline(&self.p.airport_pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
            pass.set_pipeline(&self.p.airport_decal_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        if let Some((buffer, count)) = &self.surface_units
            && *count > 0
        {
            let _surface_log = crate::frame_log::span(crate::frame_log::Phase::Surface);
            crate::frame_log::count(crate::frame_log::Count::Surface, *count as usize);
            pass.set_pipeline(&self.p.moving_pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
            pass.set_pipeline(&self.p.moving_decal_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        if let Some((buffer, count)) = &self.airport_lines {
            pass.set_pipeline(&self.p.scenery_line_pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        pass.set_pipeline(&self.p.pipeline);
        if let Some((bind, vertices, count)) = &self.aircraft {
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            if self.aircraft_visible {
                crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
                pass.draw(0..*count, 0..1);
            }
            if let Some((buffer, count)) = &self.battle {
                pass.set_vertex_buffer(0, buffer.slice(..));
                crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
                pass.draw(0..*count, 0..1);
            }
        }
        for (_, (bind, vertices, count), _, _) in &self.dummies {
            pass.set_pipeline(&self.p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
            pass.set_pipeline(&self.p.canopy_depth_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
            pass.set_pipeline(&self.p.canopy_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        if self.canopy_visible
            && let Some((bind, vertices, count)) = &self.aircraft
        {
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_pipeline(&self.p.canopy_depth_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
            pass.set_pipeline(&self.p.canopy_pipeline);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Sky);
        if cloud_count > 0 {
            pass.set_pipeline(&self.p.cloud_pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.cloud_vertices.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..cloud_count, 0..1);
        }
        drop(phase_log);
        let phase_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        // Blended and depth-tested but not depth-writing, so trails read behind
        // terrain and aircraft without occluding each other.
        if let Some((buffer, count)) = &self.vapor
            && *count > 0
        {
            pass.set_pipeline(&self.p.vapor_pipeline);
            pass.set_bind_group(0, &self.vapor_bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        if let Some((buffer, count)) = &self.battle {
            pass.set_pipeline(&self.p.tracer_pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..*count, 0..1);
        }
        self.effects.draw(&mut pass);
        self.smoke.draw(&mut pass);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_bind_group(1, &self.lighting.bind, &[]);
        self.countermeasures
            .draw_world(&mut pass, &self.p.countermeasures);
        self.airfield_lights
            .draw(&mut pass, &self.p.airfield_lights);
        self.glow_points.draw(&mut pass, &self.p.night_glow);
        drop(phase_log);
        let _phase_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        drop(pass);
        if let Some((_, bind)) = &targets.scaled {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render scale resample"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.resample);
            pass.set_bind_group(0, bind, &[]);
            crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
            pass.draw(0..3, 0..1);
        }
        // Flare glare goes over the finished image, where it can spill across
        // the aircraft that released it while the core stays in view.
        if flares {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Flare glare"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                0.,
                0.,
                output[0] as f32,
                output[1] as f32 * camera.view_fraction,
                0.,
                1.,
            );
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_bind_group(1, &self.lighting.bind, &[]);
            pass.set_bind_group(2, &targets.rim_depth, &[]);
            self.countermeasures
                .draw_glare(&mut pass, &self.p.countermeasures);
        }
        // The spotting aid goes over the finished image at output resolution.
        if spotting {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Spotting aid"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                0.,
                0.,
                output[0] as f32,
                output[1] as f32 * camera.view_fraction,
                0.,
                1.,
            );
            pass.set_bind_group(1, &self.lighting.bind, &[]);
            pass.set_bind_group(2, &targets.rim_depth, &[]);
            pass.set_vertex_buffer(1, self.spot_instances.slice(..));
            for pipeline in [&self.p.rim_dark_pipeline, &self.p.rim_light_pipeline] {
                pass.set_pipeline(pipeline);
                // Each contact draws its own vertex range; its instance slots
                // start at a multiple of eight, so `instance_index % 8` is the
                // copy.
                let mut slot = 0;
                for (bind, buffer, contacts) in &others {
                    pass.set_bind_group(0, *bind, &[]);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    for contact in contacts.iter() {
                        let base = slot * SPOT_INSTANCES as u32;
                        crate::frame_log::count(crate::frame_log::Count::WorldDraws, 1);
                        pass.draw(
                            contact.first..contact.first + contact.count,
                            base..base + SPOT_INSTANCES as u32,
                        );
                        slot += 1;
                    }
                }
            }
        }
        if flare_target.is_some() {
            self.lens_flare.measure(encoder, &targets.rim_depth);
            self.lens_flare.draw(encoder, target);
        }
    }
}

/// Store operations for the world pass's colour and depth. Multisampled
/// colour that resolves is never read again, and the depth only by flare
/// glare and the spotting aid, so neither is written back otherwise.
fn world_store_ops(resolves: bool, depth_read: bool) -> (wgpu::StoreOp, wgpu::StoreOp) {
    let store = |keep: bool| {
        if keep {
            wgpu::StoreOp::Store
        } else {
            wgpu::StoreOp::Discard
        }
    };
    (store(!resolves), store(depth_read))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod lighting_tests {
    use super::*;

    #[test]
    fn world_pass_stores_only_what_a_later_pass_reads() {
        use wgpu::StoreOp::{Discard, Store};
        // A single-sample picture is the destination itself.
        assert_eq!(world_store_ops(false, false), (Store, Discard));
        // Multisampled colour is read through its resolve.
        assert_eq!(world_store_ops(true, false), (Discard, Discard));
        // Glare or the spotting aid read the depth.
        assert_eq!(world_store_ops(true, true), (Discard, Store));
        assert_eq!(world_store_ops(false, true), (Store, Store));
    }

    #[test]
    fn world_pages_pack_more_than_256_images_without_losing_indices() {
        let mut pages = vec![0; 300 * 65536];
        for (n, page) in pages.chunks_exact_mut(65536).enumerate() {
            page.fill((n % 251) as u8);
            page[0] = (n / 251) as u8;
        }
        let packed = pack_world_pages(&pages);
        assert_eq!(packed.len(), 19 * 1024 * 1024);
        for n in 0..300 {
            for (x, y) in [(0, 0), (255, 255), (100, 42)] {
                let at = (n / 16) * 1024 * 1024 + (n % 16 / 4 * 256 + y) * 1024 + n % 4 * 256 + x;
                assert_eq!(packed[at], pages[n * 65536 + y * 256 + x]);
            }
        }
        assert_eq!(packed[18 * 1024 * 1024 + 3 * 256 * 1024], 255);
    }
    #[test]
    fn contacts_cover_nonempty_ranges_and_repeat_per_copy() {
        assert_eq!(Contact::new(30, 30, [0.; 3], 56.), None);
        let a = Contact::new(0, 90, [1., 2., 3.], 56.).unwrap();
        let b = Contact::new(90, 120, [4., 5., 6.], 40.).unwrap();
        assert_eq!((b.first, b.count), (90, 30));
        let data = spot_instances(&[&a, &b]);
        assert_eq!(data.len() as u64 * 4, 2 * SPOT_CONTACT_BYTES);
        assert!(
            data[..64]
                .chunks(8)
                .all(|v| v == [1., 2., 3., 56., 0., 0., 0., 0.])
        );
        assert!(
            data[64..]
                .chunks(8)
                .all(|v| v == [4., 5., 6., 40., 90., 0., 0., 0.])
        );
    }

    fn plane(y: f32, radius: f32) -> Vec<f32> {
        [
            (-1., -1.),
            (-1., 1.),
            (1., -1.),
            (1., -1.),
            (-1., 1.),
            (1., 1.),
        ]
        .into_iter()
        .flat_map(|(x, z)| [x * radius, y, z * radius, 0., 0., -1., 0.6, 0.6, 0.6, -1.])
        .collect()
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_airport_pavement_does_not_hide_aircraft_above_it() {
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
            let render = |distance: f32, pavement: u8, overhead: bool, height: f32| {
                let world = tore_world::test_support::terrain();
                let mut scenery = crate::scenery::tests::scenery();
                scenery.vertices = plane(0., 20_000.);
                scenery.texture_indices = vec![100; 65536];
                scenery.sky_indices = vec![100; 65536];
                scenery.smooth_weather = false;
                let mut renderer = SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::default(),
                    1,
                );
                // A red fuselage proxy: eight feet high, safely above contact.
                let mut aircraft: Vec<f32> = [
                    (-32., 4.),
                    (-32., 12.),
                    (32., 4.),
                    (32., 4.),
                    (-32., 12.),
                    (32., 12.),
                ]
                .into_iter()
                .flat_map(|(x, y)| [x, y, 0., 0., 0., -1., 1., 0., 0., -1.])
                .collect();
                if overhead {
                    aircraft = plane(8., 32.);
                    for v in aircraft.chunks_exact_mut(10) {
                        v[6] = 1.;
                        v[7] = 0.;
                        v[8] = 0.;
                    }
                }
                for v in aircraft.chunks_exact_mut(10) {
                    v[1] += height - 8.;
                }
                let vertex = |data: &[f32]| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Synthetic runway visibility"),
                        contents: &bytes(data),
                        usage: wgpu::BufferUsages::VERTEX,
                    })
                };
                renderer.aircraft = Some((renderer.bind.clone(), vertex(&aircraft), 6));
                if pavement > 0 {
                    let mut apron = plane(0., 20_000.);
                    if pavement == 2 {
                        for v in apron.chunks_exact_mut(10) {
                            v[5] = 0.;
                        }
                    }
                    renderer.airports = Some((vertex(&apron), 6));
                }
                let mut camera = Camera::new();
                camera.position = [0., 15., -f64::from(distance)];
                camera.pitch = (-7.0_f32).atan2(distance);
                camera.yaw = 0.;
                if overhead {
                    camera.position = [0., f64::from(distance), 0.];
                    camera.pitch = -std::f32::consts::FRAC_PI_2;
                }
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 256 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.draw(
                    &device,
                    &queue,
                    &mut encoder,
                    &texture.create_view(&Default::default()),
                    [256, 256],
                    &camera,
                    &world,
                    &scenery,
                );
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(1024),
                            rows_per_image: Some(256),
                        },
                    },
                    wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
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
                let pixels = buffer.slice(..).get_mapped_range().to_vec();
                buffer.unmap();
                pixels
                    .chunks_exact(4)
                    .filter(|p| u16::from(p[0]) > u16::from(p[1]) + 30 && p[0] > p[2])
                    .count()
            };
            for (distance, overhead) in
                [(500., false), (1000., false), (1500., false), (5000., true)]
            {
                let reference = render(distance, 0, overhead, 8.);
                let solid = render(distance, 1, overhead, 8.);
                assert_eq!(solid, reference, "solid airport surface at {distance} feet");
                let with_airport = render(distance, 2, overhead, 8.);
                assert!(reference > 0, "fixture invisible at {distance} feet");
                assert_eq!(
                    with_airport, reference,
                    "pavement hides an aircraft at {distance} feet"
                );
            }
            assert_eq!(
                render(500., 2, true, -8.),
                0,
                "buried geometry must still be occluded"
            );
        });
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_airport_stays_visible_with_distant_moving_cameras() {
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
            let render = |distance: f32, shift: f32, ground: bool, samples: u32| {
                let world = tore_world::test_support::terrain();
                let mut scenery = crate::scenery::tests::scenery();
                scenery.smooth_weather = false;
                scenery.texture_indices = vec![100; 65536];
                scenery.sky_indices = vec![100; 65536];
                let origin = [1_107_332., 1024., 587_544.];
                let transform = |v: &mut Vec<f32>, color: [f32; 3]| {
                    for vertex in v.chunks_exact_mut(10) {
                        for axis in 0..3 {
                            vertex[axis] += origin[axis];
                        }
                        vertex[6..9].copy_from_slice(&color);
                    }
                };
                scenery.vertices = plane(if ground { -1. } else { -1000. }, 80_000.);
                transform(&mut scenery.vertices, [0., 0.5, 0.]);
                let mut runway = plane(0., 2400.);
                transform(&mut runway, [1., 0., 0.]);
                let mut renderer = SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::default(),
                    samples,
                );
                renderer.airports = Some((
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("One-foot runway/terrain separation"),
                        contents: &bytes(&runway),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    6,
                ));
                let mut camera = Camera::new();
                camera.position = [
                    origin[0] + 1000. + shift,
                    origin[1] + 1500.,
                    origin[2] - distance,
                ]
                .map(f64::from);
                camera.yaw = (-1000. - shift).atan2(distance);
                camera.pitch = (-1500_f32).atan2(distance.hypot(1000. + shift));
                let image = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 256 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.draw(
                    &device,
                    &queue,
                    &mut encoder,
                    &image.create_view(&Default::default()),
                    [256, 256],
                    &camera,
                    &world,
                    &scenery,
                );
                encoder.copy_texture_to_buffer(
                    image.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(1024),
                            rows_per_image: Some(256),
                        },
                    },
                    wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let pixels = readback.slice(..).get_mapped_range().to_vec();
                readback.unmap();
                pixels
            };
            let red = |p: &[u8]| u16::from(p[0]) > u16::from(p[1]) + 30 && p[0] > p[2];
            for (distance, samples) in [1, 4]
                .into_iter()
                .flat_map(|samples| [10_000., 20_000., 40_000.].map(|distance| (distance, samples)))
            {
                for frame in 0..8 {
                    let shift = frame as f32 * 0.125;
                    let reference = render(distance, shift, false, samples);
                    let actual = render(distance, shift, true, samples);
                    let expected = reference.chunks_exact(4).filter(|p| red(p)).count();
                    let visible = actual.chunks_exact(4).filter(|p| red(p)).count();
                    assert!(expected >= 10, "fixture too small at {distance}");
                    assert_eq!(
                        visible, expected,
                        "runway flicker at {distance} ft, camera step {frame}, samples {samples}"
                    );
                }
            }
        });
    }

    // Runs the production shader, shadow maps, surface pipelines and readback.
    // A burning flare 40 feet above a ground plane at midnight lights the
    // ground beneath it (docs/spec/countermeasures.md).
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_burning_flare_lights_the_ground_beneath_it() {
        use tore_sim::combat::countermeasures::{Devices, Release};
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
            let render = |flare: bool| {
                let mut world = tore_world::test_support::terrain();
                let mut scenery = crate::scenery::tests::scenery();
                let mut module = tore_formats::weather::Module::parse(
                    &tore_formats::weather::synthetic_module(1),
                )
                .unwrap();
                for layer in &mut module.layers {
                    layer.start_seconds = 0;
                    layer.end_seconds = 86399;
                    layer.sunrise_seconds = 6 * 3600;
                    layer.sunset_seconds = 18 * 3600;
                    layer.fog_near_density = 0;
                    layer.fog_far_density = 0;
                }
                world.weather = tore_sim::environment::Environment::new(
                    tore_sim::environment::Configuration::new(module, 0, 0, 0, None).unwrap(),
                );
                scenery.smooth_weather = true;
                scenery.no_sun_whiteout = true;
                scenery.texture_indices = vec![255; 65536];
                scenery.sky_indices = vec![100; 65536];
                scenery.vertices = plane(0., 400.);
                let mut renderer = SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::original(),
                    1,
                );
                let mut devices = Devices::default();
                if flare {
                    devices.release_flare(Release {
                        position: [0., 40., 15.],
                        velocity: [0.; 3],
                        basis: tore_sim::attitude::Basis::new(0., 0., 0.),
                    });
                    for _ in 0..30 {
                        devices.step(&|_, _| 0.);
                    }
                }
                renderer
                    .countermeasures
                    .upload(&queue, &devices, &[], &Default::default());
                let mut camera = Camera::new();
                camera.position = [0., 200., 0.];
                camera.pitch = -std::f32::consts::FRAC_PI_2;
                camera.yaw = 0.;
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Flare lighting readback"),
                    size: wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 256 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.draw(
                    &device,
                    &queue,
                    &mut encoder,
                    &texture.create_view(&Default::default()),
                    [256, 256],
                    &camera,
                    &world,
                    &scenery,
                );
                encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(1024),
                            rows_per_image: Some(256),
                        },
                    },
                    texture.size(),
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let pixels = buffer.slice(..).get_mapped_range().to_vec();
                buffer.unmap();
                pixels
            };
            let dark = render(false);
            let lit = render(true);
            // Ground a little away from the flare bodies, so their glow and
            // glare are not what is measured.
            let at = |pixels: &[u8], x: usize, y: usize| {
                let i = (y * 256 + x) * 4;
                u16::from(pixels[i]) + u16::from(pixels[i + 1]) + u16::from(pixels[i + 2])
            };
            let (near, far) = ((128, 20), (8, 128));
            assert!(
                at(&lit, near.0, near.1) > at(&dark, near.0, near.1) + 120,
                "ground under the flare: lit={} dark={}",
                at(&lit, near.0, near.1),
                at(&dark, near.0, near.1)
            );
            // Warm light: red rises more than blue.
            let i = (near.1 * 256 + near.0) * 4;
            assert!(lit[i] > lit[i + 2]);
            // Light falls off with distance across the plane.
            assert!(
                at(&lit, far.0, far.1) - at(&dark, far.0, far.1)
                    < at(&lit, near.0, near.1) - at(&dark, near.0, near.1)
            );
        });
    }

    // Synthetic geometry only; explicitly invoked on a GPU-capable host.
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_geometry_shadows_cross_terrain_and_objects() {
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
            let render = |minutes,
                          terrain_caster: bool,
                          caster: bool,
                          smooth: bool,
                          tilt: bool,
                          material: f32,
                          view_x: f32,
                          water: bool,
                          caster_height: f32,
                          elapsed_steps: usize| {
                let mut world = tore_world::test_support::terrain();
                let mut scenery = crate::scenery::tests::scenery();
                let mut module = tore_formats::weather::Module::parse(
                    &tore_formats::weather::synthetic_module(1),
                )
                .unwrap();
                for layer in &mut module.layers {
                    layer.start_seconds = 0;
                    layer.end_seconds = 86399;
                    layer.sunrise_seconds = 6 * 3600;
                    layer.sunset_seconds = 18 * 3600;
                    layer.sun_azimuth_morning = 16384;
                    layer.sun_azimuth_evening = -16384;
                    layer.fog_near_density = 0;
                    layer.fog_far_density = 0;
                }
                world.weather = tore_sim::environment::Environment::new(
                    tore_sim::environment::Configuration::new(
                        module,
                        minutes / 60,
                        minutes % 60,
                        0,
                        None,
                    )
                    .unwrap(),
                );
                for _ in 0..elapsed_steps {
                    world.weather.step();
                }
                scenery.smooth_weather = smooth;
                scenery.no_sun_whiteout = true; // Shadows must work with sunglare disabled.
                scenery.texture_indices = vec![255; 65536];
                scenery.sky_indices = vec![100; 65536];
                scenery.vertices = if terrain_caster {
                    plane(caster_height, 10.)
                } else {
                    plane(0., 120.)
                };
                if terrain_caster && !caster {
                    scenery.vertices.clear();
                }
                if tilt {
                    for vertex in scenery.vertices.chunks_exact_mut(10) {
                        let x = vertex[0];
                        vertex[0] = x * 0.5;
                        vertex[1] = -x * 3_f32.sqrt() * 0.5;
                    }
                }
                // A zero-sized vertex buffer is not portable; supply a degenerate triangle.
                if scenery.vertices.is_empty() {
                    scenery.vertices = vec![0.; 30];
                }
                let mut renderer = SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::original(),
                    1,
                );
                if water {
                    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("Production water reflection probe"),
                        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}\n{}", include_str!("surface_lighting.wgsl"), include_str!("terrain.wgsl"),
                            "@fragment fn reflection_probe(in:VertexOut)->@location(0) vec4<f32>{return vec4<f32>(water_sun(vec3<f32>(0.05),normalize(vec3<f32>(1.0,-1.0,0.0)),scene.eye.xyz+in.direction,1000.0),1.0);}").into()),
                    });
                    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[
                            &renderer.p.pipeline.get_bind_group_layout(0),
                            &renderer.lighting.layout,
                        ],
                        push_constant_ranges: &[],
                    });
                    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("Water receiver probe"), layout: Some(&layout),
                        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: 40, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32] }] },
                        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("reflection_probe"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                        primitive: Default::default(), depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Greater, stencil: Default::default(), bias: Default::default() }), multisample: Default::default(), multiview: None, cache: None,
                    });
                    renderer.p.terrain_pipeline = pipeline.clone();
                    renderer.p.pipeline = pipeline;
                }
                let mut object = if terrain_caster {
                    plane(0., 120.)
                } else if caster {
                    plane(caster_height, 10.)
                } else {
                    vec![]
                };
                if !terrain_caster {
                    for vertex in object.chunks_exact_mut(10) {
                        // A vertical blocker exercises grazing sunlight at sunrise.
                        if minutes == 6 * 60 {
                            vertex[1] = vertex[0];
                            vertex[0] = 30.;
                        }
                        vertex[5] = material;
                    }
                }
                if !object.is_empty() {
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Synthetic opaque object"),
                        contents: &bytes(&object),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    renderer.aircraft =
                        Some((renderer.bind.clone(), buffer, (object.len() / 10) as u32));
                    renderer.bounds.aircraft = CasterShape::of(&object);
                }
                if !terrain_caster {
                    renderer.hide_aircraft();
                }
                let mut camera = Camera::new();
                camera.position = [0., 200., 0.];
                camera.pitch = -std::f32::consts::FRAC_PI_2;
                camera.yaw = 0.;
                if view_x != 0. {
                    camera.position[0] = f64::from(view_x);
                    camera.yaw = -view_x.signum() * std::f32::consts::FRAC_PI_2;
                    camera.pitch = -200_f32.atan2(view_x.abs());
                }
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Synthetic lighting readback"),
                    size: wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 256 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.draw(
                    &device,
                    &queue,
                    &mut encoder,
                    &texture.create_view(&Default::default()),
                    [256, 256],
                    &camera,
                    &world,
                    &scenery,
                );
                if water {
                    // Fixed clear-white source and coarse footprint isolate the actual
                    // water_sun consumer from imported art and procedural ripple detail.
                    let axis = 0.5_f32.sqrt();
                    queue.write_buffer(
                        &renderer.uniform,
                        128,
                        &bytes(&[axis, axis, 0., 1., 0.1, 100., 1., 0.]),
                    );
                    write_floats(&queue, &renderer.uniform, 1324, &[1.]);
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &renderer.palette,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: 100, y: 0, z: 0 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &[255; 4],
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(4),
                            rows_per_image: Some(1),
                        },
                        wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(1024),
                            rows_per_image: Some(256),
                        },
                    },
                    wgpu::Extent3d {
                        width: 256,
                        height: 256,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let pixels = buffer.slice(..).get_mapped_range().to_vec();
                buffer.unmap();
                pixels
            };
            let mut previous: Option<Vec<u8>> = None;
            for frame in 0..11 {
                let image = render(
                    6 * 60 + 1,
                    false,
                    false,
                    true,
                    true,
                    -1.,
                    frame as f32 * 0.02,
                    false,
                    30.,
                    frame * 60,
                );
                if let Some(prior) = previous {
                    for y in 112..144 {
                        for x in 112..144 {
                            for c in 0..3 {
                                let at = (y * 256 + x) * 4 + c;
                                assert!(
                                    (i32::from(image[at]) - i32::from(prior[at])).abs() <= 2,
                                    "sloped receiver must not flicker across five seconds, frame={frame} at={x},{y}"
                                );
                            }
                        }
                    }
                }
                previous = Some(image);
            }
            let near = render(12 * 60, false, true, true, false, -1., 0., false, 2., 0);
            let far = render(12 * 60, false, true, true, false, -1., 0., false, 500., 0);
            let clear = render(12 * 60, false, false, true, false, -1., 0., false, 2., 0);
            let dark = near[(128 * 256 + 128) * 4];
            let soft_pixels = |pixels: &[u8]| {
                (90..166)
                    .filter(|x| {
                        let at = (128 * 256 + x) * 4;
                        pixels[at] > dark + 4 && pixels[at] + 4 < clear[at]
                    })
                    .count()
            };
            assert!(
                soft_pixels(&far) >= soft_pixels(&near) + 4,
                "distant caster must have a wider penumbra: near={} far={}",
                soft_pixels(&near),
                soft_pixels(&far)
            );
            for terrain_caster in [false, true] {
                for (hour, x) in [(9, 95), (15, 160)] {
                    let clear = render(
                        hour * 60,
                        terrain_caster,
                        false,
                        true,
                        false,
                        -1.,
                        0.,
                        false,
                        30.,
                        0,
                    );
                    let shadow = render(
                        hour * 60,
                        terrain_caster,
                        true,
                        true,
                        false,
                        -1.,
                        0.,
                        false,
                        30.,
                        0,
                    );
                    let at = (128 * 256 + x) * 4;
                    assert!(
                        u16::from(shadow[at]) * 2 < u16::from(clear[at]),
                        "caster terrain={terrain_caster}, hour={hour}: shadow={} clear={}",
                        shadow[at],
                        clear[at]
                    );
                    let away = (128 * 256 + (255 - x)) * 4;
                    assert!(
                        (i32::from(shadow[away]) - i32::from(clear[away])).abs() <= 2,
                        "opposite side must stay lit"
                    );
                    let stepped_clear = render(
                        hour * 60,
                        terrain_caster,
                        false,
                        false,
                        false,
                        -1.,
                        0.,
                        false,
                        30.,
                        0,
                    );
                    let stepped_shadow = render(
                        hour * 60,
                        terrain_caster,
                        true,
                        false,
                        false,
                        -1.,
                        0.,
                        false,
                        30.,
                        0,
                    );
                    assert_eq!(
                        stepped_clear[at], stepped_shadow[at],
                        "compatibility has no geometric shadow"
                    );
                }
            }
            for terrain_caster in [false, true] {
                let unblocked = render(
                    9 * 60,
                    terrain_caster,
                    false,
                    true,
                    false,
                    -1.,
                    0.,
                    true,
                    30.,
                    0,
                );
                let blocked = render(
                    9 * 60,
                    terrain_caster,
                    true,
                    true,
                    false,
                    -1.,
                    0.,
                    true,
                    30.,
                    0,
                );
                let at = (128 * 256 + 95) * 4;
                assert!(unblocked[at] > 80, "clear water must have a visible glint");
                assert!(
                    (i32::from(blocked[at]) - 13).abs() <= 1,
                    "blocked glint must return the 0.05 water base, got {}",
                    blocked[at]
                );
                let away = (128 * 256 + 160) * 4;
                assert_eq!(
                    unblocked[away], blocked[away],
                    "unblocked water must keep its reflection"
                );
            }
            let clear = render(9 * 60, false, false, true, false, -1., 0., false, 30., 0);
            for material in [-2., -5., -6., -7., -8.] {
                let transparent = render(
                    9 * 60,
                    false,
                    true,
                    true,
                    false,
                    material,
                    0.,
                    false,
                    30.,
                    0,
                );
                let at = (128 * 256 + 95) * 4;
                assert_eq!(
                    transparent[at], clear[at],
                    "cutouts, glass and emissive effects must not cast a solid shadow: {material}"
                );
            }
            let at = (128 * 256 + 128) * 4;
            let horizon_clear = render(6 * 60, false, false, true, true, -1., 0., false, 30., 0);
            let horizon_shadow = render(6 * 60, false, true, true, true, -1., 0., false, 30., 0);
            assert!(
                horizon_shadow[at] + 15 < horizon_clear[at],
                "the half-visible sun must still cast a panel shadow: {} vs {}",
                horizon_shadow[at],
                horizon_clear[at]
            );
            let toward_highlight =
                render(9 * 60, true, false, true, false, -1., -200., false, 30., 0);
            let away_highlight = render(9 * 60, true, false, true, false, -1., 200., false, 30., 0);
            assert!(
                toward_highlight[at] > away_highlight[at] + 12,
                "painted panels must respond to view direction: {} vs {}",
                toward_highlight[at],
                away_highlight[at]
            );
            let terrain_a = render(9 * 60, false, false, true, false, -1., -200., false, 30., 0);
            let terrain_b = render(9 * 60, false, false, true, false, -1., 200., false, 30., 0);
            assert!(
                (i32::from(terrain_a[at]) - i32::from(terrain_b[at])).abs() <= 2,
                "terrain must not acquire a painted-panel highlight"
            );
            let morning = render(7 * 60, false, false, true, true, -1., 0., false, 30., 0);
            let shaded_land = render(17 * 60, false, false, true, true, -1., 0., false, 30., 0);
            let noon_land = render(12 * 60, false, false, true, true, -1., 0., false, 30., 0);
            let night_land = render(22 * 60, false, false, true, true, -1., 0., false, 30., 0);
            let pixel = (128 * 256 + 128) * 4;
            assert!(
                shaded_land[pixel] <= 15,
                "low-sun back slope must darken from its preceding 20-level fill, got {}",
                shaded_land[pixel]
            );
            // Reference values from the response on this synthetic grey slope. The
            // morning value includes the low-sun ground lift (surface_lighting.wgsl
            // `ground_facing`, 2026-10-20); noon and night are unchanged by it.
            for (image, expected) in [(&morning, 144), (&noon_land, 56), (&night_land, 24)] {
                assert!(
                    (i32::from(image[pixel]) - expected).abs() <= 2,
                    "preserve exposed/day/night land brightness: got {} expected {expected}",
                    image[pixel]
                );
            }
            let before = render(7 * 60 + 1, false, false, true, true, -1., 0., false, 30., 0);
            let at = (128 * 256 + 128) * 4;
            assert!(
                morning[at] > morning[at + 2] + 10,
                "sun-facing grey surface must receive warm morning light: {:?}",
                &morning[at..at + 3]
            );
            for channel in 0..3 {
                assert!(
                    (i32::from(morning[at + channel]) - i32::from(before[at + channel])).abs() <= 2,
                    "a minute of sun movement must not step brightness"
                );
            }
        });
    }
}

#[cfg(test)]
#[path = "sim_renderer_graphics_tests.rs"]
mod graphics_tests;
