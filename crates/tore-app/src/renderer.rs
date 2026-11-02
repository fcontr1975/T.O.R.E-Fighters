use crate::gpu_setup::NamedSteps as _;
mod leave_dialog;
use crate::{
    AppResult,
    menu::{HEIGHT, WIDTH},
};
use std::sync::Arc;
use winit::window::Window;

/// A window surface that is left to process exit when dropped during a panic.
///
/// wgpu skips discarding an unpresented frame while the thread panics, so the
/// Vulkan backend would then panic tearing down the swapchain, and that second
/// panic during unwinding aborts the process before the fatal report dialog.
/// A panic always ends the game, so nothing is lost by not tearing it down.
pub(crate) struct WindowSurface(Arc<wgpu::Surface<'static>>);
impl WindowSurface {
    pub(crate) fn new(surface: wgpu::Surface<'static>) -> Self {
        Self(Arc::new(surface))
    }
}
impl std::ops::Deref for WindowSurface {
    type Target = wgpu::Surface<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for WindowSurface {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // A reference that is never released keeps the surface alive.
            std::mem::forget(Arc::clone(&self.0));
        }
    }
}

#[derive(Clone, Copy)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        let scale = (width as f32 / WIDTH as f32).min(height as f32 / HEIGHT as f32);
        let (w, h) = (WIDTH as f32 * scale, HEIGHT as f32 * scale);
        Self {
            x: (width as f32 - w) / 2.0,
            y: (height as f32 - h) / 2.0,
            width: w,
            height: h,
        }
    }
    pub fn point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.width <= 0.0
            || self.height <= 0.0
            || x < self.x as f64
            || y < self.y as f64
            || x >= (self.x + self.width) as f64
            || y >= (self.y + self.height) as f64
        {
            return None;
        }
        Some((
            (x - self.x as f64) * WIDTH as f64 / self.width as f64,
            (y - self.y as f64) * HEIGHT as f64 / self.height as f64,
        ))
    }
}
struct Readback {
    buffer: wgpu::Buffer,
    receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    width: u32,
    height: u32,
    stride: u32,
    bgra: bool,
}
impl Readback {
    /// The pixels, and the unmapped buffer for reuse.
    fn pixels(self) -> (Vec<u8>, wgpu::Buffer) {
        let data = self.buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in data.chunks_exact(self.stride as usize) {
            for pixel in row[..self.width as usize * 4].chunks_exact(4) {
                pixels.extend_from_slice(&if self.bgra {
                    [pixel[2], pixel[1], pixel[0], pixel[3]]
                } else {
                    [pixel[0], pixel[1], pixel[2], pixel[3]]
                });
            }
        }
        drop(data);
        self.buffer.unmap();
        (pixels, self.buffer)
    }
}
/// Readback textures and buffers kept for reuse, by size: the camera panels
/// ask for the same small picture several times a second.
#[derive(Default)]
struct ReadbackPool {
    textures: Vec<([u32; 2], wgpu::TextureFormat, wgpu::Texture)>,
    buffers: Vec<(u64, wgpu::Buffer)>,
}
impl ReadbackPool {
    const TEXTURES: usize = 4;
    const BUFFERS: usize = 6;
    /// A render target of this size. Reusing one is safe: each use is a
    /// later submission that clears and redraws all of it.
    fn texture(
        &mut self,
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        if let Some((_, _, texture)) = self
            .textures
            .iter()
            .find(|(s, f, _)| *s == size && *f == format)
        {
            return texture.clone();
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Terrain validation capture"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        if self.textures.len() >= Self::TEXTURES {
            self.textures.remove(0);
        }
        self.textures.push((size, format, texture.clone()));
        texture
    }
    /// An unmapped readback buffer of `size` bytes, taken out of the pool.
    fn buffer(&mut self, device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        if let Some(i) = self.buffers.iter().position(|(s, _)| *s == size) {
            return self.buffers.swap_remove(i).1;
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain capture readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }
    /// The pixels of a finished readback; its buffer goes back to the pool.
    fn finish(&mut self, readback: Readback) -> Vec<u8> {
        let (pixels, buffer) = readback.pixels();
        if self.buffers.len() >= Self::BUFFERS {
            self.buffers.remove(0);
        }
        self.buffers.push((buffer.size(), buffer));
        pixels
    }
}
pub struct Renderer {
    previews: std::collections::BTreeMap<u8, Readback>,
    readbacks: ReadbackPool,
    first_frame: crate::startup::FirstFrame,
    /// Frames in a row whose surface gave no image; the caller waits
    /// [`crate::frame_pacing::surface_retry`] before trying again.
    surface_failures: u32,
    surface: WindowSurface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    /// What the canvas texture holds of the flight canvas, when the last
    /// upload was the flight canvas's: the next one sends only the rows that
    /// differ from it.
    canvas_held: Option<Vec<u8>>,
    bind_group: wgpu::BindGroup,
    /// Redout and blackout levels the canvas shader veils in flight.
    veil: wgpu::Buffer,
    veil_levels: [f32; 2],
    pipeline: wgpu::RenderPipeline,
    /// Sharp text drawn over the menu canvas (`ui_text`), and whether the
    /// screen up wants it drawn.
    ui_text: crate::ui_text_renderer::UiTextRenderer,
    menu_text_visible: bool,
    leave_dialog: Option<leave_dialog::Overlay>,
    /// Frame times for the optional corner readout. Drawn only in this
    /// window pass, never into a capture.
    frame_rate: crate::frame_rate::Meter,
    frame_gpu: crate::frame_gpu::Timer,
    frame_cpu: Option<std::time::Instant>,
    sim: crate::sim_renderer::SimRenderer,
    cockpit: crate::cockpit_renderer::CockpitRenderer,
    mirror_camera: crate::camera::Camera,
    mirror_vertices: Vec<f32>,
    /// Rear mirror renders, and which flight frames draw one.
    pub mirror_frames: u64,
    mirror_cadence: crate::mirrors::Cadence,
    mirrors_enabled: bool,
    /// When the counter's line was last logged (`TORE_SHOW_FPS=1` runs).
    counter_logged: Option<std::time::Instant>,
    graphics: crate::graphics::Options,
    /// The present modes the surface offers, and the frame rate in effect
    /// (`crate::frame_pacing`), which picks one of them.
    present_modes: Vec<wgpu::PresentMode>,
    frame_limit: crate::frame_pacing::Limit,
    /// A scripted or measured run, a capture or a smoke test: uncapped
    /// unless `TORE_FRAME_LIMIT` says otherwise (`Limit::for_run`).
    automated: bool,
    /// MSAA sample counts the adapter supports for the surface and depth.
    sample_counts: Vec<u32>,
    /// The AC-130 sensor console (`crate::sensor_console`): its CRT pass,
    /// the world picture it reads, the symbology overlay, and this frame's
    /// request (set by [`Renderer::set_console`], taken by the next draw).
    crt: crate::crt::Crt,
    console_scene: Option<(wgpu::Texture, [u32; 2])>,
    console_overlay: Option<(wgpu::Texture, [u32; 2])>,
    console: Option<(crate::crt::Frame, Vec<u8>, [u32; 2])>,
    /// The look of the last console picture drawn, and whether the next draw
    /// shows that picture again ([`Renderer::repeat_console`]).
    console_last: Option<crate::crt::Frame>,
    console_repeat: bool,
    // Fields drop in declaration order; keep the window alive through GPU cleanup.
    pub window: Arc<Window>,
}
pub(crate) struct LoadingScene<'a> {
    pub scenery: &'a crate::scenery::Scenery,
    pub world: &'a crate::terrain::Terrain,
    pub eye: [f64; 3],
}
pub(crate) struct LoadingGraphics {
    sim: crate::sim_renderer::SimRenderer,
    cockpit: crate::cockpit_renderer::CockpitRenderer,
}
impl Renderer {
    /// wgpu resources are thread safe; the window and surface stay on the event thread.
    pub fn loading_factory(
        &self,
    ) -> impl FnOnce(
        LoadingScene<'_>,
        &crate::aircraft::Airframe,
        &[crate::aircraft::Airframe],
        &crate::render_snapshot::CombatArt,
        Option<&Arc<crate::scenery::StaticGeometry>>,
    ) -> LoadingGraphics
    + Send
    + 'static {
        let device = self.device.clone();
        let queue = self.queue.clone();
        let format = self.config.format;
        let options = self.graphics;
        let samples = self.samples(options.anti_aliasing);
        move |scene, aircraft, models, art, static_geometry| {
            let LoadingScene {
                scenery,
                world,
                eye,
            } = scene;
            let mut sim = crate::sim_renderer::SimRenderer::new(
                &device, &queue, format, scenery, options, samples,
            );
            if let Some(vegetation) = &mut sim.vegetation {
                let _timing = crate::loading::timing("vegetation initial ring and upload");
                let mut camera = crate::camera::Camera::for_world(world);
                camera.position = eye;
                let mut encoder = device.create_command_encoder(&Default::default());
                vegetation.set_settle(true);
                vegetation.prepare(
                    &device,
                    &queue,
                    &mut encoder,
                    crate::sim_renderer::ViewKind::Main,
                    &camera,
                    world,
                    [0.; 3],
                    [640, 480],
                );
                vegetation.set_settle(false);
                queue.submit([encoder.finish()]);
            }
            sim.aircraft(&device, &queue, aircraft, &[]);
            sim.dummies(
                &device,
                &queue,
                models
                    .iter()
                    .map(|model| (model, Vec::new(), Vec::new()))
                    .collect(),
            );
            sim.combat(&device, &queue, &Default::default());
            sim.effects(&device, &queue, &art.effects, &[], &[]);
            let smoke = tore_sim::combat::smoke::Smoke::default();
            sim.smoke(
                &device,
                &queue,
                &art.smoke,
                [&smoke, &smoke],
                &Default::default(),
            );
            if let Some(escape) = &art.escape {
                sim.escapees(&device, &queue, escape, &[]);
            }
            if let Some(geometry) = static_geometry {
                sim.airports(&device, &queue, geometry);
            }

            let mut cockpit = crate::cockpit_renderer::CockpitRenderer::new(&device, format);
            cockpit.prepare(
                &device,
                &queue,
                &aircraft.sprites[aircraft.profile.id.cockpit()],
                &aircraft.cockpit_pic,
                aircraft.profile.id,
            );
            LoadingGraphics { sim, cockpit }
        }
    }
    pub fn install_loading(
        &mut self,
        graphics: LoadingGraphics,
    ) -> Option<std::thread::JoinHandle<()>> {
        self.previews.clear();
        self.mirror_vertices.clear();
        // The mirror picture belongs to the previous aircraft or scene.
        self.mirror_cadence = Default::default();
        let old = LoadingGraphics {
            sim: std::mem::replace(&mut self.sim, graphics.sim),
            cockpit: std::mem::replace(&mut self.cockpit, graphics.cockpit),
        };
        // Driver resource destruction can be expensive too. The caller joins
        // this after the window closes.
        match std::thread::Builder::new()
            .name("graphics-retire".into())
            .spawn(move || drop(old))
        {
            Ok(handle) => Some(handle),
            Err(error) => {
                log::warn!("Graphics retirement worker: {error}");
                None
            }
        }
    }
    pub fn prepare_aircraft(&mut self, hornet: &crate::aircraft::Airframe) {
        self.previews.clear();
        self.mirror_vertices.clear();
        // The mirror picture belongs to the previous aircraft or scene.
        self.mirror_cadence = Default::default();
        self.sim.clear_aircraft();
        self.sim.aircraft(&self.device, &self.queue, hornet, &[]);
        self.cockpit.prepare(
            &self.device,
            &self.queue,
            &hornet.sprites[hornet.profile.id.cockpit()],
            &hornet.cockpit_pic,
            hornet.profile.id,
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub fn cockpit_lighting(&self, gain: f32, flood: f32, ambient: f32) {
        self.cockpit.lighting(&self.queue, gain, flood, ambient);
    }
    pub fn cockpit(
        &mut self,
        state: &crate::flight::State,
        camera: &crate::camera::Camera,
        art: bool,
        hud: bool,
        pixels: &[u8],
        colors: &[[u8; 3]; 256],
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        self.cockpit.weather(&self.queue, colors);
        self.cockpit.update(
            &self.queue,
            self.flight_size(),
            state,
            camera,
            art,
            hud,
            pixels,
        );
    }
    /// The flown aircraft, with `stores`, the vertices of the pylons and
    /// stores it carries (B3, [`crate::pylons`]), which show and hide with it.
    #[allow(clippy::too_many_arguments)]
    pub fn aircraft(
        &mut self,
        hornet: &crate::aircraft::Airframe,
        state: &crate::flight::State,
        stores: &[f32],
        visible: bool,
        camera: &crate::camera::Camera,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        self.mirror_camera = crate::mirrors::camera(state);
        if !visible {
            self.mirror_vertices = hornet.vertices(state, &self.mirror_camera, world, scenery);
            self.mirror_vertices.extend_from_slice(stores);
        }
        let mut vertices = hornet.vertices(state, camera, world, scenery);
        vertices.extend_from_slice(stores);
        self.sim
            .aircraft(&self.device, &self.queue, hornet, &vertices);
        if !visible {
            self.sim.hide_aircraft();
        }
    }
    pub fn dummies(
        &mut self,
        geometry: Vec<(
            &crate::aircraft::Airframe,
            Vec<f32>,
            Vec<crate::sim_renderer::Contact>,
        )>,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        self.sim.dummies(&self.device, &self.queue, geometry);
    }
    pub fn combat(&mut self, geometry: &crate::sim_renderer::CombatGeometry) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        self.sim.combat(&self.device, &self.queue, geometry);
    }
    pub fn escapees(&mut self, art: &crate::ejection_art::Art, vertices: &[f32]) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
        self.sim.escapees(&self.device, &self.queue, art, vertices);
    }
    pub fn airports(&mut self, geometry: &Arc<crate::scenery::StaticGeometry>) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Cities);
        self.sim.airports(&self.device, &self.queue, geometry);
    }
    /// This frame's moving surface units, sprites and parked aircraft pieces.
    pub fn surface_units(&mut self, vertices: &[f32]) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Surface);
        self.sim.surface_units(&self.device, &self.queue, vertices);
    }
    /// Smoke puffs, flare smoke and the released chaff and flares.
    pub fn smoke(
        &mut self,
        art: &tore_formats::Pic,
        smoke: [&tore_sim::combat::smoke::Smoke; 2],
        devices: &tore_sim::combat::countermeasures::Devices,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        self.sim
            .smoke(&self.device, &self.queue, art, smoke, devices);
    }
    /// A replay's clock for the burning city fill buildings (nr-fx4).
    pub fn set_city_clock(&mut self, clock: Option<crate::city_fires::Clock>) {
        self.sim.set_city_clock(clock);
    }
    /// Explosions, craters and crash-site fires.
    pub fn effects(
        &mut self,
        art: &crate::effect_renderer::Art,
        effects: &[crate::snapshot::EffectPose],
        marks: &[crate::snapshot::MarkPose],
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        self.sim
            .effects(&self.device, &self.queue, art, effects, marks);
    }
    /// Burning flares, chaff, lit afterburners and AC-130 gunfire, which
    /// light the scene.
    pub fn emitters(
        &mut self,
        devices: &tore_sim::combat::countermeasures::Devices,
        afterburners: &[crate::countermeasure_renderer::Afterburner],
        guns: &crate::gun_flash::Drawn,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        self.sim.emitters(&self.queue, devices, afterburners, guns);
    }
    pub fn vapor(&mut self, vertices: &[f32]) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        self.sim.vapor(&self.device, &self.queue, vertices);
    }
    pub fn set_scenery(&mut self, scenery: &crate::scenery::Scenery) {
        // A theater load builds more pipelines (city fill, vegetation) than the
        // menu did, on the same driver, so it is GPU setup too.
        crate::graphics_api::setup_starting(crate::graphics_api::Screen::Game);
        self.sim = crate::sim_renderer::SimRenderer::new(
            &self.device,
            &self.queue,
            self.config.format,
            scenery,
            self.graphics,
            self.samples(self.graphics.anti_aliasing),
        );
        crate::graphics_api::setup_ended();
        self.mirror_cadence = Default::default();
    }
    pub fn begin_cpu_frame(&mut self) {
        self.frame_cpu = Some(std::time::Instant::now());
    }

    pub fn graphics(&self) -> crate::graphics::Options {
        self.graphics
    }
    /// Apply graphics choices at once; an anti-aliasing change rebuilds the
    /// world pipelines.
    pub fn set_graphics(&mut self, options: crate::graphics::Options) {
        log::info!("Graphics changed: {options:?}");
        self.graphics = options;
        let samples = self.samples(options.anti_aliasing);
        self.sim.set_graphics(&self.device, options, samples);
        self.set_frame_limit(crate::frame_pacing::Limit::for_run(
            options.frame_limit,
            self.automated,
        ));
    }
    /// Marks the run as automated (see `automated`), applying its frame rate.
    pub fn automate(&mut self, automated: bool) {
        self.automated = automated;
        self.set_frame_limit(crate::frame_pacing::Limit::for_run(
            self.graphics.frame_limit,
            automated,
        ));
    }
    /// The frame rate in effect: the saved choice or `TORE_FRAME_LIMIT`.
    pub fn frame_limit(&self) -> crate::frame_pacing::Limit {
        self.frame_limit
    }
    /// Whether the frame-rate counter is on for this run: it keeps menus and
    /// paused views drawing while it measures.
    pub fn counter_shown(&self) -> bool {
        crate::frame_rate::shown(self.graphics.show_frame_rate)
    }
    /// Applies a frame rate at once, configuring the surface again when the
    /// present mode changes.
    fn set_frame_limit(&mut self, limit: crate::frame_pacing::Limit) {
        self.frame_limit = limit;
        let mode = limit.present_mode(&self.present_modes);
        if mode != self.config.present_mode {
            self.config.present_mode = mode;
            log::info!("Presentation: {mode:?} for frame rate {limit:?}");
            let s = self.window.inner_size();
            if s.width > 0 && s.height > 0 {
                self.surface.configure(&self.device, &self.config);
            }
            self.window.request_redraw();
        }
    }
    /// Whether this adapter can draw the given anti-aliasing level exactly.
    pub fn supports(&self, level: crate::graphics::AntiAliasing) -> bool {
        self.sample_counts.contains(&level.samples())
    }
    /// The highest supported sample count not above the requested one.
    fn samples(&self, level: crate::graphics::AntiAliasing) -> u32 {
        self.sample_counts
            .iter()
            .copied()
            .filter(|&n| n <= level.samples())
            .max()
            .unwrap_or(1)
    }

    pub async fn new(
        window: Arc<Window>,
        scenery: &crate::scenery::Scenery,
        graphics: crate::graphics::Options,
    ) -> AppResult<Self> {
        crate::graphics_api::setup_starting(crate::graphics_api::Screen::Game);
        crate::diagnostics::stage("game graphics instance and surface");
        let instance = crate::graphics_api::instance();
        let surface = {
            let _step = crate::gpu_setup::begin("surface", "game window");
            instance.create_surface(window.clone())?
        };
        crate::diagnostics::stage_done();
        crate::diagnostics::stage("game graphics adapter selection");
        let adapter = {
            let _step = crate::gpu_setup::begin("adapter request", "game window");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await?
        };
        crate::diagnostics::stage_done();
        crate::gpu_setup::note_adapter(&adapter.get_info());
        let discrete_available = adapter.get_info().device_type == wgpu::DeviceType::IntegratedGpu
            && instance
                .enumerate_adapters(wgpu::Backends::all())
                .iter()
                .any(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu);
        crate::diagnostics::stage("game graphics device creation");
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            log::warn!(
                "Software rendering: '{}' runs graphics on the CPU and can be very slow. Update the graphics driver; on Windows set this game to High performance in Settings > System > Display > Graphics.",
                info.name
            );
        } else if discrete_available {
            log::warn!(
                "Integrated GPU '{}' selected although a discrete GPU is available. On Windows set this game to High performance in Settings > System > Display > Graphics and update the graphics driver. Adapter selection is unchanged.",
                info.name
            );
        }
        // Sample counts beyond 1 and 4 are adapter specific; opt in when
        // offered so 2x and 8x anti-aliasing can be used.
        let specific =
            adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        let (device, queue) = {
            let _step = crate::gpu_setup::begin("device request", "game window");
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: specific | crate::frame_gpu::features(adapter.features()),
                    ..Default::default()
                })
                .await?
        };
        crate::diagnostics::stage_done();
        crate::diagnostics::reassert_crash_reporter();
        device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                log::error!("game graphics device lost: {reason:?}: {message}");
            }
        });
        crate::diagnostics::stage("game graphics resources and pipelines");
        // The window stops responding while the shaders compile, for a minute or
        // more on DirectX 12 (docs/spec/graphics-options.md), so say why.
        let window_title = crate::graphics_api::dx12_chosen().then(|| window.title());
        if window_title.is_some() {
            window.set_title("T.O.R.E-Fighters - preparing DirectX 12 shaders, please wait");
        }
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no configuration")?;
        let present_modes = surface.get_capabilities(&adapter).present_modes;
        let frame_limit = crate::frame_pacing::Limit::for_run(graphics.frame_limit, false);
        config.present_mode = frame_limit.present_mode(&present_modes);
        config.desired_maximum_frame_latency = 1;

        log::info!(
            "Presentation: {:?} for frame rate {:?}, maximum queued frames: {}",
            config.present_mode,
            frame_limit,
            config.desired_maximum_frame_latency
        );
        let sample_counts: Vec<u32> = [1, 2, 4, 8]
            .into_iter()
            .filter(|&n| {
                if specific.is_empty() {
                    return matches!(n, 1 | 4);
                }
                [config.format, wgpu::TextureFormat::Depth32Float]
                    .iter()
                    .all(|&f| {
                        adapter
                            .get_texture_format_features(f)
                            .flags
                            .sample_count_supported(n)
                    })
            })
            .collect();
        log::info!(
            "Session system: adapter={:?} vendor=0x{:04x} device=0x{:04x} backend={:?} type={:?} driver={:?} driver_info={:?}; surface={:?} present={:?} window={}x{} scale={} logical_cores={} os={}/{}; graphics={:?} frame_limit={:?} effective_msaa={}; worker_pool={:?}",
            info.name,
            info.vendor,
            info.device,
            info.backend,
            info.device_type,
            info.driver,
            info.driver_info,
            config.format,
            config.present_mode,
            size.width,
            size.height,
            window.scale_factor(),
            std::thread::available_parallelism().map_or(1, usize::from),
            std::env::consts::OS,
            std::env::consts::ARCH,
            graphics,
            frame_limit,
            sample_counts
                .iter()
                .copied()
                .filter(|n| *n <= graphics.anti_aliasing.samples())
                .max()
                .unwrap_or(1),
            tore_workers::shared()
        );
        // Keep the selected system details even when a driver cannot create a swapchain.
        {
            let _step = crate::gpu_setup::begin("surface configuration", "game window");
            surface.configure(&device, &config);
        }
        let samples = sample_counts
            .iter()
            .copied()
            .filter(|&n| n <= graphics.anti_aliasing.samples())
            .max()
            .unwrap_or(1);
        log::info!("Anti-aliasing: {samples}x (supported {sample_counts:?})");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Retail menu canvas"),
            size: wgpu::Extent3d {
                width: WIDTH as u32,
                height: HEIGHT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Menu compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu.wgsl").into()),
        });
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Menu"),
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
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let veil = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("G-effect veil"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Menu image"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: veil.as_entire_binding(),
                },
            ],
        });
        let sim = crate::sim_renderer::SimRenderer::new(
            &device,
            &queue,
            config.format,
            scenery,
            graphics,
            samples,
        );
        let cockpit = crate::cockpit_renderer::CockpitRenderer::new(&device, config.format);
        let ui_text = crate::ui_text_renderer::UiTextRenderer::new(&device, &queue, config.format);
        let crt = crate::crt::Crt::new(&device, &queue, config.format);
        crate::diagnostics::stage_done();
        if let Some(title) = &window_title {
            window.set_title(title);
        }
        Ok(Self {
            crt,
            console_scene: None,
            console_overlay: None,
            console: None,
            console_last: None,
            console_repeat: false,
            first_frame: Default::default(),
            surface_failures: 0,
            mirror_camera: crate::camera::Camera::new(),
            mirror_vertices: Vec::new(),
            mirror_frames: 0,
            mirror_cadence: Default::default(),
            mirrors_enabled: std::env::var("TORE_MIRRORS").as_deref() != Ok("0"),
            graphics,
            sample_counts,
            cockpit,
            ui_text,
            menu_text_visible: false,
            leave_dialog: None,
            frame_rate: crate::frame_rate::Meter::default(),
            frame_gpu: crate::frame_gpu::Timer::new(&device, &queue),
            frame_cpu: None,
            previews: Default::default(),
            readbacks: Default::default(),
            sim,
            window,
            surface: WindowSurface::new(surface),
            device,
            queue,
            config,
            counter_logged: None,
            present_modes,
            frame_limit,
            automated: false,
            texture,
            canvas_held: None,
            bind_group,
            veil,
            veil_levels: [0.; 2],
            pipeline,
        })
    }
    /// Submit before the primary pass writes the shared camera/vertex buffers.
    /// GPU-only render-to-texture with no pixel readback: every other visible
    /// flight frame (`cadence`), always for a capture. One rear camera feeds
    /// every mirror. With `timed` its GPU time joins the frame counter's GPU
    /// figure.
    fn render_mirrors(
        &mut self,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
        timed: bool,
        cadence: bool,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        let visible = self.mirrors_enabled && self.cockpit.mirrors_visible;
        let due = if cadence {
            self.mirror_cadence.due(visible)
        } else {
            visible
        };
        if !visible {
            return;
        }
        // The main view draws after this with the aircraft buffer as the
        // mirror left it, drawn or not, so its picture (the flown
        // aircraft's shadow) is the same either way.
        self.sim
            .update_aircraft_vertices(&self.queue, &self.mirror_vertices);
        if !due {
            self.sim.hide_aircraft();
            return;
        }
        let view = self.cockpit.mirror_target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let timed = timed && (self.counter_shown() || crate::frame_log::enabled());
        if timed {
            self.frame_gpu.begin(&mut encoder);
        }
        self.sim.draw_view(
            &self.device,
            &self.queue,
            &mut encoder,
            crate::sim_renderer::ViewKind::Mirror,
            &view,
            crate::mirrors::SIZE,
            &self.mirror_camera,
            world,
            scenery,
        );
        if timed {
            self.frame_gpu.end(&mut encoder);
        }
        let encode_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        let commands = encoder.finish();
        drop(encode_log);
        let submit_log = crate::frame_log::span(crate::frame_log::Phase::Submit);
        self.queue.submit([commands]);
        drop(submit_log);
        self.sim.hide_aircraft();
        crate::frame_log::count(crate::frame_log::Count::Mirrors, 1);
        self.mirror_frames += 1;
    }
    /// Saves the console picture last drawn (the HUD camera's tape, the
    /// Sensor View) without the interface, as a PPM; false when the renderer
    /// has drawn none. For captures and the before-and-after checks of the
    /// tape's look.
    pub fn capture_console(&mut self, path: &std::path::Path) -> AppResult<bool> {
        use std::io::Write;
        let (Some(crt), Some((picture, size)), Some((layer, _))) = (
            &self.console_last,
            &self.console_scene,
            &self.console_overlay,
        ) else {
            return Ok(false);
        };
        let [width, height] = *size;
        let texture = self
            .readbacks
            .texture(&self.device, [width, height], self.config.format);
        let view = texture.create_view(&Default::default());
        let picture = picture.create_view(&Default::default());
        let layer = layer.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.crt.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &picture,
            Some(&layer),
            &view,
            [width, height],
            crt,
        );
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = self
            .readbacks
            .buffer(&self.device, u64::from(stride) * u64::from(height));
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
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let encode_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        let commands = encoder.finish();
        drop(encode_log);
        let submit_log = crate::frame_log::span(crate::frame_log::Phase::Submit);
        self.queue.submit([commands]);
        drop(submit_log);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let pending = Readback {
            buffer,
            receiver: rx,
            width,
            height,
            stride,
            bgra: matches!(
                self.config.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
        };
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })?;
        pending.receiver.recv()??;
        let pixels = self.readbacks.finish(pending);
        let mut file = std::fs::File::create(path)?;
        write!(file, "P6\n{width} {height}\n255\n")?;
        for p in pixels.chunks_exact(4) {
            file.write_all(&p[..3])?;
        }
        println!("Console capture: {}", path.display());
        Ok(true)
    }
    pub fn capture_sim(
        &mut self,
        path: &std::path::Path,
        camera: &crate::camera::Camera,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
        overlay: bool,
    ) -> AppResult<()> {
        use std::io::Write;
        let [width, height] = if overlay {
            self.flight_size()
        } else {
            [960, 720]
        };
        let pixels = self.scene_pixels(
            crate::sim_renderer::ViewKind::Main,
            camera,
            world,
            scenery,
            width,
            height,
            overlay,
        )?;
        let mut file = std::fs::File::create(path)?;
        write!(file, "P6\n{width} {height}\n255\n")?;
        for p in pixels.chunks_exact(4) {
            file.write_all(&p[..3])?;
        }
        println!("Scene capture: {}", path.display());
        Ok(())
    }
    /// A blocking capture of `kind`'s view (the main view for a screenshot,
    /// a camera panel's page for the smoke test's panels).
    #[allow(clippy::too_many_arguments)]
    pub fn scene_pixels(
        &mut self,
        kind: crate::sim_renderer::ViewKind,
        camera: &crate::camera::Camera,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
        width: u32,
        height: u32,
        overlay: bool,
    ) -> AppResult<Vec<u8>> {
        // A still shows the sun as it is, not eased from an earlier frame.
        self.sim.snap_sun_visibility();
        let pending =
            self.submit_readback(kind, camera, world, scenery, width, height, overlay, false)?;
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })?;
        pending.receiver.recv()??;
        Ok(self.readbacks.finish(pending))
    }
    /// Live panels consume a previous completed frame; never wait for GPU completion.
    pub fn poll_previews(&mut self) -> AppResult<Vec<(u8, Vec<u8>)>> {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        self.device.poll(wgpu::PollType::Poll)?;
        let mut ready = Vec::new();
        for (&page, pending) in &self.previews {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    result?;
                    ready.push(page);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(ready
            .into_iter()
            .map(|page| {
                let pending = self.previews.remove(&page).unwrap();
                (page, self.readbacks.finish(pending))
            })
            .collect())
    }
    pub fn request_preview(
        &mut self,
        page: u8,
        camera: &crate::camera::Camera,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
    ) -> AppResult<bool> {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        if !matches!(page, 2..=4) {
            return Err("invalid camera instrument".into());
        }
        if !self.previews.contains_key(&page) {
            let pending = self.submit_readback(
                crate::sim_renderer::ViewKind::Panel(page),
                camera,
                world,
                scenery,
                138,
                114,
                false,
                true,
            )?;
            crate::frame_log::count(crate::frame_log::Count::Panels, 1);
            self.previews.insert(page, pending);
            return Ok(true);
        }
        Ok(false)
    }
    #[allow(clippy::too_many_arguments)]
    fn submit_readback(
        &mut self,
        kind: crate::sim_renderer::ViewKind,
        camera: &crate::camera::Camera,
        world: &crate::terrain::Terrain,
        scenery: &crate::scenery::Scenery,
        width: u32,
        height: u32,
        overlay: bool,
        timed: bool,
    ) -> AppResult<Readback> {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Panels);
        if width == 0 || height == 0 || width > 1920 || height > 1080 {
            return Err("capture dimensions outside bounds".into());
        }
        let texture = self
            .readbacks
            .texture(&self.device, [width, height], self.config.format);
        if overlay {
            self.render_mirrors(world, scenery, false, false);
        }
        let view = texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        // Live camera panels are part of the frame's GPU work.
        let timed = timed && (self.counter_shown() || crate::frame_log::enabled());
        if timed {
            self.frame_gpu.begin(&mut encoder);
        }
        self.sim.draw_view(
            &self.device,
            &self.queue,
            &mut encoder,
            kind,
            &view,
            [width, height],
            camera,
            world,
            scenery,
        );
        self.write_veil(overlay);
        if overlay {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Flight UI capture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // The cockpit, then the canvas, in one pass.
            self.cockpit.draw_in(&mut pass, [width, height]);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
            self.draw_leave_dialog(&mut pass, [width, height]);
        }
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = self
            .readbacks
            .buffer(&self.device, u64::from(stride) * u64::from(height));
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
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if timed {
            self.frame_gpu.end(&mut encoder);
        }
        let encode_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        let commands = encoder.finish();
        drop(encode_log);
        let submit_log = crate::frame_log::span(crate::frame_log::Phase::Submit);
        self.queue.submit([commands]);
        drop(submit_log);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        Ok(Readback {
            buffer,
            receiver: rx,
            width,
            height,
            stride,
            bgra: matches!(
                self.config.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
        })
    }
    /// The next flight draw shows the world through the sensor console's CRT
    /// pass with `overlay` (straight-alpha sRGB RGBA of `size`) on it; the
    /// world camera should be thermal. Taken by that draw.
    pub fn set_console(&mut self, frame: crate::crt::Frame, overlay: Vec<u8>, size: [u32; 2]) {
        self.console = Some((frame, overlay, size));
    }
    /// The next flight draw shows the console picture it last drew again, the
    /// world and the overlay as they were, through the same CRT pass: the
    /// HUD camera's tape holds a picture between its 24 a second. False, and
    /// nothing changed, when the renderer has no such picture at the window's
    /// size (the caller then draws a new one with [`Renderer::set_console`]).
    pub fn repeat_console(&mut self) -> bool {
        let window = self.window.inner_size();
        let size = [window.width, window.height];
        let held = self.console_last.is_some()
            && self.console_overlay.is_some()
            && self.console_scene.as_ref().is_some_and(|(_, s)| *s == size);
        self.console_repeat = held;
        held
    }
    /// The console's world picture and overlay textures at these sizes.
    fn console_targets(&mut self, scene: [u32; 2], overlay: [u32; 2]) {
        if self.console_scene.as_ref().is_none_or(|(_, s)| *s != scene) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Sensor console world"),
                size: wgpu::Extent3d {
                    width: scene[0],
                    height: scene[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.console_scene = Some((texture, scene));
        }
        if self
            .console_overlay
            .as_ref()
            .is_none_or(|(_, s)| *s != overlay)
        {
            self.console_overlay =
                Some((crate::crt::overlay_texture(&self.device, overlay), overlay));
        }
    }
    /// Levels for the flight canvas veil (redout, blackout); the next flight
    /// draw applies them on the GPU.
    pub fn set_veil(&mut self, levels: [f32; 2]) {
        self.veil_levels = levels;
    }
    fn write_veil(&self, flight: bool) {
        let [redout, blackout] = if flight { self.veil_levels } else { [0.; 2] };
        crate::sim_renderer::write_floats(&self.queue, &self.veil, 0, &[redout, blackout, 0., 0.]);
    }
    /// The sharp text to draw over the menu canvas from now on (none for no
    /// text), and whether it is shown: the app turns it on only while a screen
    /// that records its text is up.
    pub fn set_menu_text(&mut self, layer: Option<&crate::ui_text::Layer>) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        let quads = layer.map_or_else(Vec::new, |layer| layer.quads(self.ui_text.linear()));
        self.ui_text.set(&self.device, &self.queue, &quads);
    }
    pub fn show_menu_text(&mut self, visible: bool) {
        self.menu_text_visible = visible;
    }
    pub fn flight_size(&self) -> [u32; 2] {
        let s = self.window.inner_size();
        let scale = (1920. / s.width.max(1) as f64)
            .min(1080. / s.height.max(1) as f64)
            .min(1.);
        [
            (s.width as f64 * scale).round().max(1.) as u32,
            (s.height as f64 * scale).round().max(1.) as u32,
        ]
    }
    fn canvas_texture(&mut self, size: [u32; 2]) {
        if self.texture.width() == size[0] && self.texture.height() == size[1] {
            return;
        }
        self.canvas_held = None;
        self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Responsive UI"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = self.texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Responsive UI"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.veil.as_entire_binding(),
                },
            ],
        });
    }
    pub fn viewport(&self) -> Viewport {
        let s = self.window.inner_size();
        Viewport::new(s.width, s.height)
    }
    pub fn resize(&mut self) {
        if self.configure() {
            self.window.request_redraw();
        }
    }
    /// Configures the surface for the window's size; false at no size.
    fn configure(&mut self) -> bool {
        let s = self.window.inner_size();
        if s.width > 0 && s.height > 0 {
            self.config.width = s.width;
            self.config.height = s.height;
            self.surface.configure(&self.device, &self.config);
            true
        } else {
            false
        }
    }
    /// After a frame that drew nothing because the surface gave no image
    /// (lost, outdated, timed out): how long to wait before the next try.
    /// None when the last frame had its image. The frame never asks for its
    /// own redraw on such a failure, so a surface that keeps failing (a
    /// window being minimised or changing mode on Windows) is tried a few
    /// times a second, not in a tight loop.
    pub fn surface_retry(&self) -> Option<std::time::Duration> {
        (self.surface_failures > 0)
            .then(|| crate::frame_pacing::surface_retry(self.surface_failures))
    }
    pub fn draw(
        &mut self,
        pixels: &[u8],
        scene: Option<(
            &crate::camera::Camera,
            &crate::terrain::Terrain,
            &crate::scenery::Scenery,
        )>,
        flight_size: Option<[u32; 2]>,
        canvas_rows: Option<&mut crate::canvas_rows::RowSet>,
        readout: crate::frame_rate::Surface,
    ) -> AppResult<bool> {
        if self.leave_dialog.is_some() {
            self.window.set_cursor_visible(true);
        }
        let draw_log = crate::frame_log::span(crate::frame_log::Phase::Render);
        self.first_frame.begin("game first frame presentation");
        let s = self.window.inner_size();
        if s.width == 0 || s.height == 0 {
            return Ok(false);
        }
        let cpu_before_acquire = self
            .frame_cpu
            .take()
            .map_or(0., |start| start.elapsed().as_secs_f64());
        let acquire_log = crate::frame_log::span(crate::frame_log::Phase::Acquire);
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => {
                self.surface_failures = 0;
                frame
            }
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.configure();
                self.surface_failures = self.surface_failures.saturating_add(1);
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => {
                self.surface_failures = self.surface_failures.saturating_add(1);
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        drop(acquire_log);
        crate::frame_rate::trace_present(readout);
        let cpu_started = std::time::Instant::now();
        let show_counter = crate::frame_rate::shown(self.graphics.show_frame_rate);
        let time_gpu = show_counter || crate::frame_log::enabled();
        let poll_log = crate::frame_log::span(crate::frame_log::Phase::Readback);
        let gpu_ms = self.frame_gpu.poll(&self.device);
        drop(poll_log);
        let upload_log = crate::frame_log::span(crate::frame_log::Phase::Upload);
        let size = flight_size.unwrap_or([WIDTH as u32, HEIGHT as u32]);
        self.canvas_texture(size);
        self.write_veil(flight_size.is_some());
        let stride = size[0] as usize * 4;
        // The flight canvas names the rows it may have changed; only those
        // rows that really differ from what the texture holds are sent. Any
        // other picture, or a first one, goes up whole.
        let bands = match canvas_rows {
            Some(rows) if pixels.len() == stride * size[1] as usize => {
                let bands = match &mut self.canvas_held {
                    Some(held) if held.len() == pixels.len() && rows.len() == size[1] as usize => {
                        crate::canvas_rows::changed_bands(
                            pixels,
                            held,
                            stride,
                            rows,
                            crate::canvas_rows::GAP,
                        )
                    }
                    held => {
                        *held = Some(pixels.to_vec());
                        std::iter::once(0..size[1] as usize).collect()
                    }
                };
                rows.clear();
                bands
            }
            _ => {
                self.canvas_held = None;
                std::iter::once(0..size[1] as usize).collect()
            }
        };
        for band in bands {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: band.start as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels[band.start * stride..band.end * stride],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size[0] * 4),
                    rows_per_image: Some((band.end - band.start) as u32),
                },
                wgpu::Extent3d {
                    width: size[0],
                    height: (band.end - band.start) as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        drop(upload_log);
        if flight_size.is_some()
            && let Some((_, world, scenery)) = scene
        {
            self.render_mirrors(world, scenery, true, true);
        }
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if time_gpu {
            self.frame_gpu.begin(&mut encoder);
        } else {
            self.frame_gpu.cancel();
        }
        let console = self.console.take().filter(|_| flight_size.is_some());
        let repeat = std::mem::take(&mut self.console_repeat) && console.is_none();
        if repeat
            && scene.is_some()
            && let (Some(crt), Some((picture, _)), Some((layer, _))) = (
                &self.console_last,
                &self.console_scene,
                &self.console_overlay,
            )
        {
            // The same picture again: only the CRT pass runs.
            let picture = picture.create_view(&Default::default());
            let layer = layer.create_view(&Default::default());
            self.crt.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &picture,
                Some(&layer),
                &view,
                [s.width, s.height],
                crt,
            );
        } else if let (Some((crt, overlay, overlay_size)), Some((camera, world, scenery))) =
            (&console, scene)
        {
            self.console_last = Some(*crt);
            // The sensor console: the world into its own picture, then the
            // CRT pass with the symbology onto the frame.
            let size = [s.width, s.height];
            self.console_targets(size, *overlay_size);
            let (picture, _) = self.console_scene.as_ref().expect("made above");
            let (layer, _) = self.console_overlay.as_ref().expect("made above");
            crate::crt::write_overlay(&self.queue, layer, *overlay_size, overlay);
            let picture = picture.create_view(&Default::default());
            let layer = layer.create_view(&Default::default());
            self.sim.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &picture,
                size,
                camera,
                world,
                scenery,
            );
            self.crt.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &picture,
                Some(&layer),
                &view,
                size,
                crt,
            );
        } else if let Some((camera, world, scenery)) = scene {
            self.sim.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                [s.width, s.height],
                camera,
                world,
                scenery,
            );
        }
        let ui_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        // The frame rate's glyphs are uploaded before the one pass that draws
        // the cockpit, the canvas and them.
        if show_counter {
            self.frame_rate.tick();
        } else {
            self.frame_rate = Default::default();
        }
        let counter = if show_counter && let Some(line) = self.frame_rate.line() {
            // A run with TORE_SHOW_FPS=1 also logs the line once a second, so
            // a scripted window run can show the counter live without a
            // screenshot.
            if std::env::var("TORE_SHOW_FPS").is_ok_and(|v| v == "1")
                && self
                    .counter_logged
                    .is_none_or(|at| at.elapsed().as_secs_f64() >= 1.0)
            {
                self.counter_logged = Some(std::time::Instant::now());
                log::info!("Frame counter ({readout:?}): {line}");
            }
            let place = crate::frame_rate::anchor([s.width, s.height], readout);
            let glyphs = crate::frame_rate::quads(&line, place, self.ui_text.linear());
            self.ui_text.prepare_window(
                &self.device,
                &self.queue,
                &glyphs,
                [s.width as f32, s.height as f32],
            )
        } else {
            None
        };
        let console_shown = console.is_some() || repeat;
        if let Some((_, overlay, _)) = console {
            // Uploaded above; the next overlay draws into the same buffer.
            crate::sensor_console::recycle(overlay);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main menu"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if scene.is_some() {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // Without a scene the pass clears, so a cockpit would not show.
            if flight_size.is_some() && !console_shown && scene.is_some() {
                self.cockpit.draw_in(&mut pass, [s.width, s.height]);
            }
            if flight_size.is_none() {
                let v = self.viewport();
                pass.set_viewport(v.x, v.y, v.width, v.height, 0., 1.);
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
            if flight_size.is_none() && self.menu_text_visible {
                self.ui_text.draw(&mut pass);
            }
            self.draw_leave_dialog(&mut pass, [s.width, s.height]);
            // After every other layer, over the whole window, and only on the
            // window. Captures read the scene or the CPU canvas and never this.
            if let Some(glyphs) = counter {
                pass.set_viewport(0., 0., s.width as f32, s.height as f32, 0., 1.);
                self.ui_text.draw_window_in(&mut pass, glyphs);
            }
        }
        drop(ui_log);
        if time_gpu {
            self.frame_gpu.end(&mut encoder);
        }
        let encode_log = crate::frame_log::span(crate::frame_log::Phase::Encoding);
        let commands = encoder.finish();
        drop(encode_log);
        let submit_log = crate::frame_log::span(crate::frame_log::Phase::Submit);
        self.queue.submit([commands]);
        drop(submit_log);
        if time_gpu {
            let _poll_log = crate::frame_log::span(crate::frame_log::Phase::Readback);
            self.frame_gpu.submitted();
        }
        if show_counter {
            self.frame_rate.work(
                (cpu_before_acquire + cpu_started.elapsed().as_secs_f64()) * 1000.,
                gpu_ms,
            );
        }
        // Wayland frame callbacks throttle redraw requests to compositor
        // refresh. Request that pacing with FIFO (V-sync): a hidden window
        // then gets no redraws rather than blocking in its next present.
        if self.config.present_mode == wgpu::PresentMode::Fifo {
            self.window.pre_present_notify();
        }
        let present_log = crate::frame_log::span(crate::frame_log::Phase::Present);
        frame.present();
        drop(present_log);
        if self.first_frame.complete() {
            crate::graphics_api::setup_ended();
        }
        drop(draw_log);
        crate::frame_log::finish(gpu_ms);
        Ok(true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn letterbox_coordinates_match_rendered_canvas() {
        let v = Viewport::new(1600, 900);
        assert_eq!(v.point(200.0, 0.0), Some((0.0, 0.0)));
        assert_eq!(v.point(800.0, 450.0), Some((320.0, 240.0)));
        assert_eq!(v.point(199.0, 200.0), None);
        assert_eq!(v.point(1400.0, 200.0), None);
        assert_eq!(Viewport::new(0, 0).point(0.0, 0.0), None);
    }
}
