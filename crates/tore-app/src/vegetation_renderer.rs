//! Cached vegetation impostors, legacy tufts and GPU-generated grass blades. Source shapes
//! are rasterized once into indexed atlas pages. GPU culling compacts all
//! visible species into one instance stream and one indirect draw. Not collidable.
use crate::gpu_setup::NamedSteps as _;

use crate::vegetation::{self, Inst, Patch, Placement, Species};
use crate::vegetation_shapes::{FLOATS, Library, Look};
use crate::{camera::Camera, terrain::Terrain};
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[path = "vegetation_stream.rs"]
pub(crate) mod stream;

#[path = "vegetation_popin.rs"]
pub(crate) mod popin;

const INST_STRIDE: u64 = 64;
const DRAWN_STRIDE: u64 = 96;
/// A tree's shadow caster is its first card: one quad.
const SHADOW_CARD_VERTICES: u32 = 6;
const PATCH_STRIDE: u64 = 64;
/// Grass distance rings: outer edge as a share of the grass radius, and
/// the share of a patch's full tufts drawn in it.
const RINGS: [(f32, f32); 4] = [(0.14, 1.), (0.32, 0.5), (0.6, 0.25), (1.0, 0.125)];

/// Which camera a world draw is for. Only the main view streams vegetation
/// (optimization S10): the mirror, whose eye is about 12 ft from the main
/// one, draws the main view's uploaded chunks and reach read-only, and so
/// does a camera panel near the main eye. A panel far from it (the target
/// camera framing a distant target) streams its own small circle, keyed by
/// its page, so it never moves the main view's circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Main,
    Mirror,
    /// An instrument camera panel, by page.
    Panel(u8),
}

/// Where a view's vegetation comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The main stream, which this view moves.
    Stream,
    /// The main stream as it stands, read-only.
    Share,
    /// The panel's own small stream.
    Panel(u8),
}

/// A panel this close to the main view's eye, feet, draws the main stream:
/// the stream keeps a two-chunk guard ring beyond its drawn circle.
pub(crate) const PANEL_SHARE_FT: f64 = 2. * vegetation::CHUNK as f64;
/// A panel's own stream: drawn circle and full-density circle at most, feet
/// (fitted, agent decision 2026-10-19: a 138 by 114 pixel picture of a
/// target at most a mile from its camera).
pub(crate) const PANEL_DRAW_FT: f32 = 2. * vegetation::NM as f32;
pub(crate) const PANEL_FULL_FT: f32 = vegetation::NM as f32;
/// A panel stream not drawn for this long is dropped with its GPU buffers.
pub(crate) const PANEL_IDLE: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) fn route(kind: ViewKind, eye: [f64; 2], main_eye: Option<[f64; 2]>) -> Route {
    match kind {
        ViewKind::Main => Route::Stream,
        ViewKind::Mirror => Route::Share,
        ViewKind::Panel(page) => {
            if main_eye.is_some_and(|m| (m[0] - eye[0]).hypot(m[1] - eye[1]) <= PANEL_SHARE_FT) {
                Route::Share
            } else {
                Route::Panel(page)
            }
        }
    }
}

/// A panel stream's circles and budget for the main stream's `full` and
/// `draw` (feet) and `capacity` (objects): the same objects per chunk as the
/// main stream, so it is as dense, over the smaller ring. Returns the full
/// and draw distances, the capacity and the spare pages.
pub(crate) fn panel_shape(full: f32, draw: f32, capacity: usize) -> (f32, f32, usize, usize) {
    let panel_draw = draw.min(PANEL_DRAW_FT);
    let panel_full = full.min(PANEL_FULL_FT).min(panel_draw);
    let quota = capacity / vegetation::ring_chunks(draw).max(1);
    let chunks = vegetation::ring_chunks(panel_draw);
    let objects = (quota * chunks)
        .next_multiple_of(stream::PAGE)
        .clamp(stream::PAGE, capacity.max(stream::PAGE));
    // One partial page per chunk, for the published and the staged selection.
    (panel_full, panel_draw, objects, 2 * chunks)
}

/// A stream with its GPU instance pages, page table and cull bindings.
struct GpuStream {
    cpu: stream::Stream,
    instances: wgpu::Buffer,
    chunks: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

/// A far camera panel's own stream.
struct PanelStream {
    page: u8,
    full: f32,
    draw: f32,
    capacity: usize,
    used: std::time::Instant,
    gpu: GpuStream,
}

pub struct Renderer {
    atlas: Arc<vegetation::Atlas>,
    epoch: u64,
    main: GpuStream,
    panels: Vec<PanelStream>,
    /// The stream the last `prepare` drew from (`None` for the main one),
    /// with its object count, page count and reach.
    view: (Option<u8>, u32, u32, f32),
    depth_pipeline: wgpu::RenderPipeline,
    /// The tree shadow casters, one pipeline per cascade (the cascade is
    /// the vertex entry, since the instance index is the tree).
    shadow: [wgpu::RenderPipeline; 3],
    /// Trees nearer than this cast shadows, feet (0 none), from the shadow
    /// quality.
    trees_ft: f32,
    library: Arc<Library>,
    vertices: wgpu::Buffer,
    drawn: wgpu::Buffer,
    shadow_drawn: wgpu::Buffer,
    counts: wgpu::Buffer,
    args: wgpu::Buffer,
    params: wgpu::Buffer,
    meshes: wgpu::Buffer,
    mesh_dirty: bool,
    patches: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    /// Objects the instance buffers hold: the density's budget.
    capacity: usize,
    cull: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    pipeline: wgpu::RenderPipeline,
    grass_pipeline: wgpu::RenderPipeline,
    blades: vegetation::grass::gpu::Blades,
    grass_mode: crate::graphics::Grass,
    grass_density: crate::graphics::VegDensity,
    grass_suppressed: bool,
    repeat_frame: bool,
    /// The page entries the last view sent to the cull pass.
    visible_pages: Vec<u32>,
    /// Coarse page culling per view (`TORE_VEG_PAGE_CULL=0` sends every
    /// page, a comparison aid).
    page_cull: bool,
    /// Per ring: patch range and tufts per patch.
    rings: Vec<(std::ops::Range<u32>, u32)>,
    prepared: bool,
    suppressed: bool,
    settle: bool,
    draw_nm: f32,
    detail_nm: f32,
    density: f32,
    grass_radius: f32,
    tufts: u32,
    cards: crate::graphics::VegCards,
}

/// Cache work done while preparing one frame. Zeros when vegetation is hidden.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheFrame {
    pub object_chunks: u64,
    pub grass_chunks: u64,
    pub build_ms: f64,
    pub pending: bool,
    pub submit_ms: f64,
    pub packed_chunks: u64,
    pub uploaded_bytes: usize,
    pub upload_chunks: usize,
    pub upload_bytes: usize,
    pub worker_pending: bool,
    pub range_pending: bool,
    pub available: f32,
    pub visible: f32,
}

impl CacheFrame {
    pub const CSV_HEADER: &str = "frame,object_builds,grass_builds,packed_chunks,uploaded_bytes,upload_chunks,upload_bytes,worker_pending,range_pending,available_ft,visible_ft,pending\n";

    pub fn write_csv(&self, out: &mut impl std::io::Write, frame: usize) -> std::io::Result<()> {
        writeln!(
            out,
            "{frame},{},{},{},{},{},{},{},{},{:.3},{:.3},{}",
            self.object_chunks,
            self.grass_chunks,
            self.packed_chunks,
            self.uploaded_bytes,
            self.upload_chunks,
            self.upload_bytes,
            self.worker_pending,
            self.range_pending,
            self.available,
            self.visible,
            self.pending
        )
    }
}

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material: &wgpu::BindGroupLayout,
        lighting: &crate::surface_lighting::SurfaceLighting,
        atlas: Arc<vegetation::Atlas>,
        library: Arc<Library>,
        queue: &wgpu::Queue,
        grass_map: &vegetation::grass::Map,
    ) -> Self {
        let mesh_count = library.meshes.len();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vegetation meshes"),
            contents: &bytes_of_f32(&library.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let capacity = object_budget(crate::graphics::VegDensity::High);
        let counts = buffer(
            "Vegetation counts",
            (mesh_count as u64 * 2).max(1) * 4,
            storage,
        );
        let mut args = vec![0u32; mesh_count * 8];
        for (m, mesh) in library.meshes.iter().enumerate() {
            for list in 0..2 {
                let at = (list * mesh_count + m) * 4;
                args[at] = mesh.range.end - mesh.range.start;
                args[at + 2] = mesh.range.start;
            }
        }
        let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vegetation draws"),
            contents: &bytes_of_u32(&args),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
        });
        let params = buffer(
            "Vegetation cull params",
            128,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let meshes = buffer(
            "Vegetation mesh table",
            (mesh_count as u64).max(1) * 48,
            storage,
        );
        let patches = buffer(
            "Grass patches",
            vegetation::MAX_PATCHES as u64 * PATCH_STRIDE,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Vegetation cull layout"),
            entries: &[
                storage_entry(0, true),
                uniform_entry(1),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, false),
                storage_entry(5, false),
                storage_entry(6, true),
                storage_entry(7, true),
            ],
        });
        let (drawn, shadow_drawn) = drawn_buffers(device, capacity);
        let shared = Shared {
            layout: &layout,
            params: &params,
            counts: &counts,
            drawn: &drawn,
            shadow_drawn: &shadow_drawn,
            args: &args,
            meshes: &meshes,
        };
        let main = main_stream(device, &shared, capacity);
        let cull_shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Vegetation cull"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vegetation_cull.wgsl").into()),
        });
        let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Vegetation cull pipelines"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let compute = |entry: &str| {
            device.named_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Vegetation cull"),
                layout: Some(&compute_layout),
                module: &cull_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let cull = compute("cull");
        let finish = compute("finish");
        let (pipeline, grass_pipeline, depth_pipeline) =
            render_pipelines(device, shader, format, samples, material, &lighting.layout);
        let blades = vegetation::grass::gpu::Blades::new(
            device,
            queue,
            shader,
            format,
            samples,
            material,
            &lighting.layout,
            grass_map,
        );
        let shadow = shadow_pipelines(device, shader, lighting);
        Self {
            atlas,
            epoch: 0,
            main,
            panels: Vec::new(),
            view: (None, 0, 0, 0.),
            depth_pipeline,
            shadow,
            trees_ft: 0.,
            library,
            vertices,
            drawn,
            shadow_drawn,
            counts,
            args,
            params,
            meshes,
            mesh_dirty: true,
            patches,
            layout,
            capacity,
            cull,
            finish,
            pipeline,
            grass_pipeline,
            blades,
            grass_mode: Default::default(),
            grass_density: crate::graphics::VegDensity::High,
            grass_suppressed: false,
            repeat_frame: false,
            visible_pages: Vec::new(),
            page_cull: std::env::var("TORE_VEG_PAGE_CULL").as_deref() != Ok("0"),
            rings: Vec::new(),
            prepared: false,
            suppressed: false,
            settle: false,
            draw_nm: 0.,
            detail_nm: 0.,
            density: 1.,
            grass_radius: 0.,
            tufts: 0,
            cards: Default::default(),
        }
    }

    pub fn set_options(&mut self, device: &wgpu::Device, options: &crate::graphics::Options) {
        self.mesh_dirty = true;
        let grass_mode = options.grass.effective();
        self.grass_mode = grass_mode;
        self.grass_density = options
            .grass_density
            .unwrap_or(crate::graphics::VegDensity::High);
        self.cards = options.vegetation_cards.effective();
        self.draw_nm = options.vegetation_distance_nm as f32;
        self.detail_nm = (options.vegetation_detail_nm as f32).min(self.draw_nm);
        let density = options.vegetation_density;
        if self.density != density.scale() {
            self.reset_cache();
        }
        self.density = density.scale();
        let capacity = object_budget(density);
        if capacity != self.capacity {
            self.capacity = capacity;
            (self.drawn, self.shadow_drawn) = drawn_buffers(device, capacity);
            let fresh = main_stream(device, &self.shared(), capacity);
            let worker_kept = std::mem::replace(&mut self.main, fresh);
            // The worker, its results and the stream's eye carry over; the
            // pages restart.
            let mut cpu = worker_kept.cpu;
            cpu.reset_pages(capacity, stream::SPARE_PAGES);
            self.main.cpu = cpu;
        }
        // Panel streams are sized from these settings; the next far panel
        // makes a new one.
        self.panels.clear();
        self.tufts = options.grass_density.map_or(0, |d| d.grass().1);
        self.grass_radius = if options.grass_density.is_some() {
            options.grass_distance_ft as f32
        } else {
            0.
        };
    }

    /// Takes the resolved tree shadow distance
    /// (`TORE_TREE_SHADOW_FT` overrides it while shadows are on, a measuring
    /// aid).
    pub fn set_shadows(&mut self, shadows: &crate::surface_lighting::Settings) {
        let feet = std::env::var("TORE_TREE_SHADOW_FT")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(shadows.trees_ft);
        self.trees_ft = if shadows.enabled { feet.max(0.) } else { 0. };
    }

    /// The before picture: objects and grass are not drawn. Ground detail
    /// is the graphics option's own switch.
    pub fn suppressed(&self) -> bool {
        self.suppressed
    }

    pub fn set_repeat_frame(&mut self, repeat: bool) {
        self.repeat_frame = repeat;
    }

    pub fn set_grass_suppressed(&mut self, suppressed: bool) {
        self.grass_suppressed = suppressed;
    }

    pub fn set_suppressed(&mut self, suppressed: bool) {
        self.suppressed = suppressed;
    }

    /// Build every chunk the view needs before drawing (previews and
    /// captures), instead of a few per frame.
    pub fn set_settle(&mut self, settle: bool) {
        self.settle = settle;
    }

    /// Drops the patch cache so the next frames fill it at the current budget.
    pub fn reset_cache(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.main.cpu.reset(self.capacity, stream::SPARE_PAGES);
        self.panels.clear();
        self.view = (None, 0, 0, 0.);
        self.rings.clear();
        self.prepared = false;
    }

    /// Chunks built while preparing the last main frame, and the CPU time that took.
    pub fn cache_frame(&self) -> CacheFrame {
        self.main.cpu.cache
    }

    fn shared(&self) -> Shared<'_> {
        Shared {
            layout: &self.layout,
            params: &self.params,
            counts: &self.counts,
            drawn: &self.drawn,
            shadow_drawn: &self.shadow_drawn,
            args: &self.args,
            meshes: &self.meshes,
        }
    }

    // PASS6_POPIN_OBSERVER_BEGIN
    /// Observe the uploaded population without GPU readback or frustum-entry
    /// false positives. Called only by the explicit low-pass benchmark.
    pub fn near_population(&self, camera: &Camera, _size: [u32; 2]) -> popin::Population {
        let full = self.detail_nm * vegetation::NM as f32;
        let mut out = popin::Population::new();
        if let Some(ready) = &self.main.cpu.ready {
            for (_, bytes) in &ready.chunks {
                for bytes in bytes.chunks_exact(INST_STRIDE as usize) {
                    let words: [f32; 16] = std::array::from_fn(|i| {
                        f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
                    });
                    let distance = (words[0] - camera.position[0] as f32)
                        .hypot(words[2] - camera.position[2] as f32);
                    if distance > full || distance >= self.main.cpu.visible_reach {
                        continue;
                    }
                    let fade = 1.;
                    out.insert(
                        [
                            words[0].to_bits(),
                            words[1].to_bits(),
                            words[2].to_bits(),
                            words[12].to_bits(),
                        ],
                        fade,
                    );
                }
            }
        }
        out
    }
    pub fn detail_reach(&self) -> f32 {
        self.detail_nm * vegetation::NM as f32
    }
    // PASS6_POPIN_OBSERVER_END

    /// Objects placed around the main camera.
    pub fn placed(&self) -> u32 {
        self.main.cpu.count
    }

    /// Feet to the farthest chunk of objects within the instance budget.
    pub fn kept_reach(&self, _eye: [f64; 2]) -> f32 {
        self.main.cpu.ready.as_ref().map_or(0., |r| r.reach)
    }

    /// How far the last prepared view draws objects, feet: the main view's
    /// reach for the main view, the mirror and near panels.
    pub fn draw_reach(&self) -> f32 {
        self.view.3
    }

    pub fn squeeze(&self) -> f32 {
        self.main.cpu.ready.as_ref().map_or(1., |r| r.squeeze)
    }

    /// Grass tufts drawn last frame, before the shader drops any.
    pub fn blades_drawn(&self) -> u64 {
        if self.grass_mode == crate::graphics::Grass::Blades
            && !self.suppressed
            && !self.grass_suppressed
        {
            self.blades.count()
        } else {
            0
        }
    }

    pub fn tufts_drawn(&self) -> u64 {
        self.rings
            .iter()
            .map(|(r, n)| u64::from(r.end - r.start) * u64::from(*n))
            .sum()
    }

    pub fn rebuild(
        &mut self,
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        material: &wgpu::BindGroupLayout,
        lighting: &crate::surface_lighting::SurfaceLighting,
    ) {
        self.blades
            .rebuild(device, shader, format, samples, material, &lighting.layout);
        (self.pipeline, self.grass_pipeline, self.depth_pipeline) =
            render_pipelines(device, shader, format, samples, material, &lighting.layout);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        kind: ViewKind,
        camera: &Camera,
        world: &Terrain,
        origin: [f64; 3],
        size: [u32; 2],
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Vegetation);
        if self.repeat_frame && !self.suppressed {
            self.encode_cull(encoder);
            return;
        }
        let started = std::time::Instant::now();
        self.prepared = false;
        self.rings.clear();
        if self.suppressed || self.draw_nm <= 0. {
            self.main.cpu.cache = CacheFrame::default();
            self.view = (None, 0, 0, 0.);
            return;
        }
        let eye2 = [camera.position[0], camera.position[2]];
        let full = self.detail_nm * vegetation::NM as f32;
        let draw = self.draw_nm * vegetation::NM as f32;
        let grass_radius = if self.grass_mode == crate::graphics::Grass::Tufts {
            self.grass_radius
        } else {
            0.
        };
        let shape = stream::Shape {
            epoch: self.epoch,
            full,
            draw,
            density: self.density,
            capacity: self.capacity,
            grass_radius,
            settle: self.settle,
        };
        let route = route(kind, eye2, self.main.cpu.last_eye);
        let mut full_drawn = full;
        let mut source = None;
        match route {
            Route::Stream => {
                let request = self.main.cpu.request(shape, eye2);
                let (atlas, library) = (&self.atlas, &self.library);
                let GpuStream { cpu, instances, .. } = &mut self.main;
                cpu.advance(
                    request,
                    || stream::Worker::new(atlas.clone(), library.clone(), world),
                    false,
                    |offset, bytes| queue.write_buffer(instances, offset, bytes),
                );
                // Panels that stopped asking give their memory back.
                let now = std::time::Instant::now();
                self.panels
                    .retain(|p| now.duration_since(p.used) < PANEL_IDLE);
            }
            Route::Share => {}
            Route::Panel(page) => {
                let at = self.panel(device, page);
                let (atlas, library) = (&self.atlas, &self.library);
                let panel = &mut self.panels[at];
                panel.used = std::time::Instant::now();
                let request = panel.gpu.cpu.request(
                    stream::Shape {
                        full: panel.full,
                        draw: panel.draw,
                        capacity: panel.capacity,
                        ..shape
                    },
                    eye2,
                );
                let GpuStream { cpu, instances, .. } = &mut panel.gpu;
                cpu.advance(
                    request,
                    || stream::Worker::new(atlas.clone(), library.clone(), world),
                    true,
                    |offset, bytes| queue.write_buffer(instances, offset, bytes),
                );
                // Until its circle is in, the panel draws whatever the main
                // stream holds there.
                if cpu.visible_reach > 0. {
                    full_drawn = panel.full;
                    source = Some(page);
                }
            }
        }
        let stream = self.stream(source);
        self.view = (
            source,
            stream.cpu.count,
            stream.cpu.page_count,
            stream.cpu.visible_reach,
        );
        let draw = self.view.3;
        if self.mesh_dirty {
            self.upload_meshes(queue);
            queue.write_buffer(&self.args, 0, &self.cards.vertices().to_le_bytes());
            // The casters draw one card each.
            let casters = self.library.meshes.len() as u64 * 16;
            queue.write_buffer(&self.args, casters, &SHADOW_CARD_VERTICES.to_le_bytes());
            self.mesh_dirty = false;
        }
        let (fx, rx, ux) = basis(camera);
        let height = (size[1] as f32 * camera.view_fraction).max(1.);
        let aspect = size[0] as f32 / height;
        let f = 1.732_050_8 * camera.zoom.max(0.01);
        let eye = [
            (camera.position[0] - origin[0]) as f32,
            (camera.position[1] - origin[1]) as f32,
            (camera.position[2] - origin[2]) as f32,
        ];
        if self.grass_mode == crate::graphics::Grass::Blades {
            self.blades.prepare(
                device,
                queue,
                world,
                self.grass_density,
                self.grass_radius,
                camera.position,
            );
        } else if self.grass_mode == crate::graphics::Grass::Tufts {
            self.prepare_grass(
                queue,
                world,
                camera,
                origin,
                eye,
                [fx, rx, ux],
                [aspect / f, 1. / f],
            );
        }
        let main = route == Route::Stream;
        if self.view.1 == 0 {
            self.prepared =
                self.grass_mode == crate::graphics::Grass::Blades || !self.rings.is_empty();
            if main {
                self.main.cpu.cache.submit_ms = started.elapsed().as_secs_f64() * 1000.;
            }
            return;
        }
        // Only the pages this view may draw from go to the cull pass.
        let mut visible = std::mem::take(&mut self.visible_pages);
        let stream = self.stream(source);
        if self.page_cull {
            crate::vegetation_pages::PageView {
                eye: camera.position,
                forward: fx,
                right: rx,
                up: ux,
                tangents: [aspect / f, 1. / f],
                draw,
            }
            .visible(&stream.cpu.table, &stream.cpu.bounds, &mut visible);
        } else {
            visible.clone_from(&stream.cpu.table);
        }
        if !visible.is_empty() {
            queue.write_buffer(&stream.chunks, 0, &bytes_of_u32(&visible));
        }
        self.view.2 = (visible.len() / 2) as u32;
        self.visible_pages = visible;
        let mesh_count = self.library.meshes.len();
        let mut params = Vec::with_capacity(32);
        params.extend([origin[0] as f32, origin[1] as f32, origin[2] as f32, 0.]);
        params.extend([eye[0], eye[1], eye[2], 0.]);
        params.extend([fx[0], fx[1], fx[2], 0.]);
        params.extend([rx[0], rx[1], rx[2], 0.]);
        params.extend([ux[0], ux[1], ux[2], 0.]);
        params.extend([
            aspect / f,
            1. / f,
            height * f * 0.5,
            camera.position[1] as f32,
        ]);
        params.extend([full_drawn, draw, self.trees_ft, 1.5]);
        params.extend(u32s(&[self.view.1, mesh_count as u32, self.view.2, 0]));
        queue.write_buffer(&self.params, 0, &bytes_of_f32(&params));
        self.encode_cull(encoder);
        self.prepared = true;
        if main {
            self.main.cpu.cache.submit_ms = started.elapsed().as_secs_f64() * 1000.;
        }
    }

    /// The stream a view draws: the main one, or a panel's own.
    fn stream(&self, source: Option<u8>) -> &GpuStream {
        source
            .and_then(|page| self.panels.iter().find(|p| p.page == page))
            .map_or(&self.main, |p| &p.gpu)
    }

    /// The index of `page`'s own stream, made at the current settings if it
    /// has none.
    fn panel(&mut self, device: &wgpu::Device, page: u8) -> usize {
        if let Some(at) = self.panels.iter().position(|p| p.page == page) {
            return at;
        }
        let (full, draw, capacity, spare) = panel_shape(
            self.detail_nm * vegetation::NM as f32,
            self.draw_nm * vegetation::NM as f32,
            self.capacity,
        );
        let gpu = gpu_stream(
            device,
            &self.shared(),
            capacity,
            spare,
            stream::pages(capacity, spare) as u64 * 8,
            "Panel vegetation",
        );
        self.panels.push(PanelStream {
            page,
            full,
            draw,
            capacity,
            used: std::time::Instant::now(),
            gpu,
        });
        self.panels.len() - 1
    }

    // The diagnostic grass-off repeat reuses the exact uploaded tree scene,
    // but encodes the same culling workload so its GPU delta isolates grass.
    fn encode_cull(&self, encoder: &mut wgpu::CommandEncoder) {
        let (source, count, page_count, _) = self.view;
        if count == 0 {
            return;
        }
        encoder.clear_buffer(&self.counts, 0, None);
        let mesh_count = self.library.meshes.len();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Vegetation cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.cull);
        pass.set_bind_group(0, &self.stream(source).bind, &[]);
        pass.dispatch_workgroups(page_count, 1, 1);
        pass.set_pipeline(&self.finish);
        pass.dispatch_workgroups((mesh_count as u32).div_ceil(64), 1, 1);
        drop(pass);
    }

    fn upload_meshes(&self, queue: &wgpu::Queue) {
        let lib = &self.library;
        let mesh_count = lib.meshes.len();
        let mut table = Vec::with_capacity(mesh_count * 12);
        for mesh in &lib.meshes {
            let look = match mesh.look {
                Look::Foliage => 0.,
                Look::Solid => 1.,
            };
            let mean = mesh.recolour.unwrap_or([1.; 3]);
            table.extend([
                f32::from_bits(0),
                look,
                mesh.radius,
                f32::from_bits(self.capacity as u32),
            ]);
            // Negative page codes carry an original material atlas cell.
            let page = mesh.sprite_tile.map_or(mesh.sprite_layer as f32, |tile| {
                -((mesh.sprite_layer * 4 + usize::from(tile) + 1) as f32)
            });
            table.extend([mean[0], mean[1], mean[2], page]);
            table.extend(mesh.top);
        }
        if !table.is_empty() {
            queue.write_buffer(&self.meshes, 0, &bytes_of_f32(&table));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_grass(
        &mut self,
        queue: &wgpu::Queue,
        world: &Terrain,
        camera: &Camera,
        origin: [f64; 3],
        eye: [f32; 3],
        basis: [[f32; 3]; 3],
        tan: [f32; 2],
    ) {
        let agl = camera.position[1] as f32
            - world.height(camera.position[0] as f32, camera.position[2] as f32);
        if self.tufts == 0 || self.grass_radius <= 0. || agl > 2_500. {
            return;
        }
        let radius = self.grass_radius;
        // Field borrows, so `self.rings` stays free to fill below.
        let stream = self
            .view
            .0
            .and_then(|page| self.panels.iter().find(|p| p.page == page))
            .map_or(&self.main, |p| &p.gpu);
        let Some(ready) = &stream.cpu.ready else {
            return;
        };
        let patches = &ready.grass;
        let [fx, rx, ux] = basis;
        let mut rings: Vec<Vec<&Patch>> = vec![Vec::new(); RINGS.len()];
        for p in patches {
            let rel = [
                p.pos[0] - origin[0] as f32 - eye[0],
                p.pos[1] - origin[1] as f32 - eye[1],
                p.pos[2] - origin[2] as f32 - eye[2],
            ];
            let dist = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
            let r = 34.;
            let z = dot(rel, fx);
            if z < -r || dot(rel, rx).abs() > z * tan[0] + r || dot(rel, ux).abs() > z * tan[1] + r
            {
                continue;
            }
            let share = dist / radius;
            if share >= 1. {
                continue;
            }
            let ring = RINGS
                .iter()
                .position(|(edge, _)| share < *edge)
                .unwrap_or(RINGS.len() - 1);
            rings[ring].push(p);
        }
        let mut bytes = Vec::new();
        let mut start = 0u32;
        for (ring, list) in rings.iter().enumerate() {
            let tufts = ((self.tufts as f32 * RINGS[ring].1).round() as u32).max(4);
            for p in list {
                let record = [
                    p.pos[0] - origin[0] as f32,
                    p.pos[1] - origin[1] as f32,
                    p.pos[2] - origin[2] as f32,
                    p.density,
                    p.grad[0],
                    p.grad[1],
                    p.kind,
                    p.height,
                    p.color[0],
                    p.color[1],
                    p.color[2],
                    p.hash,
                    tufts as f32,
                    self.tufts as f32,
                    radius,
                    0.,
                ];
                bytes.extend(bytes_of_f32(&record));
            }
            let end = start + list.len() as u32;
            if end > start {
                self.rings.push((start..end, tufts));
            }
            start = end;
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.patches, 0, &bytes);
        }
    }

    /// The blade grass (a test aid).
    #[cfg(test)]
    pub fn blades_mut(&mut self) -> &mut vegetation::grass::gpu::Blades {
        &mut self.blades
    }
    #[cfg(test)]
    pub fn blades(&self) -> &vegetation::grass::gpu::Blades {
        &self.blades
    }

    /// Records the blade grass candidate pass for the prepared view, when
    /// its draw will draw blades. `bind` is the world bind group `draw`
    /// gets.
    pub fn encode_blades(&self, encoder: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup) {
        if self.prepared
            && !self.grass_suppressed
            && self.grass_mode == crate::graphics::Grass::Blades
        {
            self.blades.encode(encoder, bind);
        }
    }

    /// Casts the prepared view's near trees into the shadow maps, when the
    /// shadow quality has trees cast.
    pub fn draw_shadows(
        &self,
        lighting: &crate::surface_lighting::SurfaceLighting,
        encoder: &mut wgpu::CommandEncoder,
        bind: &wgpu::BindGroup,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Vegetation);
        if !self.prepared || self.suppressed || self.trees_ft <= 0. || self.view.1 == 0 {
            return;
        }
        let offset = self.library.meshes.len() as u64 * 16;
        lighting.draw_more(encoder, |pass, cascade| {
            pass.set_pipeline(&self.shadow[cascade]);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_vertex_buffer(1, self.shadow_drawn.slice(..));
            crate::frame_log::count(crate::frame_log::Count::VegetationDraws, 1);
            pass.draw_indirect(&self.args, offset);
        });
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, bind: &'a wgpu::BindGroup) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Vegetation);
        if !self.prepared {
            return;
        }
        pass.set_bind_group(0, bind, &[]);
        if self.view.1 > 0 {
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_vertex_buffer(1, self.drawn.slice(..));
            crate::frame_log::count(crate::frame_log::Count::VegetationDraws, 1);
            pass.draw_indirect(&self.args, 0);
            pass.set_pipeline(&self.pipeline);
            crate::frame_log::count(crate::frame_log::Count::VegetationDraws, 1);
            pass.draw_indirect(&self.args, 0);
        }
        if self.grass_suppressed {
            return;
        }
        if self.grass_mode == crate::graphics::Grass::Blades {
            self.blades.draw(pass);
        }
        if !self.rings.is_empty() {
            pass.set_pipeline(&self.grass_pipeline);
            pass.set_vertex_buffer(0, self.patches.slice(..));
            for (range, tufts) in &self.rings {
                crate::frame_log::count(crate::frame_log::Count::VegetationDraws, 1);
                pass.draw(0..tufts * 6, range.clone());
            }
        }
    }
}

/// Readiness includes the fade circle reaching the requested radius. Otherwise
/// a fast worker could end benchmark warmup while the visible forest still grows.
fn advance_visible_range(
    previous: f32,
    available: f32,
    requested: f32,
    dt: f32,
    settle: bool,
) -> (f32, bool) {
    let target = available.min(requested);
    let reach = if settle {
        target
    } else {
        (previous + dt.min(0.1) * 6000.).min(target)
    };
    (reach, reach + 1. < requested)
}

/// Objects kept around the camera at most, per density.
fn object_budget(density: crate::graphics::VegDensity) -> usize {
    density.object_budget()
}

/// The cull pass's bindings that every stream shares: each view culls and
/// draws in its own submission, so one set of output lists serves them all.
struct Shared<'a> {
    layout: &'a wgpu::BindGroupLayout,
    params: &'a wgpu::Buffer,
    counts: &'a wgpu::Buffer,
    drawn: &'a wgpu::Buffer,
    shadow_drawn: &'a wgpu::Buffer,
    args: &'a wgpu::Buffer,
    meshes: &'a wgpu::Buffer,
}

/// The two lists the cull pass writes, for `capacity` objects.
fn drawn_buffers(device: &wgpu::Device, capacity: usize) -> (wgpu::Buffer, wgpu::Buffer) {
    let listed = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX;
    crate::vegetation_limits::buffer(
        "Vegetation drawn",
        capacity as u64 * DRAWN_STRIDE,
        listed,
        &device.limits(),
    )
    .expect("vegetation buffer limits");
    (
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation drawn"),
            size: capacity as u64 * DRAWN_STRIDE,
            usage: listed,
            mapped_at_creation: false,
        }),
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation shadow casters"),
            size: capacity as u64 * DRAWN_STRIDE,
            usage: listed,
            mapped_at_creation: false,
        }),
    )
}

/// The main view's stream at the density's budget.
fn main_stream(device: &wgpu::Device, shared: &Shared<'_>, capacity: usize) -> GpuStream {
    gpu_stream(
        device,
        shared,
        capacity,
        stream::SPARE_PAGES,
        crate::vegetation_limits::CHUNK_BYTES,
        "Vegetation",
    )
}

/// A stream's instance pages, page table and cull bindings.
fn gpu_stream(
    device: &wgpu::Device,
    shared: &Shared<'_>,
    capacity: usize,
    spare: usize,
    table_bytes: u64,
    label: &str,
) -> GpuStream {
    let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    let size = stream::pages(capacity, spare) as u64 * stream::PAGE as u64 * INST_STRIDE;
    crate::vegetation_limits::buffer("Vegetation instances", size, storage, &device.limits())
        .expect("vegetation buffer limits");
    let instances = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("{label} instances")),
        size,
        usage: storage,
        mapped_at_creation: false,
    });
    let chunks = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("{label} chunk pages")),
        size: table_bytes,
        usage: storage,
        mapped_at_creation: false,
    });
    let buffers = [
        &instances,
        shared.params,
        shared.counts,
        shared.drawn,
        shared.shadow_drawn,
        shared.args,
        shared.meshes,
        &chunks,
    ];
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Vegetation cull"),
        layout: shared.layout,
        entries: &entries,
    });
    GpuStream {
        cpu: stream::Stream::new(capacity, spare),
        instances,
        chunks,
        bind,
    }
}

fn basis(camera: &Camera) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sy, cy) = camera.yaw.sin_cos();
    let (sp, cp) = camera.pitch.sin_cos();
    let (sr, cr) = camera.roll.sin_cos();
    let right = [cy * cr - sy * sp * sr, cp * sr, -sy * cr - cy * sp * sr];
    let up = [-cy * sr - sy * sp * cr, cp * cr, sy * sr - cy * sp * cr];
    let forward = [sy * cp, sp, cy * cp];
    (forward, right, up)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// One instance record: position and yaw; height, width, ground slope;
/// tint and rank; mesh, distance reach, radius in feet, hash.
fn pack(i: &Inst, mesh: u16, shape: &crate::vegetation_shapes::Mesh) -> [f32; 16] {
    let reach = Species::reach(i.species);
    [
        i.pos[0],
        i.pos[1],
        i.pos[2],
        i.yaw,
        i.height * shape.height_scale,
        i.width,
        i.grad[0],
        i.grad[1],
        i.tint[0],
        i.tint[1],
        i.tint[2],
        i.rank,
        f32::from(mesh),
        reach,
        shape.radius * shape.height_scale * i.height * i.width,
        f32::from(i.variant) / 255.,
    ]
}

fn render_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    material: &wgpu::BindGroupLayout,
    lighting: &wgpu::BindGroupLayout,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let constants = [("FILL_SAMPLES", f64::from(samples))];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Vegetation layout"),
        bind_group_layouts: &[material, lighting],
        push_constant_ranges: &[],
    });
    let object_buffers = [
        wgpu::VertexBufferLayout {
            array_stride: (FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32x3, 4 => Float32],
        },
        wgpu::VertexBufferLayout {
            array_stride: DRAWN_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                5 => Float32x4,
                6 => Float32x4,
                7 => Float32x4,
                8 => Float32x4,
                9 => Float32x4,
                10 => Float32x4
            ],
        },
    ];
    let grass_buffers = [wgpu::VertexBufferLayout {
        array_stride: PATCH_STRIDE,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4
        ],
    }];
    let make = |label,
                vertex: &str,
                fragment: &str,
                buffers: &[wgpu::VertexBufferLayout],
                depth: bool,
                writes: bool| {
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vertex),
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
                    write_mask: if depth {
                        wgpu::ColorWrites::empty()
                    } else {
                        wgpu::ColorWrites::ALL
                    },
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: writes,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        })
    };
    (
        make(
            "Vegetation",
            "veg_vertex",
            "veg_fragment",
            &object_buffers,
            false,
            false,
        ),
        make(
            "Grass",
            "grass_vertex",
            "grass_fragment",
            &grass_buffers,
            false,
            true,
        ),
        make(
            "Vegetation depth",
            "veg_vertex",
            "veg_depth",
            &object_buffers,
            true,
            true,
        ),
    )
}

/// The tree shadow casters: a sun-facing card per tree, cut out by its
/// sprite (`veg_shadow_vertex0` to `2` in terrain.wgsl), with the object
/// casters' bias (surface_lighting.rs).
fn shadow_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    lighting: &crate::surface_lighting::SurfaceLighting,
) -> [wgpu::RenderPipeline; 3] {
    let buffers = [
        wgpu::VertexBufferLayout {
            array_stride: (FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
        },
        wgpu::VertexBufferLayout {
            array_stride: DRAWN_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                5 => Float32x4,
                6 => Float32x4,
                7 => Float32x4,
                8 => Float32x4,
                9 => Float32x4
            ],
        },
    ];
    std::array::from_fn(|cascade| {
        let entry = format!("veg_shadow_vertex{cascade}");
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Vegetation shadow casters"),
            layout: Some(&lighting.caster_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(&entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("terrain_shadow_fragment"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
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

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn u32s(v: &[u32]) -> [f32; 4] {
    std::array::from_fn(|i| f32::from_bits(v[i]))
}

pub(crate) fn bytes_of_f32(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for n in v {
        out.extend(n.to_le_bytes());
    }
    out
}

fn bytes_of_u32(v: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for n in v {
        out.extend(n.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn warmup_waits_for_both_uploaded_range_and_visible_range() {
        let (mut reach, pending) = super::advance_visible_range(0., 24000., 24000., 0.016, false);
        assert!(pending);
        assert!(reach < 100.);
        for _ in 0..400 {
            reach = super::advance_visible_range(reach, 24000., 24000., 0.016, false).0;
        }
        assert_eq!(
            super::advance_visible_range(reach, 24000., 24000., 0.016, false),
            (24000., false)
        );
        assert_eq!(
            super::advance_visible_range(24000., 8000., 24000., 0.016, false),
            (8000., true)
        );
        assert_eq!(
            super::advance_visible_range(0., 30000., 24000., 0., true),
            (24000., false)
        );
    }

    #[test]
    fn card_stream_layout_matches_the_compute_output() {
        let module =
            wgpu::naga::front::wgsl::parse_str(include_str!("vegetation_cull.wgsl")).unwrap();
        for (name, bytes) in [
            ("Inst", super::INST_STRIDE as u32),
            ("Drawn", super::DRAWN_STRIDE as u32),
            ("Mesh", 48),
        ] {
            let ty = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap()
                .1;
            let wgpu::naga::TypeInner::Struct { span, .. } = ty.inner else {
                panic!("expected struct")
            };
            assert_eq!(span, bytes, "{name}");
        }
    }

    /// John saw the ground detail vanish with the afterburner lit: a
    /// dynamic light (flare, afterburner, fire, gun flash) swapped in an
    /// undetailed albedo. Both the shaded colour and the flare-lit albedo
    /// must carry the same detail tint and normal.
    #[test]
    fn a_dynamic_light_never_drops_the_ground_detail() {
        // A Windows checkout may give the shader CRLF line ends.
        let shader = include_str!("terrain.wgsl").replace("\r\n", "\n");
        let start = shader
            .find("@fragment fn terrain_fragment")
            .expect("terrain fragment");
        let end = start + shader[start..].find("\n}\n").expect("end of function");
        let body = &shader[start..end];
        let calls: Vec<&str> = body
            .match_indices("surface_color(")
            .map(|(i, _)| {
                let rest = &body[i..];
                &rest[..rest.find(';').expect("statement end")]
            })
            .collect();
        assert_eq!(calls.len(), 2, "textured and untextured ground");
        for call in calls {
            assert_eq!(call.matches("detail.tint").count(), 2, "{call}");
            assert!(call.contains("detail.normal"), "{call}");
        }
    }
}

#[cfg(test)]
#[path = "vegetation_sample_tests.rs"]
mod sample_tests;
