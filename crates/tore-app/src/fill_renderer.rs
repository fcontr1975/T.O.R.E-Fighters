//! City fill buildings (`tore_world::city`), drawn in three tiers set by the
//! Graphics screen (John, 2026-10-10; defaults 30 nm and 5 nm): full detail
//! (the kit piece with its textures) inside the building detail distance, a
//! blend band a quarter beyond it where each building cross-fades from its
//! far box to full detail, and the far box (its footprint in its brightened
//! mean colour, untextured, a cheap shader) out to the building distance.
//! Anything smaller than one pixel on screen is not drawn.
//!
//! Transitions fade (John, 2026-10-11: "let's try phase in transitions"):
//! a building fades in over the last [`FADE_SHARE`] of the building
//! distance and while it grows from one to two pixels on screen, and in the
//! blend band the full building is drawn whole with its box over it at the
//! remaining coverage. Coverage is a fixed ordered dither across each pixel's
//! samples (`fill_coverage` in terrain.wgsl), so a resolved 4x pixel blends
//! in quarter steps, nothing is sorted and a still view never shimmers. Only
//! the far boxes, and full detail in a view whose full detail reaches the
//! fade band, use the dithered fragment shaders; the full detail passes of
//! the main view keep their early depth test. The target display (camera
//! slot 4: the target window and the TV and gunsight views) draws every
//! building it sees at full detail, culled to its own narrow view, and fades
//! at its draw distance too.
//!
//! The per-view work is on the GPU: a compute pass culls every building
//! against the view and writes the full detail lists per piece and the far
//! box list, and indirect draws read them (one per piece and pass, plus one
//! for every far box). The buildings sit on the GPU in spatial blocks of up
//! to 256 ([`blocks`]); the CPU drops the blocks no drawn building can be
//! in and writes one small uniform and the visible block list per view.
//! Knocked-down buildings draw as a low stub (0.08 of their height, in the
//! cull shader); a knock-down rewrites only the changed records.
//! Buildings within the separate shadow distance cast their box into the
//! shadow maps at Med, High and Ultra.
use crate::gpu_setup::NamedSteps as _;

#[path = "fill_blocks.rs"]
pub mod blocks;

use crate::{AppResult, camera::Camera};
use std::{collections::BTreeMap, ops::Range, sync::Arc};
use tore_world::city::CityFill;

const NM: f64 = 6_076.115_49;

/// The tier distances, nautical miles, and the smallest drawn size, pixels.
/// The graphics options set them ([`Tiers::from_options`]);
/// `TORE_CITY_FILL_TIERS=FULL,BLEND,DRAW,MIN_PX[,DETAIL_PX]` overrides them
/// (a developer aid).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tiers {
    /// Full detail inside this.
    pub full_nm: f64,
    /// The blend band ends here; far boxes beyond.
    pub blend_nm: f64,
    /// Nothing beyond this.
    pub draw_nm: f64,
    /// Buildings smaller than this on screen are not drawn (fitted).
    pub min_px: f64,
    /// Buildings smaller than this on screen draw as far boxes even inside
    /// the full detail range (0: distance alone decides).
    pub detail_px: f64,
}

/// The blend band runs from the detail distance to this share beyond it
/// (fitted).
pub const BLEND_SHARE: f64 = 1.25;

impl Tiers {
    /// The tiers the Graphics screen's building distance and building
    /// detail choose: full detail inside the detail distance, a blend band
    /// a quarter beyond it, plain boxes out to the building distance.
    pub fn from_options(options: &crate::graphics::Options) -> Self {
        let draw = f64::from(options.building_distance_nm);
        let full = f64::from(options.building_detail_nm).min(draw);
        Self {
            full_nm: full,
            blend_nm: (full * BLEND_SHARE).min(draw),
            draw_nm: draw,
            min_px: 1.,
            detail_px: 0.,
        }
    }
    /// The developer override, when set and well formed.
    pub fn from_env() -> Option<Self> {
        let text = std::env::var("TORE_CITY_FILL_TIERS").ok()?;
        let v: Vec<f64> = text.split(',').filter_map(|s| s.parse().ok()).collect();
        (v.len() >= 4).then(|| Self {
            full_nm: v[0],
            blend_nm: v[1],
            draw_nm: v[2],
            min_px: v[3],
            detail_px: v.get(4).copied().unwrap_or(0.),
        })
    }
    /// The target display's tiers: everything it draws at full detail.
    pub fn target(self) -> Self {
        Self {
            full_nm: self.draw_nm,
            blend_nm: self.draw_nm,
            detail_px: 0.,
            ..self
        }
    }
}

/// The share of the building distance over which buildings fade in before
/// it (fitted, 2026-10-11): 4.5 nm of the default 30.
pub const FADE_SHARE: f64 = 0.15;
/// Initial fallback before the renderer applies the graphics settings.
/// Run-only measuring override: `TORE_CITY_FILL_SHADOW_NM`.
pub const SHADOW_NM: f64 = 0.5;
/// Views that may draw in one submission, each with its own uniform.
const SLOTS: usize = 8;

/// The unit far box: four walls and a roof, x and z from -1 to 1, y from 0
/// to 1, untextured.
fn unit_box() -> Vec<f32> {
    let corner = |x: f32, y: f32, z: f32| [x, y, z, 0., 0., -1., 0., 0., 0., 0.];
    let quads = [
        [[-1., 0., -1.], [1., 0., -1.], [1., 1., -1.], [-1., 1., -1.]],
        [[1., 0., -1.], [1., 0., 1.], [1., 1., 1.], [1., 1., -1.]],
        [[1., 0., 1.], [-1., 0., 1.], [-1., 1., 1.], [1., 1., 1.]],
        [[-1., 0., 1.], [-1., 0., -1.], [-1., 1., -1.], [-1., 1., 1.]],
        [[-1., 1., -1.], [1., 1., -1.], [1., 1., 1.], [-1., 1., 1.]],
    ];
    let mut out = Vec::new();
    for q in quads {
        for i in [0, 1, 2, 0, 2, 3] {
            out.extend(corner(q[i][0], q[i][1], q[i][2]));
        }
    }
    out
}

/// Brightening of the far box colour over the building's mean (fitted: the
/// close art's panel sheen and window texture read lighter than its mean).
pub const FAR_BRIGHTEN: f64 = 1.2;

/// The far box's palette index: the building's area-weighted mean colour,
/// brightened, matched to the nearest fixed shape colour (indices 0 to 191,
/// which the weather does not repaint).
fn far_color(sum: [f64; 4], palette: &[[u8; 3]; 256]) -> u8 {
    let n = sum[3].max(1e-9);
    let want: [f64; 3] = std::array::from_fn(|k| (sum[k] / n * FAR_BRIGHTEN).min(255.));
    (0..192u8)
        .min_by(|&a, &b| {
            let d = |i: u8| {
                let c = palette[usize::from(i)];
                (0..3)
                    .map(|k| (f64::from(c[k]) - want[k]).powi(2))
                    .sum::<f64>()
            };
            d(a).total_cmp(&d(b)).then(a.cmp(&b))
        })
        .unwrap_or(0)
}

/// The fill's art for one scenery: the kit's vertices, the piece table and
/// every building's fixed record for the cull pass.
pub struct FillArt {
    pub fill: Arc<CityFill>,
    /// Kit vertices, ten floats each, base-centred feet (x right, y up, z
    /// forward), then the unit far box.
    pub vertices: Vec<f32>,
    /// Each piece's vertex ranges: untextured triangles, then textured ones.
    pub ranges: Vec<[Range<u32>; 2]>,
    /// The unit far box's vertex range.
    pub box_range: Range<u32>,
    /// Per piece: its first slot in the full detail list, colour, half
    /// footprint, height and radius (the cull shader's `Piece`).
    piece_table: Vec<u32>,
    /// Per building, in GPU order ([`blocks::arrange`]): world base, piece,
    /// cosine, sine, hash and flags (the cull shader's `Inst`), eight words
    /// each.
    records: Vec<u32>,
    /// The building (layout index) at each GPU slot, and each building's
    /// slot.
    order: Vec<u32>,
    slot_of: Vec<u32>,
    /// Spatial runs of GPU slots with conservative bounds, culled per view
    /// before the per-building pass.
    blocks: Vec<blocks::Block>,
}

impl FillArt {
    /// Builds the kit vertices, appending its textures as static art pages
    /// after `first_layer` layers (the scenery's static art rule).
    pub fn build(
        resources: &BTreeMap<String, Vec<u8>>,
        fill: &Arc<CityFill>,
        pages: &mut Vec<u8>,
        first_layer: impl Fn(&Vec<u8>) -> usize,
        palette: &[[u8; 3]; 256],
    ) -> AppResult<Self> {
        let layout = &fill.layout;
        let mut shapes = BTreeMap::<String, tore_formats::shape::Shape>::new();
        let mut images = BTreeMap::<String, crate::static_art::Image>::new();
        let mut vertices = Vec::new();
        let mut ranges = Vec::new();
        let mut colors = Vec::new();
        // Each texture's mean colour through the palette (its transparent
        // and water texels left out), for the far boxes.
        let mut means = BTreeMap::<String, [f64; 3]>::new();
        for piece in &layout.kit.pieces {
            let mut sum = [0f64; 4];
            if !shapes.contains_key(&piece.shape) {
                let bytes = resources
                    .get(&piece.shape)
                    .ok_or_else(|| format!("city fill: missing {}", piece.shape))?;
                shapes.insert(
                    piece.shape.clone(),
                    tore_formats::shape::Shape::scenery(bytes)?,
                );
            }
            let shape = &shapes[&piece.shape];
            let mut solid = Vec::new();
            let mut textured = Vec::new();
            for &f in &piece.faces {
                let face = &shape.faces[f as usize];
                if face.positions.len() < 3 {
                    continue;
                }
                let image = if face.texture.is_empty() || face.uv.is_empty() {
                    None
                } else {
                    let name = face.texture.to_ascii_uppercase();
                    if !images.contains_key(&name) {
                        let pic = tore_formats::Pic::parse(
                            resources
                                .get(&name)
                                .ok_or_else(|| format!("city fill: missing texture {name}"))?,
                        )?;
                        let mut mean = [0f64; 4];
                        for (p, m) in pic.pixels.iter().zip(&pic.mask) {
                            if *m && *p != 255 {
                                let c = palette[usize::from(*p)];
                                for k in 0..3 {
                                    mean[k] += f64::from(c[k]);
                                }
                                mean[3] += 1.;
                            }
                        }
                        let n = mean[3].max(1.);
                        means.insert(name.clone(), [mean[0] / n, mean[1] / n, mean[2] / n]);
                        let first = first_layer(pages);
                        let image = crate::static_art::Image::append(&pic, pages, first)?;
                        images.insert(name.clone(), image);
                    }
                    images.get(&name)
                };
                // The face's share of the far box colour: its area in shape
                // units, for its colour or its texture's mean colour.
                let a = face.positions[0];
                let mut area = 0f64;
                for w in face.positions[1..].windows(2) {
                    let (b, c) = (w[0], w[1]);
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]].map(f64::from);
                    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]].map(f64::from);
                    let n = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() * 0.5;
                }
                let color = match image {
                    Some(_) => means[&face.texture.to_ascii_uppercase()],
                    None => palette[usize::from(face.colors[0])].map(f64::from),
                };
                for k in 0..3 {
                    sum[k] += color[k] * area;
                }
                sum[3] += area;
                for triangle in 1..face.positions.len() - 1 {
                    let mut points = [[0f32; 10]; 3];
                    for (slot, vertex_index) in [0, triangle, triangle + 1].into_iter().enumerate()
                    {
                        let p = face.positions[vertex_index];
                        let local = |axis: usize| {
                            ((f64::from(p[axis]) - piece.origin[axis]) * piece.scale) as f32
                        };
                        let uv = face.uv.get(vertex_index).copied().unwrap_or([0.0; 2]);
                        let uv = image.map_or([0., 0.], |image| {
                            [uv[0] + 0.5, image.height as f32 - 0.5 - uv[1]]
                        });
                        points[slot] = [
                            local(0),
                            local(2),
                            local(1),
                            uv[0],
                            uv[1],
                            -1.0,
                            0.0,
                            0.0,
                            0.0,
                            f32::from(face.colors[vertex_index]) + f32::from(face.fog as u8) * 256.,
                        ];
                    }
                    match image {
                        Some(image) => image.triangle(points, &mut textured, usize::MAX)?,
                        None => solid.extend(points.into_iter().flatten()),
                    }
                }
            }
            let start = (vertices.len() / 10) as u32;
            let middle = start + (solid.len() / 10) as u32;
            let end = middle + (textured.len() / 10) as u32;
            vertices.extend(solid);
            vertices.extend(textured);
            ranges.push([start..middle, middle..end]);
            colors.push(f32::from(far_color(sum, palette)));
        }
        let box_start = (vertices.len() / 10) as u32;
        vertices.extend(unit_box());
        let box_range = box_start..(vertices.len() / 10) as u32;
        let one = tore_world::surface::layout::trig::ONE as f64;
        // The full detail list holds, per piece, room for every building of
        // that piece.
        let mut per_piece = vec![0u32; layout.kit.pieces.len()];
        for i in &layout.instances {
            per_piece[usize::from(i.piece)] += 1;
        }
        let mut piece_table = Vec::with_capacity(per_piece.len() * 8);
        let mut offset = 0u32;
        for (n, p) in layout.kit.pieces.iter().enumerate() {
            let radius = ((p.half[0] * p.half[0] + p.half[1] * p.half[1]) as f64).sqrt() as f32;
            piece_table.extend([
                offset,
                colors[n].to_bits(),
                (p.half[0] as f32).to_bits(),
                (p.half[1] as f32).to_bits(),
                (p.height as f32).to_bits(),
                radius.to_bits(),
                0,
                0,
            ]);
            offset += per_piece[n];
        }
        // The GPU order: spatial blocks, each culled whole per view.
        let buildings: Vec<blocks::Building> = layout
            .instances
            .iter()
            .enumerate()
            .map(|(n, i)| {
                let piece = &piece_table[usize::from(i.piece) * 8..];
                blocks::Building {
                    pos: [f64::from(i.x), f64::from(fill.base[n]), f64::from(i.z)],
                    height: f64::from(f32::from_bits(piece[4])),
                    radius: f64::from(f32::from_bits(piece[5])),
                }
            })
            .collect();
        let (order, blocks) = blocks::arrange(&buildings);
        let mut slot_of = vec![0u32; order.len()];
        for (slot, &n) in order.iter().enumerate() {
            slot_of[n as usize] = slot as u32;
        }
        let mut records = Vec::with_capacity(layout.instances.len() * 8);
        for &n in &order {
            let n = n as usize;
            let i = &layout.instances[n];
            let heading = i32::from(i.heading);
            let cos = (tore_world::surface::layout::trig::cos(heading) as f64 / one) as f32;
            let sin = (tore_world::surface::layout::trig::sin(heading) as f64 / one) as f32;
            // A fixed per-building number in [0, 1): where in the blend band
            // it turns to full detail.
            let mut h = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
            let hash = (h >> 40) as f32 / (1u64 << 24) as f32;
            records.extend([
                (i.x as f32).to_bits(),
                fill.base[n].to_bits(),
                (i.z as f32).to_bits(),
                u32::from(i.piece),
                cos.to_bits(),
                sin.to_bits(),
                hash.to_bits(),
                blocks::flags(fill.hidden[n], false),
            ]);
        }
        Ok(Self {
            fill: Arc::clone(fill),
            vertices,
            ranges,
            box_range,
            piece_table,
            records,
            order,
            slot_of,
            blocks,
        })
    }

    /// Kit vertex bytes on the GPU.
    pub fn vertex_bytes(&self) -> u64 {
        self.vertices.len() as u64 * 4
    }

    /// Buildings in the art.
    pub fn len(&self) -> usize {
        self.records.len() / 8
    }
}

/// What a view drew, read back from the GPU on request (a measuring aid).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Buildings drawn at full detail (inside the blend band's far edge).
    pub full: usize,
    /// Buildings drawn as far boxes.
    pub boxes: usize,
}

pub struct FillRenderer {
    art: Arc<FillArt>,
    /// The Graphics screen's tiers, and the developer override.
    tiers: Tiers,
    override_tiers: Option<Tiers>,
    vertices: wgpu::Buffer,
    records: wgpu::Buffer,
    counts: wgpu::Buffer,
    full: wgpu::Buffer,
    boxes: wgpu::Buffer,
    /// Every drawn building's box, for the shadow maps.
    shadows: wgpu::Buffer,
    args: wgpu::Buffer,
    readback: wgpu::Buffer,
    params: Vec<wgpu::Buffer>,
    /// Per slot: the view's visible blocks (first and end GPU slot each).
    block_lists: Vec<wgpu::Buffer>,
    binds: Vec<wgpu::BindGroup>,
    slot: usize,
    /// The bind group of the last prepared view, and whether it drew.
    prepared: bool,
    /// The fill version and flattened set the GPU records hold (none
    /// before the first view).
    flattened_version: Option<u64>,
    uploaded: Vec<u32>,
    /// Scratch: this view's visible blocks.
    visible: Vec<u32>,
    /// Coarse block culling on (`TORE_CITY_FILL_BLOCKS=0` dispatches every
    /// block, a comparison aid).
    block_cull: bool,
    cull: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    pipelines: Pipelines,
    /// The prepared view's full detail reaches the fade band: it draws with
    /// the fading pipelines.
    fading: bool,
    /// One shadow caster pipeline per shadow cascade.
    shadow: [wgpu::RenderPipeline; 3],
    /// Buildings inside this cast shadows, nautical miles.
    shadow_nm: f64,
}

/// The box shadow casters, one pipeline per cascade (the cascade is the
/// vertex entry, since the instance index is the building).
fn shadow_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    lighting: &crate::surface_lighting::SurfaceLighting,
) -> [wgpu::RenderPipeline; 3] {
    let buffers = [
        wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3],
        },
        wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![5=>Float32x4,6=>Float32x4,7=>Float32x4],
        },
    ];
    std::array::from_fn(|cascade| {
        let entry = format!("fill_shadow_vertex{cascade}");
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("City fill shadow casters"),
            layout: Some(&lighting.caster_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(&entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("shadow_fragment"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            // The object casters' bias (surface_lighting.rs).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.,
                    clamp: 0.,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        })
    })
}

/// The city fill's render pipelines: full detail solid art and texture
/// details, each opaque and fading, and the far boxes.
struct Pipelines {
    solid: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    solid_fade: wgpu::RenderPipeline,
    decal_fade: wgpu::RenderPipeline,
    far: wgpu::RenderPipeline,
}

/// Depth bias of the far boxes, in reversed-depth steps: in the blend band a
/// box covers its full detail building (and its texture details, one step
/// in front), whose walls may lie in the box's own planes.
const FAR_BIAS: i32 = 3;

fn pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    material_layout: &wgpu::BindGroupLayout,
    lighting_layout: &wgpu::BindGroupLayout,
) -> Pipelines {
    let constants = [("FILL_SAMPLES", f64::from(samples))];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("City fill layout"),
        bind_group_layouts: &[material_layout, lighting_layout],
        push_constant_ranges: &[],
    });
    let vertex = wgpu::VertexBufferLayout {
        array_stride: 40,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Float32,3=>Float32x3,4=>Float32],
    };
    let near = [
        vertex.clone(),
        wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![5=>Float32x4,6=>Float32x4],
        },
    ];
    let far = [
        vertex,
        wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![5=>Float32x4,6=>Float32x4,7=>Float32x4],
        },
    ];
    let make = |label: &str,
                entry: &str,
                buffers: &[wgpu::VertexBufferLayout<'_>],
                fragment: &str,
                bias: i32| {
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
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
            // The static airport passes' depth rules: solid art, then
            // coplanar texture details one reversed-depth ULP in front. Each
            // pass draws only its own triangles, and the opaque solid pass
            // has no discard, so it keeps the early depth test.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: bias,
                    ..Default::default()
                },
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        })
    };
    Pipelines {
        solid: make(
            "City fill surfaces",
            "fill_vertex",
            &near,
            "fill_solid_fragment",
            0,
        ),
        decal: make(
            "City fill texture details",
            "fill_vertex",
            &near,
            "airport_fragment",
            1,
        ),
        solid_fade: make(
            "City fill surfaces fading in",
            "fill_vertex",
            &near,
            "fill_solid_fade_fragment",
            0,
        ),
        decal_fade: make(
            "City fill texture details fading in",
            "fill_vertex",
            &near,
            "fill_decal_fade_fragment",
            1,
        ),
        far: make(
            "City fill far boxes",
            "fill_box_vertex",
            &far,
            "fill_far_fragment",
            FAR_BIAS,
        ),
    }
}

/// Words in the cull uniform.
const PARAM_WORDS: usize = 32;

impl FillRenderer {
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material_layout: &wgpu::BindGroupLayout,
        lighting: &crate::surface_lighting::SurfaceLighting,
        art: &Arc<FillArt>,
    ) -> Self {
        let lighting_layout = &lighting.layout;
        use wgpu::util::DeviceExt;
        let storage = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let words = |w: &[u32]| w.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let n = art.len().max(1) as u64;
        let pieces = art.ranges.len();
        let vertices = storage(
            "City fill kit vertices",
            &crate::sim_renderer::bytes(&art.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let records = storage(
            "City fill buildings",
            &words(&art.records),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let piece_table = storage(
            "City fill pieces",
            &words(&art.piece_table),
            wgpu::BufferUsages::STORAGE,
        );
        let counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("City fill counts"),
            size: (pieces as u64 + 2) * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let full = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("City fill full detail list"),
            size: n * 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let boxes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("City fill far box list"),
            size: n * 48,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let shadows = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("City fill shadow boxes"),
            size: n * 48,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        // Indirect draws: each piece's solid and textured ranges, then the
        // far boxes and the shadow boxes; instance counts start at zero.
        let mut args = Vec::with_capacity((2 * pieces + 1) * 4);
        // First instance stays 0 (a non-zero one needs a device feature):
        // each piece's draws bind the full detail list at that piece's slot.
        for range in &art.ranges {
            for r in range {
                args.extend([r.end - r.start, 0, r.start, 0]);
            }
        }
        // The far boxes, then the shadow boxes.
        for _ in 0..2 {
            args.extend([
                art.box_range.end - art.box_range.start,
                0,
                art.box_range.start,
                0,
            ]);
        }
        let args = storage(
            "City fill indirect draws",
            &words(&args),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("City fill count readback"),
            size: (pieces as u64 + 2) * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let module = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("City fill cull"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fill_cull.wgsl").into()),
        });
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let write = wgpu::BufferBindingType::Storage { read_only: false };
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("City fill cull bindings"),
            entries: &[
                entry(0, read),
                entry(1, read),
                entry(2, wgpu::BufferBindingType::Uniform),
                entry(3, write),
                entry(4, write),
                entry(5, write),
                entry(6, write),
                entry(7, write),
                entry(8, read),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("City fill cull layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let compute = |entry: &str| {
            device.named_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let cull = compute("cull");
        let finish = compute("finish");
        let params: Vec<wgpu::Buffer> = (0..SLOTS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("City fill cull view"),
                    size: (PARAM_WORDS * 4) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let block_lists: Vec<wgpu::Buffer> = (0..SLOTS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("City fill visible blocks"),
                    size: art.blocks.len().max(1) as u64 * 8,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let binds = params
            .iter()
            .zip(&block_lists)
            .map(|(p, list)| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("City fill cull bindings"),
                    layout: &bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: records.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: piece_table.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: p.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: counts.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: full.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: boxes.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: args.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: shadows.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: list.as_entire_binding(),
                        },
                    ],
                })
            })
            .collect();
        let pipelines = pipelines(
            device,
            shader,
            format,
            samples,
            material_layout,
            lighting_layout,
        );
        let shadow = shadow_pipelines(device, shader, lighting);
        Self {
            art: Arc::clone(art),
            tiers: Tiers::from_options(&crate::graphics::Options::default()),
            override_tiers: Tiers::from_env(),
            vertices,
            records,
            counts,
            full,
            boxes,
            shadows,
            args,
            readback,
            params,
            block_lists,
            binds,
            slot: 0,
            prepared: false,
            flattened_version: None,
            uploaded: Vec::new(),
            visible: Vec::new(),
            block_cull: std::env::var("TORE_CITY_FILL_BLOCKS").as_deref() != Ok("0"),
            cull,
            finish,
            pipelines,
            fading: false,
            shadow,
            shadow_nm: std::env::var("TORE_CITY_FILL_SHADOW_NM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(SHADOW_NM),
        }
    }

    /// Takes the Graphics screen's building distances.
    pub fn set_options(&mut self, options: &crate::graphics::Options) {
        self.tiers = Tiers::from_options(options);
    }

    /// Takes the resolved building shadow distance
    /// (`TORE_CITY_FILL_SHADOW_NM` still overrides it).
    pub fn set_shadows(&mut self, shadows: &crate::surface_lighting::Settings) {
        self.shadow_nm = std::env::var("TORE_CITY_FILL_SHADOW_NM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(shadows.buildings_nm);
    }

    /// Replaces the tiers for every view until cleared (a measuring aid, as
    /// `TORE_CITY_FILL_TIERS`).
    pub fn set_tiers(&mut self, tiers: Option<Tiers>) {
        self.override_tiers = tiers;
    }

    /// Rebuilds the render pipelines for a new sample count.
    pub fn rebuild(
        &mut self,
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material_layout: &wgpu::BindGroupLayout,
        lighting: &crate::surface_lighting::SurfaceLighting,
    ) {
        self.pipelines = pipelines(
            device,
            shader,
            format,
            samples,
            material_layout,
            &lighting.layout,
        );
    }

    /// GPU bytes: kit, building records, the two lists, counts and draws.
    pub fn gpu_bytes(&self) -> u64 {
        self.vertices.size()
            + self.records.size()
            + self.full.size()
            + self.boxes.size()
            + self.shadows.size()
            + self.counts.size()
            + self.args.size()
    }

    /// Records this view's cull: clears the counts, culls every building and
    /// sets the indirect draws. `size` is the world pass's pixel size.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        origin: [f64; 3],
        size: [u32; 2],
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        let fill = &self.art.fill;
        // Knocked-down buildings: rewrite only the records whose flags
        // changed since the last upload.
        let version = fill.version();
        if self.flattened_version != Some(version) {
            self.flattened_version = Some(version);
            let now = fill.flattened();
            let art = &self.art;
            for (offset, bytes) in blocks::flag_writes(
                &art.records,
                &art.slot_of,
                &self.uploaded,
                &now,
                |n| fill.is_flattened(n),
                &art.order,
            ) {
                queue.write_buffer(&self.records, offset, &bytes);
            }
            self.uploaded = now;
        }
        let (sy, cy) = camera.yaw.sin_cos();
        let (sp, cp) = camera.pitch.sin_cos();
        let (sr, cr) = camera.roll.sin_cos();
        let right = [cy * cr - sy * sp * sr, cp * sr, -sy * cr - cy * sp * sr];
        let up = [-cy * sr - sy * sp * cr, cp * cr, sy * sr - cy * sp * cr];
        let forward = [sy * cp, sp, cy * cp];
        let height = (size[1] as f32 * camera.view_fraction).max(1.);
        let aspect = size[0] as f32 / height;
        let f = 1.732_050_8 * camera.zoom.max(0.01);
        let zoom = f64::from(camera.zoom).clamp(1., 4.);
        let eye: [f32; 3] = std::array::from_fn(|i| (camera.position[i] - origin[i]) as f32);
        let tiers = self.override_tiers.unwrap_or(self.tiers);
        let t = if camera.weather_slot == 4 {
            tiers.target()
        } else {
            tiers
        };
        let mut p = [0f32; PARAM_WORDS];
        p[0..3].copy_from_slice(&origin.map(|v| v as f32));
        p[4..7].copy_from_slice(&eye);
        p[8..11].copy_from_slice(&forward);
        p[12..15].copy_from_slice(&right);
        p[16..19].copy_from_slice(&up);
        p[20] = (t.full_nm * NM * zoom) as f32;
        p[21] = (t.blend_nm * NM * zoom) as f32;
        let draw = t.draw_nm * NM * zoom;
        let fade = (draw * FADE_SHARE).max(1.);
        p[22] = draw as f32;
        p[23] = fade as f32;
        self.fading = t.blend_nm * NM * zoom > draw - fade;
        p[24] = aspect / f * 1.05;
        p[25] = 1. / f * 1.05;
        p[26] = f * height * 0.5;
        p[27] = t.min_px as f32;
        let mut bytes: Vec<u8> = p.iter().flat_map(|v| v.to_le_bytes()).collect();
        bytes[28 * 4..29 * 4].copy_from_slice(&(self.art.len() as u32).to_le_bytes());
        bytes[29 * 4..30 * 4].copy_from_slice(&(self.art.ranges.len() as u32).to_le_bytes());
        bytes[30 * 4..31 * 4].copy_from_slice(&(t.detail_px as f32).to_le_bytes());
        bytes[31 * 4..32 * 4].copy_from_slice(&((self.shadow_nm * NM) as f32).to_le_bytes());
        self.slot = (self.slot + 1) % SLOTS;
        queue.write_buffer(&self.params[self.slot], 0, &bytes);
        // The blocks that may hold a drawn building, from the same view
        // values the shader reads.
        let f = |r: std::ops::Range<usize>| -> [f64; 3] {
            std::array::from_fn(|k| f64::from(p[r.start + k]))
        };
        let view = blocks::View {
            eye: camera.position,
            forward: f(8..11),
            right: f(12..15),
            up: f(16..19),
            draw: f64::from(p[22]),
            view: std::array::from_fn(|k| f64::from(p[24 + k])),
        };
        self.visible.clear();
        for block in &self.art.blocks {
            if !self.block_cull || blocks::visible(block, &view) {
                self.visible
                    .extend([block.records.start, block.records.end]);
            }
        }
        let groups = (self.visible.len() / 2) as u32;
        if groups > 0 {
            let bytes: Vec<u8> = self.visible.iter().flat_map(|v| v.to_le_bytes()).collect();
            queue.write_buffer(&self.block_lists[self.slot], 0, &bytes);
        }
        encoder.clear_buffer(&self.counts, 0, None);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("City fill cull"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.binds[self.slot], &[]);
        if groups > 0 {
            pass.set_pipeline(&self.cull);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        pass.set_pipeline(&self.finish);
        pass.dispatch_workgroups((self.art.ranges.len() as u32 + 2).div_ceil(64), 1, 1);
        drop(pass);
        self.prepared = true;
    }

    /// Casts the prepared view's buildings into the shadow maps, as boxes.
    pub fn draw_shadows(
        &self,
        lighting: &crate::surface_lighting::SurfaceLighting,
        encoder: &mut wgpu::CommandEncoder,
        bind: &wgpu::BindGroup,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        if !self.prepared || self.shadow_nm <= 0. {
            return;
        }
        let offset = (2 * self.art.ranges.len() as u64 + 1) * 16;
        lighting.draw_more(encoder, |pass, cascade| {
            pass.set_pipeline(&self.shadow[cascade]);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_vertex_buffer(1, self.shadows.slice(..));
            crate::frame_log::count(crate::frame_log::Count::CityDraws, 1);
            pass.draw_indirect(&self.args, offset);
        });
    }

    /// Draws the prepared view: far boxes, then each piece's solid art and
    /// texture details, all indirect.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, bind: &'a wgpu::BindGroup) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        if !self.prepared {
            return;
        }
        pass.set_bind_group(0, bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        let pieces = self.art.ranges.len() as u64;
        let p = &self.pipelines;
        pass.set_pipeline(&p.far);
        pass.set_vertex_buffer(1, self.boxes.slice(..));
        crate::frame_log::count(crate::frame_log::Count::CityDraws, 1);
        pass.draw_indirect(&self.args, 2 * pieces * 16);
        let passes = if self.fading {
            [&p.solid_fade, &p.decal_fade]
        } else {
            [&p.solid, &p.decal]
        };
        for (kind, pipeline) in passes.into_iter().enumerate() {
            pass.set_pipeline(pipeline);
            for (p, range) in self.art.ranges.iter().enumerate() {
                let first = u64::from(self.art.piece_table[p * 8]) * 32;
                if !range[kind].is_empty() && first < self.full.size() {
                    pass.set_vertex_buffer(1, self.full.slice(first..));
                    crate::frame_log::count(crate::frame_log::Count::CityDraws, 1);
                    pass.draw_indirect(&self.args, (2 * p as u64 + kind as u64) * 16);
                }
            }
        }
    }

    /// Reads back what the last view drew (waits for the GPU; a measuring
    /// aid).
    pub fn stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> AppResult<Stats> {
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.counts, 0, &self.readback, 0, self.counts.size());
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        })?;
        rx.recv()??;
        let counts: Vec<u32> = self
            .readback
            .slice(..)
            .get_mapped_range()
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        self.readback.unmap();
        let pieces = self.art.ranges.len();
        Ok(Stats {
            full: counts[..pieces].iter().map(|&c| c as usize).sum(),
            boxes: counts[pieces] as usize,
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
pub(crate) mod tests {
    use super::*;
    use tore_world::city::{self, CityFill};

    /// One box-shaped kit piece, `half` feet each way and `height` tall, at
    /// each `(x, z)` on flat ground, its full detail mesh the far box.
    pub(crate) fn synthetic(at: &[(i32, i32)], half: i64, height: i64) -> FillArt {
        let piece = city::kit::Piece {
            source: "T.OT".into(),
            shape: "T.SH".into(),
            faces: Vec::new(),
            origin: [0.; 3],
            scale: 1.,
            half: [half, half],
            height,
            class: city::kit::Class::Tower,
            cut: false,
        };
        let layout = city::Layout {
            layout: "TEST.MM".into(),
            map: city::mask::LightMap::default(),
            kit: city::kit::Kit {
                pieces: vec![piece],
            },
            cities: Vec::new(),
            instances: at
                .iter()
                .map(|&(x, z)| city::Instance {
                    piece: 0,
                    x,
                    z,
                    heading: 0,
                    core: true,
                    city: 0,
                })
                .collect(),
            candidates: at.len(),
            inputs: Vec::new(),
        };
        let fill = Arc::new(CityFill::assemble(
            Arc::new(layout),
            vec![0.; at.len()],
            vec![false; at.len()],
        ));
        let mut vertices = unit_box();
        for v in vertices.chunks_exact_mut(10) {
            v[0] *= half as f32;
            v[1] *= height as f32;
            v[2] *= half as f32;
            v[9] = 40.;
        }
        let end = (vertices.len() / 10) as u32;
        vertices.extend(unit_box());
        let box_range = end..(vertices.len() / 10) as u32;
        let radius = ((2 * half * half) as f64).sqrt() as f32;
        let piece_table = vec![
            0,
            40f32.to_bits(),
            (half as f32).to_bits(),
            (half as f32).to_bits(),
            (height as f32).to_bits(),
            radius.to_bits(),
            0,
            0,
        ];
        let buildings: Vec<blocks::Building> = at
            .iter()
            .map(|&(x, z)| blocks::Building {
                pos: [f64::from(x), 0., f64::from(z)],
                height: height as f64,
                radius: f64::from(radius),
            })
            .collect();
        let (order, blocks) = blocks::arrange(&buildings);
        let mut slot_of = vec![0; order.len()];
        for (slot, &n) in order.iter().enumerate() {
            slot_of[n as usize] = slot as u32;
        }
        let mut records = Vec::new();
        for &n in &order {
            let n = n as usize;
            let (x, z) = at[n];
            records.extend([
                (x as f32).to_bits(),
                0f32.to_bits(),
                (z as f32).to_bits(),
                0,
                1f32.to_bits(),
                0f32.to_bits(),
                (n as f32 / at.len() as f32).to_bits(),
                0,
            ]);
        }
        FillArt {
            fill,
            vertices,
            ranges: vec![[0..end, end..end]],
            box_range,
            piece_table,
            records,
            order,
            slot_of,
            blocks,
        }
    }

    /// The tiers on the GPU (production cull shader): a building inside the
    /// detail distance draws at full detail only, one in the blend band both
    /// (its box dithered over it), one beyond as its box, one fading in at
    /// the building distance or by size as its box, and none past the
    /// building distance. The target display's view draws full detail out
    /// to its draw distance, with its fading pipelines.
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_buildings_fade_in_and_cross_fade_by_tier() {
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
            let nm = |n: f64| (n * NM) as i32;
            // 1,131 ft across its diagonal: at 256 pixels high it is about
            // 14 pixels at 3 nm and 1.4 at 29 nm (between one and two:
            // fading in by size).
            let at = [
                (0, nm(3.)),
                (0, nm(5.6)),
                (0, nm(8.)),
                (0, nm(29.)),
                (0, nm(31.)),
            ];
            let art = Arc::new(synthetic(&at, 400, 1_000));
            let world = tore_world::test_support::terrain();
            let mut scenery = crate::scenery::tests::scenery();
            scenery.fill = Some(Arc::clone(&art));
            // A zero-sized vertex buffer is not portable.
            if scenery.vertices.is_empty() {
                scenery.vertices = vec![0.; 30];
            }
            // With and without anti-aliasing (the coverage's sample count).
            for samples in [1, 4] {
                let mut renderer = crate::sim_renderer::SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::default(),
                    samples,
                );
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
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let mut stats = |slot: usize| {
                    let mut camera = Camera::new();
                    camera.position = [0., 500., 0.];
                    camera.weather_slot = slot;
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
                    queue.submit([encoder.finish()]);
                    let fill = renderer.fill.as_ref().unwrap();
                    (fill.stats(&device, &queue).unwrap(), fill.fading)
                };
                let (main, fading) = stats(0);
                // Full: 3 and 5.6 nm. Boxes: 5.6 (over its full detail), 8 and
                // 29 nm (fading in). 31 nm: past the building distance.
                assert_eq!((main.full, main.boxes), (2, 3));
                assert!(
                    !fading,
                    "the main view's full detail keeps its early depth test"
                );
                let (target, fading) = stats(4);
                // Full detail to the draw distance but 29 nm, still a box while
                // under two pixels.
                assert_eq!((target.full, target.boxes), (3, 1));
                assert!(fading, "the target display's full detail fades in");
            }
        });
    }

    /// Knock-downs and restores on a live renderer (sparse flag writes)
    /// draw exactly what a renderer built fresh for that state draws, and
    /// the coarse block cull draws exactly what dispatching every block
    /// draws.
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_sparse_knock_downs_and_block_cull_match_full_work() {
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
            // A 40 x 40 town 400 ft apart, ahead of the camera, plus a far
            // ring of blocks behind and beside it.
            let mut at = Vec::new();
            for i in 0..40 {
                for j in 0..40 {
                    at.push((i * 400 - 8_000, j * 400 + 1_000));
                }
            }
            for k in 0..64 {
                let a = f64::from(k) / 64. * std::f64::consts::TAU;
                at.push(((a.sin() * 60_000.) as i32, (a.cos() * 60_000.) as i32));
            }
            let world = tore_world::test_support::terrain();
            let size = 256u32;
            let image = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(size * size * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let make = |art: &Arc<FillArt>| {
                let mut scenery = crate::scenery::tests::scenery();
                scenery.fill = Some(Arc::clone(art));
                if scenery.vertices.is_empty() {
                    scenery.vertices = vec![0.; 30];
                }
                let renderer = crate::sim_renderer::SimRenderer::new(
                    &device,
                    &queue,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &scenery,
                    crate::graphics::Options::default(),
                    4,
                );
                (renderer, scenery)
            };
            let mut camera = Camera::new();
            camera.position = [0., 600., -2_000.];
            camera.pitch = -0.2;
            let picture = |renderer: &mut crate::sim_renderer::SimRenderer,
                           scenery: &crate::scenery::Scenery| {
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.draw(
                    &device,
                    &queue,
                    &mut encoder,
                    &image.create_view(&Default::default()),
                    [size, size],
                    &camera,
                    &world,
                    scenery,
                );
                encoder.copy_texture_to_buffer(
                    image.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(size * 4),
                            rows_per_image: Some(size),
                        },
                    },
                    image.size(),
                );
                queue.submit([encoder.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                let pixels = read.slice(..).get_mapped_range().to_vec();
                read.unmap();
                let stats = renderer
                    .fill
                    .as_ref()
                    .unwrap()
                    .stats(&device, &queue)
                    .unwrap();
                (pixels, stats.full, stats.boxes)
            };
            let live_art = Arc::new(synthetic(&at, 60, 300));
            let (mut live, live_scenery) = make(&live_art);
            let culled = live.fill.as_ref().unwrap();
            assert!(culled.block_cull);
            let fill = Arc::clone(&live_art.fill);
            let states: [&[u32]; 4] = [&[], &[3, 4, 5, 41, 42, 900, 1_599], &[], &[7, 8]];
            for (step, ids) in states.iter().enumerate() {
                fill.set_flattened(ids);
                let now = picture(&mut live, &live_scenery);
                // A fresh renderer for the same state, every block
                // dispatched.
                let fresh_art = Arc::new(synthetic(&at, 60, 300));
                fresh_art.fill.set_flattened(ids);
                let (mut fresh, fresh_scenery) = make(&fresh_art);
                fresh.fill.as_mut().unwrap().block_cull = false;
                let want = picture(&mut fresh, &fresh_scenery);
                assert_eq!((now.1, now.2), (want.1, want.2), "step {step}");
                assert!(now.0 == want.0, "step {step}: pictures differ");
                assert!(now.1 + now.2 > 0);
            }
            // Knocked down: the stubs change the picture.
            fill.set_flattened(&[]);
            let whole = picture(&mut live, &live_scenery);
            fill.set_flattened(&(0..1_600).collect::<Vec<u32>>());
            let flat = picture(&mut live, &live_scenery);
            assert!(whole.0 != flat.0);
        });
    }
}
