//! Flight preparation without holding the window event callback.
//! Behaviour: docs/spec/loading-screen.md.
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

// CPU weights measured on the private UKR/F18 profile probe (2026-10-16).
// The loadout allowance is fitted. Graphics is a fitted 500 ms reservation
// until measured on a GPU host.
const WEIGHTS: [u32; 5] = [25, 2442, 11, 164, 500];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Selection,
    Content,
    Connecting,
    Waiting,
    Replay,
    Mission,
    Aircraft,
    Scenery,
    Graphics,
    Ready,
}
impl Stage {
    pub fn words(self) -> &'static str {
        match self {
            Self::Selection => "Preparing loadout",
            Self::Content => "Checking game content",
            Self::Connecting => "Connecting to the host",
            Self::Waiting => "Waiting for the host",
            Self::Replay => "Loading the replay",
            Self::Mission => "Building the mission",
            Self::Aircraft => "Loading aircraft",
            Self::Scenery => "Loading terrain and scenery",
            Self::Graphics => "Preparing graphics",
            Self::Ready => "Starting flight",
        }
    }
    #[cfg(test)]
    pub fn percent(self) -> u32 {
        self.completed_weight() * 100 / WEIGHTS.iter().sum::<u32>()
    }
    fn completed_weight(self) -> u32 {
        WEIGHTS[..self.index()].iter().sum()
    }
    fn index(self) -> usize {
        match self {
            Self::Selection | Self::Content | Self::Connecting | Self::Waiting => 0,
            Self::Replay | Self::Mission => 1,
            Self::Aircraft => 2,
            Self::Scenery => 3,
            Self::Graphics => 4,
            Self::Ready => 5,
        }
    }
    fn estimate(self, elapsed: f64) -> f64 {
        if self == Self::Ready {
            return 100.;
        }
        if matches!(self, Self::Content | Self::Connecting | Self::Waiting) {
            return 0.;
        }
        let weight = f64::from(WEIGHTS[self.index()]);
        let fraction = elapsed.max(0.) / (elapsed.max(0.) + weight / 1000.);
        (f64::from(self.completed_weight()) + weight * fraction) * 100.
            / f64::from(WEIGHTS.iter().sum::<u32>())
    }
}
#[derive(Clone)]
pub(crate) struct Progress(Arc<Mutex<(Stage, Instant)>>, usize);
fn random_photo() -> usize {
    // Presentation only. Independent of all simulation seeds and worker order.
    use std::hash::{BuildHasher, Hasher};
    (std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
        & 1) as usize
}
impl Progress {
    fn new() -> Self {
        Self(
            Arc::new(Mutex::new((Stage::Selection, Instant::now()))),
            random_photo(),
        )
    }
    pub fn network() -> Self {
        Self(
            Arc::new(Mutex::new((Stage::Content, Instant::now()))),
            random_photo(),
        )
    }
    /// A mission build, restart or replay, already on Building the mission
    /// so the first frame is not Preparing loadout.
    pub(crate) fn mission() -> Self {
        let progress = Self::new();
        progress.stage(Stage::Mission);
        progress
    }
    pub fn draw(&self, pixels: &mut Vec<u8>, size: [u32; 2], details: [&str; 2]) -> [u32; 2] {
        let (stage, elapsed) = self.current();
        draw_window(pixels, size, self.1, stage, elapsed, details)
    }
    pub fn stage(&self, stage: Stage) {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Network labels at the same weight may change, but the clock stays.
        if stage.index() == 0 && current.0.index() == 0 && current.0 != Stage::Selection {
            current.0 = stage;
            return;
        }
        // Replay preparation replaces the mission label on the same clock.
        if stage == Stage::Replay && current.0 == Stage::Mission {
            current.0 = stage;
            return;
        }
        // A duplicate or late report cannot restart a stage or rewind the bar.
        if stage.index() <= current.0.index() {
            return;
        }
        log::info!(
            "loading: {:?} {:.3} ms",
            current.0,
            current.1.elapsed().as_secs_f64() * 1000.
        );
        #[cfg(test)]
        eprintln!(
            "loading stage {:?}: {:.3} ms",
            current.0,
            current.1.elapsed().as_secs_f64() * 1000.
        );
        *current = (stage, Instant::now());
    }
    fn current(&self) -> (Stage, f64) {
        let current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (current.0, current.1.elapsed().as_secs_f64())
    }
}
/// How long a load that continues from a menu page may run before the
/// loading photograph replaces that page (`Job::continue_from_menu`). A short
/// load shows the page it was started from until the next page is ready, not a
/// photograph for a handful of frames.
pub(crate) const MENU_GRACE: Duration = Duration::from_millis(250);
pub(crate) struct Job<T> {
    receive: mpsc::Receiver<Result<T, String>>,
    pub progress: Progress,
    photo: usize,
    /// Until this time the screen the load started from stays up.
    quiet_until: Option<Instant>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl<T: Send + 'static> Job<T> {
    pub fn spawn(
        work: impl FnOnce(Progress) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::continue_load(Some(Progress::mission()), true, work)
    }
    /// `progress` is the load already on screen. `complete` marks Ready when
    /// the worker succeeds. A selection that continues into the mission build
    /// leaves the bar where it is.
    pub fn continue_load(
        progress: Option<Progress>,
        complete: bool,
        work: impl FnOnce(Progress) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::start(progress, complete, false, work)
    }
    /// A load that opens the next menu page, started from a menu page: that
    /// page stays up for [`MENU_GRACE`] and the loading photograph comes only
    /// if the load runs longer (`screen_kept`). The photographs decode beside
    /// the work, not before it, so a short load is not held up by them.
    pub fn continue_from_menu(
        work: impl FnOnce(Progress) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        request_background_decode();
        Self::start(None, false, true, work)
    }
    fn start(
        progress: Option<Progress>,
        complete: bool,
        from_menu: bool,
        work: impl FnOnce(Progress) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        let (send, receive) = mpsc::channel();
        let progress = progress.unwrap_or_else(Progress::new);
        let photo = progress.1;
        let status = progress.clone();
        let thread = std::thread::Builder::new()
            .name("flight-loader".into())
            .spawn(move || {
                // Decode before mission work, on this thread, not the window thread.
                if !from_menu {
                    prepare_photos();
                }
                let result = work(status.clone());
                // A failed load is not completion: leave the last real stage up.
                if complete && result.is_ok() {
                    status.stage(Stage::Ready);
                }
                let _ = send.send(result);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            receive,
            progress,
            photo,
            quiet_until: from_menu.then(|| Instant::now() + MENU_GRACE),
            thread: Some(thread),
        })
    }
    /// Whether the screen the load started from is still the one to show at
    /// `now`.
    pub fn screen_kept(&self, now: Instant) -> bool {
        self.quiet_until.is_some_and(|until| now < until)
    }
    /// The worker, still running or finished. Dropping the job discards its
    /// result; the caller joins this handle after the window closes.
    pub fn into_thread(mut self) -> Option<std::thread::JoinHandle<()>> {
        self.thread.take()
    }
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.receive.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("The flight loader stopped unexpectedly.".into()))
            }
        }
    }
    pub fn draw(&self, pixels: &mut Vec<u8>, size: [u32; 2], details: [&str; 2]) -> [u32; 2] {
        let (stage, elapsed) = self.progress.current();
        draw_window(pixels, size, self.photo, stage, elapsed, details)
    }
}
struct Rgb {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}
struct Photos {
    frames: [Option<Rgb>; 2],
    #[cfg(test)]
    decode_ms: f64,
}
fn bundled_jpegs() -> [(&'static str, &'static [u8]); 2] {
    [
        (
            "harrier.jpg",
            include_bytes!("../assets/loading/harrier.jpg"),
        ),
        (
            "thunderbirds.jpg",
            include_bytes!("../assets/loading/thunderbirds.jpg"),
        ),
    ]
}
fn decode_jpeg(bytes: &[u8]) -> Result<Rgb, String> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().map_err(|error| error.to_string())?;
    let info = decoder
        .info()
        .ok_or_else(|| "jpeg has no dimensions".to_string())?;
    let width = usize::from(info.width);
    let height = usize::from(info.height);
    if width == 0 || height == 0 || pixels.len() != width * height * 3 {
        return Err(format!(
            "jpeg decoded to {width}x{height}, {} bytes",
            pixels.len()
        ));
    }
    Ok(Rgb {
        width,
        height,
        pixels,
    })
}
fn load_photos() -> Photos {
    let started = Instant::now();
    let mut frames = [None, None];
    for (index, (name, bytes)) in bundled_jpegs().into_iter().enumerate() {
        match decode_jpeg(bytes) {
            Ok(photo) => frames[index] = Some(photo),
            Err(error) => log::error!("loading: {name} decode failed: {error}"),
        }
    }
    let decode_ms = started.elapsed().as_secs_f64() * 1000.;
    let describe = |photo: &Option<Rgb>| match photo {
        Some(photo) => format!("{}x{}", photo.width, photo.height),
        None => "failed".to_string(),
    };
    log::info!(
        "loading: jpeg decode {decode_ms:.3} ms on {} ({}, {})",
        std::thread::current().name().unwrap_or("unnamed"),
        describe(&frames[0]),
        describe(&frames[1]),
    );
    Photos {
        frames,
        #[cfg(test)]
        decode_ms,
    }
}
static PHOTOS: OnceLock<Photos> = OnceLock::new();
static DECODE_STARTED: AtomicBool = AtomicBool::new(false);
fn cached_photos() -> Option<&'static Photos> {
    PHOTOS.get()
}
/// Decode both photographs on this thread. The flight loader calls this
/// before mission work. The headless snapshot calls it because that path
/// has no window. The window thread must not.
pub(crate) fn prepare_photos() {
    DECODE_STARTED.store(true, Ordering::Release);
    let _ = PHOTOS.get_or_init(load_photos);
}
fn request_background_decode() {
    if PHOTOS.get().is_some() || DECODE_STARTED.load(Ordering::Acquire) {
        return;
    }
    if DECODE_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("loading-photos".into())
        .spawn(|| {
            let _ = PHOTOS.get_or_init(load_photos);
        })
    {
        DECODE_STARTED.store(false, Ordering::Release);
        log::error!("loading: jpeg decode thread failed: {error}");
    }
}
/// `loading`, `loading-thunderbirds`, or either name with a `-WIDTHxHEIGHT`
/// window. The battery states stay 640 by 480.
pub(crate) fn snapshot_window(state: &str) -> Option<(usize, [u32; 2])> {
    let rest = state.strip_prefix("loading")?;
    let (label, window) = match rest.rsplit_once('-') {
        Some((label, size)) => match parse_window(size) {
            Some(window) => (label, window),
            None => (rest, [640, 480]),
        },
        None => (rest, [640, 480]),
    };
    Some((usize::from(label.contains("thunderbirds")), window))
}
fn parse_window(size: &str) -> Option<[u32; 2]> {
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    ((320..=7680).contains(&width) && (240..=4320).contains(&height)).then_some([width, height])
}
fn blit_photo(pixels: &mut [u8], width: usize, height: usize, photo: Option<&Rgb>) {
    let Some(photo) = photo else {
        pixels.fill(0);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        return;
    };
    let (w, h) = (photo.width, photo.height);
    let scale = (width as f64 / w as f64).max(height as f64 / h as f64);
    for y in 0..height {
        for x in 0..width {
            let sx = ((x as f64 - width as f64 / 2.) / scale + w as f64 / 2.)
                .clamp(0., (w - 1) as f64) as usize;
            let sy = ((y as f64 - height as f64 / 2.) / scale + h as f64 / 2.)
                .clamp(0., (h - 1) as f64) as usize;
            let shade = 0.85 - 0.55 * y as f64 / height as f64;
            let dst = (y * width + x) * 4;
            let src = (sy * w + sx) * 3;
            for c in 0..3 {
                pixels[dst + c] = (photo.pixels[src + c] as f64 * shade) as u8;
            }
            pixels[dst + 3] = 255;
        }
    }
}
fn dimensions(bytes: &[u8]) -> (usize, usize) {
    (
        u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize,
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
    )
}
/// Keep the photograph's aspect ratio and cover the whole window. Text stays
/// in a centered 640x480 logical area, matching the bundled menu font metrics.
pub(crate) fn draw_window(
    pixels: &mut Vec<u8>,
    window: [u32; 2],
    photo: usize,
    stage: Stage,
    elapsed: f64,
    details: [&str; 2],
) -> [u32; 2] {
    let ratio = (window[0].max(1) as f64 / window[1].max(1) as f64).clamp(0.25, 6.);
    let size = if ratio >= 4. / 3. {
        [(480. * ratio).round() as u32, 480]
    } else {
        [640, (640. / ratio).round() as u32]
    };
    pixels.resize(size[0] as usize * size[1] as usize * 4, 0);
    // The window thread only samples. A loader that has not finished yet
    // leaves a dark field until the next redraw.
    if cached_photos().is_none() {
        request_background_decode();
    }
    draw_sized(pixels, size, photo, stage, elapsed, details);
    size
}
/// The composed background of the loading screen: the photograph cropped to
/// the canvas with its shading, and the logo over the centre. It depends only
/// on the canvas size and the photograph, so it is made once and each frame
/// copies it and draws the changing text and bar over it.
struct Background {
    size: [u32; 2],
    photo: usize,
    /// Whether the photograph had been decoded when this was composed. A
    /// dark field composed before that is replaced once it has.
    decoded: bool,
    pixels: Vec<u8>,
}
thread_local! {
    static BACKGROUND: std::cell::RefCell<Option<Background>> =
        const { std::cell::RefCell::new(None) };
}
/// The text face of the loading screen, built once.
fn loading_font() -> &'static crate::menu::Sprite {
    static FONT: OnceLock<crate::menu::Sprite> = OnceLock::new();
    FONT.get_or_init(|| crate::menu::flat_font_large([245, 245, 245]))
}
/// The photograph and the logo, as the loading screen draws them behind its
/// text.
fn compose_background(size: [u32; 2], decoded: Option<&Rgb>) -> Vec<u8> {
    use crate::menu::{HEIGHT, WIDTH};
    let (width, height) = (size[0] as usize, size[1] as usize);
    let mut pixels = vec![0; width * height * 4];
    blit_photo(&mut pixels, width, height, decoded);
    let left = (width - WIDTH) / 2;
    let top = (height - HEIGHT) / 2;
    let logo = include_bytes!("../assets/loading/logo.rgba");
    let (w, h) = dimensions(logo);
    for y in 0..h {
        for x in 0..w {
            let src = 8 + (y * w + x) * 4;
            let dst = ((y + 55 + top) * width + left + x + (WIDTH - w) / 2) * 4;
            let a = logo[src + 3] as u32;
            for c in 0..3 {
                pixels[dst + c] =
                    ((logo[src + c] as u32 * a + pixels[dst + c] as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
    pixels
}
fn draw_sized(
    pixels: &mut [u8],
    size: [u32; 2],
    photo: usize,
    stage: Stage,
    elapsed: f64,
    details: [&str; 2],
) {
    let decoded = cached_photos().and_then(|photos| photos.frames[photo & 1].as_ref());
    BACKGROUND.with_borrow_mut(|cache| {
        let current = cache.as_ref().is_some_and(|b| {
            b.size == size && b.photo == photo & 1 && b.decoded == decoded.is_some()
        });
        if !current {
            *cache = Some(Background {
                size,
                photo: photo & 1,
                decoded: decoded.is_some(),
                pixels: compose_background(size, decoded),
            });
        }
        let background = cache.as_ref().expect("a composed background");
        pixels.copy_from_slice(&background.pixels);
    });
    draw_text_and_bar(pixels, size, stage, elapsed, details);
}
/// What changes from frame to frame: the stage words, the bar, the clock and
/// the two lines of details, drawn over the 640 by 480 centre.
fn draw_text_and_bar(
    output: &mut [u8],
    size: [u32; 2],
    stage: Stage,
    elapsed: f64,
    details: [&str; 2],
) {
    use crate::menu::{Canvas, HEIGHT, WIDTH};
    let width = size[0] as usize;
    let left = (width - WIDTH) / 2;
    let top = (size[1] as usize - HEIGHT) / 2;
    let mut overlay = vec![0; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        overlay[y * WIDTH * 4..(y + 1) * WIDTH * 4].copy_from_slice(
            &output[((y + top) * width + left) * 4..((y + top) * width + left + WIDTH) * 4],
        );
    }
    let mut canvas = Canvas(&mut overlay[..]);
    let font = loading_font();
    canvas.centered_text(font, stage.words(), (40, 330, 560, 24));
    canvas.rect((80, 370, 480, 16), [30, 35, 40, 255]);
    canvas.rect(
        (80, 370, (4.8 * stage.estimate(elapsed)) as i32, 16),
        [180, 200, 220, 255],
    );
    // No seconds counter (John, 2026-10-19). The externally timed stages show
    // the stage words and a stationary bar only.
    if !matches!(stage, Stage::Content | Stage::Connecting | Stage::Waiting) {
        canvas.centered_text(
            font,
            &format!("Estimated {:.1}%", stage.estimate(elapsed)),
            (80, 405, 480, 24),
        );
    }
    canvas.centered_text(font, details[0], (20, 285, 600, 18));
    canvas.centered_text(font, details[1], (20, 307, 600, 18));
    for y in 0..HEIGHT {
        output[((y + top) * width + left) * 4..((y + top) * width + left + WIDTH) * 4]
            .copy_from_slice(&overlay[y * WIDTH * 4..(y + 1) * WIDTH * 4]);
    }
}
/// The loading screen as every frame drew it before the background was
/// cached: the reference the cached drawing must equal pixel for pixel.
#[cfg(test)]
fn draw_sized_reference(
    pixels: &mut [u8],
    size: [u32; 2],
    photo: usize,
    stage: Stage,
    elapsed: f64,
    details: [&str; 2],
) {
    use crate::menu::{Canvas, HEIGHT, WIDTH};
    let (width, height) = (size[0] as usize, size[1] as usize);
    let decoded = cached_photos().and_then(|photos| photos.frames[photo & 1].as_ref());
    blit_photo(pixels, width, height, decoded);
    let left = (width - WIDTH) / 2;
    let top = (height - HEIGHT) / 2;
    let mut overlay = vec![0; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        overlay[y * WIDTH * 4..(y + 1) * WIDTH * 4].copy_from_slice(
            &pixels[((y + top) * width + left) * 4..((y + top) * width + left + WIDTH) * 4],
        );
    }
    let output = pixels;
    let pixels = &mut overlay[..];
    let logo = include_bytes!("../assets/loading/logo.rgba");
    let (w, h) = dimensions(logo);
    for y in 0..h {
        for x in 0..w {
            let src = 8 + (y * w + x) * 4;
            let dst = ((y + 55) * WIDTH + x + (WIDTH - w) / 2) * 4;
            let a = logo[src + 3] as u32;
            for c in 0..3 {
                pixels[dst + c] =
                    ((logo[src + c] as u32 * a + pixels[dst + c] as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
    let mut canvas = Canvas(pixels);
    let font = crate::menu::flat_font_large([245, 245, 245]);
    canvas.centered_text(&font, stage.words(), (40, 330, 560, 24));
    canvas.rect((80, 370, 480, 16), [30, 35, 40, 255]);
    canvas.rect(
        (80, 370, (4.8 * stage.estimate(elapsed)) as i32, 16),
        [180, 200, 220, 255],
    );
    // No seconds counter (John, 2026-10-19). The externally timed stages show
    // the stage words and a stationary bar only.
    if !matches!(stage, Stage::Content | Stage::Connecting | Stage::Waiting) {
        canvas.centered_text(
            &font,
            &format!("Estimated {:.1}%", stage.estimate(elapsed)),
            (80, 405, 480, 24),
        );
    }
    canvas.centered_text(&font, details[0], (20, 285, 600, 18));
    canvas.centered_text(&font, details[1], (20, 307, 600, 18));
    for y in 0..HEIGHT {
        output[((y + top) * width + left) * 4..((y + top) * width + left + WIDTH) * 4]
            .copy_from_slice(&overlay[y * WIDTH * 4..(y + 1) * WIDTH * 4]);
    }
}

pub(crate) fn prepare_mission(
    spec: &crate::mission::MissionSpec,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    player: Arc<tore_world::aircraft_type::AircraftType>,
    progress: &Progress,
) -> Result<
    (
        crate::world::Built,
        crate::combat_view::CombatView,
        crate::scenery::Scenery,
    ),
    String,
> {
    progress.stage(Stage::Mission);
    let mut models = Vec::new();
    let font = crate::ordnance::label_font();
    let label = crate::ordnance::weapon_label(resources, &font);
    // The creator no longer builds the layout before OK: a seed whose
    // layout cannot place every vehicle, or leaves a target's Blue start no
    // room for the distance, gives way to the next, as a host's does
    // (`seed_retry`). Nearly every flight builds its first seed.
    let mut accepted = spec.clone();
    let (built, _) = tore_world::mission_layout::seed_retry::build(&mut accepted, |candidate| {
        // A refused attempt's models go with it: the view's models follow
        // the built combat's order.
        models.clear();
        let mut load =
            |id| -> tore_world::WorldResult<Arc<tore_world::aircraft_type::AircraftType>> {
                let model = crate::aircraft::Airframe::load(resources, id)?;
                let kind = Arc::clone(&model.kind);
                models.push(model);
                Ok(kind)
            };
        crate::world::World::build(
            candidate,
            resources,
            crate::world::Seating::SinglePlayer,
            &mut crate::world::Hooks {
                inspect_invalid_placement: false,
                player: Some(Arc::clone(&player)),
                load: Some(&mut load),
                weapon_label: Some(&label),
                ground_variation: None,
            },
        )
        .map_err(|e| e.to_string())
    })?;
    progress.stage(Stage::Aircraft);
    let view = crate::combat_view::CombatView::with_models(
        &built.world.combat,
        built.world.picture_plane().0,
        resources,
        models,
    )
    .map_err(|e| e.to_string())?;
    progress.stage(Stage::Scenery);
    let scenery = crate::scenery::Scenery::build(resources, &built.world.terrain)
        .map_err(|e| e.to_string())?;
    progress.stage(Stage::Graphics);
    Ok((built, view, scenery))
}

pub(crate) type NetParts = (
    crate::world::World,
    crate::flight_map::Plan,
    crate::aircraft::Airframe,
    crate::combat_view::CombatView,
    crate::scenery::Scenery,
    Option<crate::renderer::LoadingGraphics>,
);

pub(crate) type MissionParts = (
    crate::world::Built,
    crate::combat_view::CombatView,
    crate::scenery::Scenery,
    Option<crate::renderer::LoadingGraphics>,
    Option<crate::replay::recorder::Recorder>,
    Vec<crate::mission_brief::Sentence>,
);

pub(crate) enum Prepared {
    NetRebuilt(Box<crate::net::session::Built>),
    Selection(Box<crate::aircraft::Airframe>),
    Restart(
        Box<(
            crate::world::World,
            crate::world::Restarted,
            Option<crate::replay::recorder::Recorder>,
        )>,
    ),
    Net(Box<NetParts>),
    Mission(Box<MissionParts>),
    Replay(
        Box<crate::replay::viewer::Viewer>,
        Option<Box<crate::renderer::LoadingGraphics>>,
    ),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_load_from_a_menu_page_keeps_that_page_up_for_the_grace_time() {
        // A plain load shows the loading photograph at once.
        let plain = Job::continue_load(None, false, |_| Ok(())).unwrap();
        assert!(!plain.screen_kept(Instant::now()));
        // One started from a menu page keeps the page for MENU_GRACE, then
        // lets the photograph come (it is not hidden for good).
        let from_menu = Job::continue_from_menu(|_| Ok(())).unwrap();
        let started = Instant::now();
        assert!(from_menu.screen_kept(started));
        assert!(from_menu.screen_kept(started + MENU_GRACE / 2));
        assert!(!from_menu.screen_kept(started + MENU_GRACE + Duration::from_millis(5)));
        // The grace is short enough that a slow load still shows its bar.
        assert!(MENU_GRACE <= Duration::from_millis(300));
    }
    #[test]
    fn one_photo_and_monotonic_progress_across_selection_and_mission() {
        let progress = Progress::new();
        let photo = progress.1;
        let stages = [
            Stage::Selection,
            Stage::Mission,
            Stage::Aircraft,
            Stage::Scenery,
            Stage::Graphics,
            Stage::Ready,
        ];
        let mut previous = 0.;
        for stage in stages {
            progress.stage(stage);
            let (current, elapsed) = progress.current();
            assert_eq!(current, stage);
            let value = current.estimate(elapsed);
            assert!(value >= previous);
            previous = value;
            // Duplicate and late producer notifications cannot reset a clock
            // or replay an earlier visible stage.
            progress.stage(stage);
            progress.stage(Stage::Selection);
            let (current, elapsed) = progress.current();
            assert!(current.estimate(elapsed) >= previous);
            assert_eq!(progress.1, photo);
        }
        let job = Job::continue_load(Some(progress.clone()), true, |_| Ok(())).unwrap();
        assert_eq!(job.photo, photo);
        let finished = loop {
            if let Some(result) = job.poll() {
                break result;
            }
        };
        finished.expect("continued load");
        assert_eq!(job.progress.current().0.estimate(0.), 100.);
        assert_eq!(job.progress.1, photo);
    }
    #[test]
    fn network_wait_cannot_rewind_the_bar_or_the_photo() {
        let progress = Progress::network();
        let photo = progress.1;
        let mut previous = 0.;
        for stage in [
            Stage::Content,
            Stage::Connecting,
            Stage::Mission,
            Stage::Waiting,
            Stage::Aircraft,
            Stage::Scenery,
            Stage::Graphics,
            Stage::Ready,
        ] {
            progress.stage(stage);
            let (current, elapsed) = progress.current();
            let value = current.estimate(elapsed);
            assert!(
                value >= previous,
                "{current:?} rewound to {value} from {previous}"
            );
            previous = value;
            assert_eq!(progress.1, photo);
        }
        // Waiting is behind Mission, so the mission label and clock stay.
        let held = Progress::network();
        let held_photo = held.1;
        held.stage(Stage::Mission);
        let (mission, started) = held.current();
        assert_eq!(mission, Stage::Mission);
        held.stage(Stage::Waiting);
        held.stage(Stage::Content);
        let (still, later) = held.current();
        assert_eq!(still, Stage::Mission);
        assert!(later >= started);
        assert_eq!(held.1, held_photo);
    }
    #[test]
    fn a_fresh_mission_load_opens_on_building_the_mission_and_a_rebuild_does_not_finish() {
        let progress = Progress::mission();
        let photo = progress.1;
        assert_eq!(progress.current().0, Stage::Mission);
        let job = Job::continue_load(Some(progress.clone()), false, move |status| {
            assert_eq!(status.current().0, Stage::Mission);
            assert_eq!(status.1, photo);
            Ok(())
        })
        .unwrap();
        assert_eq!(job.photo, photo);
        let finished = loop {
            if let Some(result) = job.poll() {
                break result;
            }
        };
        finished.expect("rebuild");
        assert_eq!(job.progress.current().0, Stage::Mission);
        assert_eq!(job.progress.1, photo);
    }
    #[test]
    fn the_bar_has_no_moving_highlight() {
        prepare_photos();
        let mut early = Vec::new();
        let mut later = Vec::new();
        draw_window(&mut early, [640, 480], 0, Stage::Scenery, 0.2, ["", ""]);
        draw_window(&mut later, [640, 480], 0, Stage::Scenery, 3.4, ["", ""]);
        // The old activity strip was a 20 by 2 highlight just under the bar,
        // and it moved with the clock. Those rows stay put.
        for row in 390..392 {
            let start = row * 640 * 4;
            let end = start + 640 * 4;
            assert_eq!(early[start..end], later[start..end], "row {row}");
        }
    }
    #[test]
    fn creator_smoke_restart_keeps_the_accepted_start() {
        let resources = tore_world::test_support::resources::resources();
        let id = tore_formats::aircraft::AircraftId::F18;
        let spec = crate::mission::MissionSpec::new("UKR", id);
        let aircraft = tore_world::aircraft_type::load_type(&resources, id).unwrap();
        let mut world =
            crate::world::World::new(&spec, &resources, crate::world::Seating::SinglePlayer)
                .unwrap();
        verify_restart(&mut world, &aircraft, &resources).unwrap();
        world.cockpits[crate::OWN].flight.fuel -= 100.;
        assert!(verify_restart(&mut world, &aircraft, &resources).is_err());
    }
    #[test]
    fn stage_progress_is_monotonic_and_complete() {
        let stages = [
            Stage::Mission,
            Stage::Aircraft,
            Stage::Scenery,
            Stage::Graphics,
            Stage::Ready,
        ];
        assert_eq!(Stage::Selection.percent(), 0);
        assert_eq!(stages[4].percent(), 100);
        assert!(
            stages
                .windows(2)
                .all(|w| w[0].estimate(0.) < w[1].estimate(0.))
        );
    }
    #[test]
    fn estimated_progress_uses_weights_without_claiming_completion() {
        assert_eq!(WEIGHTS.iter().sum::<u32>(), 3142);
        for stage in [
            Stage::Mission,
            Stage::Aircraft,
            Stage::Scenery,
            Stage::Graphics,
        ] {
            assert!(stage.estimate(1.) < stage.estimate(10.));
            assert!(stage.estimate(1000.) < 100.);
        }
        assert_eq!(Stage::Ready.estimate(0.), 100.);
    }
    #[test]
    fn loading_photo_covers_wide_and_tall_windows() {
        prepare_photos();
        let mut pixels = Vec::new();
        for size in [[1920, 1080], [800, 1200]] {
            let drawn = draw_window(
                &mut pixels,
                size,
                0,
                Stage::Scenery,
                1.,
                ["Host", "Players"],
            );
            assert!(
                (drawn[0] as f64 / drawn[1] as f64 - size[0] as f64 / size[1] as f64).abs() < 0.003
            );
            assert!(pixels.chunks_exact(4).all(|p| p[3] == 255));
            assert!(pixels.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]));
        }
    }
    #[test]
    fn the_cached_background_draws_the_same_pixels_as_every_frame_did() {
        prepare_photos();
        let details = ["Host game", "Goose, Maverick"];
        for (size, photo) in [([640, 480], 0), ([853, 480], 1), ([640, 853], 0)] {
            // Several frames on one cache: the bar and the clock move, the
            // background underneath is the one composed on the first.
            for (stage, elapsed) in [
                (Stage::Mission, 0.),
                (Stage::Aircraft, 1.25),
                (Stage::Scenery, 3.4),
                (Stage::Connecting, 7.),
                (Stage::Ready, 9.),
            ] {
                let mut cached = vec![0; size[0] as usize * size[1] as usize * 4];
                let mut reference = cached.clone();
                draw_sized(&mut cached, size, photo, stage, elapsed, details);
                draw_sized_reference(&mut reference, size, photo, stage, elapsed, details);
                assert!(
                    cached == reference,
                    "{size:?} photo {photo} {stage:?} {elapsed}"
                );
            }
        }
    }
    #[test]
    fn the_cached_background_follows_the_window_size_and_the_photograph() {
        prepare_photos();
        let mut pixels = Vec::new();
        let a = draw_window(&mut pixels, [640, 480], 0, Stage::Mission, 0., ["", ""]);
        let first = pixels.clone();
        let b = draw_window(&mut pixels, [1920, 1080], 0, Stage::Mission, 0., ["", ""]);
        assert_ne!(a, b);
        assert_eq!(pixels.len(), b[0] as usize * b[1] as usize * 4);
        draw_window(&mut pixels, [640, 480], 1, Stage::Mission, 0., ["", ""]);
        assert_ne!(pixels, first, "the other photograph");
        draw_window(&mut pixels, [640, 480], 0, Stage::Mission, 0., ["", ""]);
        assert_eq!(pixels, first, "back to the first, as a fresh drawing");
    }
    #[test]
    fn the_bar_only_moves_forward_between_cached_frames() {
        prepare_photos();
        let mut pixels = Vec::new();
        let mut last = 0;
        for tenth in 0..40 {
            draw_window(
                &mut pixels,
                [640, 480],
                0,
                Stage::Scenery,
                f64::from(tenth) / 10.,
                ["", ""],
            );
            let lit = (80..560)
                .take_while(|&x| pixels[(378 * 640 + x) * 4..][..3] == [180, 200, 220])
                .count();
            assert!(lit >= last, "the bar went back at {tenth}");
            last = lit;
        }
        assert!(last > 0);
    }
    #[test]
    fn no_seconds_counter_is_drawn_under_the_bar() {
        prepare_photos();
        let (mut early, mut later) = (Vec::new(), Vec::new());
        // The stages without a percentage show the label and a bar only.
        draw_window(&mut early, [640, 480], 0, Stage::Connecting, 0., ["", ""]);
        draw_window(&mut later, [640, 480], 0, Stage::Connecting, 42., ["", ""]);
        assert!(early == later, "elapsed time must not change the frame");
        // Elsewhere only the percentage sits there, and rows of text do not
        // depend on the whole seconds.
        draw_window(&mut early, [640, 480], 0, Stage::Scenery, 5.01, ["", ""]);
        draw_window(&mut later, [640, 480], 0, Stage::Scenery, 5.02, ["", ""]);
        let rows = 405 * 640 * 4..430 * 640 * 4;
        assert_eq!(early[rows.clone()], later[rows]);
    }
    #[test]
    fn a_missing_photo_is_a_plain_dark_background() {
        let mut pixels = vec![7; 2 * 2 * 4];
        blit_photo(&mut pixels, 2, 2, None);
        assert!(pixels.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }
    #[test]
    fn a_bad_jpeg_is_rejected() {
        assert!(decode_jpeg(b"not a jpeg").is_err());
    }
    #[test]
    fn flight_loader_decodes_the_bundled_jpegs() {
        std::thread::Builder::new()
            .name("flight-loader".into())
            .spawn(prepare_photos)
            .unwrap()
            .join()
            .unwrap();
        let photos = cached_photos().expect("decoded photographs");
        eprintln!("loading jpeg decode {:.3} ms", photos.decode_ms);
        for frame in &photos.frames {
            let frame = frame.as_ref().expect("bundled jpeg");
            assert_eq!((frame.width, frame.height), (2560, 1707));
            assert_eq!(frame.pixels.len(), 2560 * 1707 * 3);
        }
    }
    #[test]
    fn pending_work_never_waits_for_worker() {
        let (send, receive) = mpsc::channel();
        let job = Job::spawn(move |_| {
            receive.recv().unwrap();
            Ok(42)
        })
        .unwrap();
        for _ in 0..1000 {
            assert!(job.poll().is_none());
        }
        send.send(()).unwrap();
    }
    #[test]
    fn loading_patch_has_valid_dimensions() {
        let logo = include_bytes!("../assets/loading/logo.rgba").as_slice();
        let (w, h) = dimensions(logo);
        assert_eq!(logo.len(), 8 + w * h * 4);
    }
    #[test]
    fn loading_snapshot_states_keep_the_battery_size() {
        assert_eq!(snapshot_window("loading"), Some((0, [640, 480])));
        assert_eq!(
            snapshot_window("loading-thunderbirds"),
            Some((1, [640, 480]))
        );
        assert_eq!(
            snapshot_window("loading-1920x1080"),
            Some((0, [1920, 1080]))
        );
        assert_eq!(
            snapshot_window("loading-thunderbirds-1920x1080"),
            Some((1, [1920, 1080]))
        );
        assert_eq!(snapshot_window("normal"), None);
    }
}

#[cfg(test)]
mod profile_probe {
    use super::*;
    /// Explicit local-only measurement. The path must be a private profile copy.
    /// `TORE_LOADING_THEATER` selects the theater (default `UKR`).
    /// `TORE_LOADING_REPEAT=1` times a second build in the same process.
    #[test]
    #[ignore = "requires TORE_LOADING_PROFILE pointing to a private imported profile"]
    fn real_mission_worker_services_headless_heartbeat() {
        let path = std::env::var_os("TORE_LOADING_PROFILE").expect("private profile path");
        let theater = std::env::var("TORE_LOADING_THEATER").unwrap_or_else(|_| "UKR".to_string());
        eprintln!("loading theater: {theater}");
        let assets = crate::assets::Assets::load(std::path::Path::new(&path)).unwrap();
        let resources = assets.theater_resources;
        let player =
            crate::aircraft::Airframe::load(&resources, tore_formats::aircraft::AircraftId::F18)
                .unwrap();
        let spec =
            crate::mission::MissionSpec::new(&theater, tore_formats::aircraft::AircraftId::F18);
        // A second build shows the process-wide city-fill layout cache. The
        // copy is outside the heartbeat timer.
        let resources_repeat = std::env::var_os("TORE_LOADING_REPEAT")
            .is_some()
            .then(|| resources.clone());
        let job =
            Job::spawn(move |progress| prepare_mission(&spec, &resources, player.kind, &progress))
                .unwrap();
        let started = Instant::now();
        let mut last = started;
        let mut max_gap = std::time::Duration::ZERO;
        let mut polls = 0;
        loop {
            let now = Instant::now();
            max_gap = max_gap.max(now - last);
            last = now;
            polls += 1;
            if let Some(result) = job.poll() {
                result.unwrap();
                break;
            }
            assert!(started.elapsed().as_secs() < 120, "loader did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        eprintln!(
            "loading probe: total_ms={:.3} heartbeat_max_ms={:.3} polls={polls}",
            started.elapsed().as_secs_f64() * 1000.,
            max_gap.as_secs_f64() * 1000.
        );
        assert!(polls > 1);
        assert!(max_gap < std::time::Duration::from_millis(250));
        if let Some(resources) = resources_repeat {
            let spec =
                crate::mission::MissionSpec::new(&theater, tore_formats::aircraft::AircraftId::F18);
            let player = crate::aircraft::Airframe::load(
                &resources,
                tore_formats::aircraft::AircraftId::F18,
            )
            .unwrap();
            let again = Instant::now();
            let job = Job::spawn(move |progress| {
                prepare_mission(&spec, &resources, player.kind, &progress)
            })
            .unwrap();
            loop {
                if let Some(result) = job.poll() {
                    result.unwrap();
                    break;
                }
                assert!(
                    again.elapsed().as_secs() < 120,
                    "cached loader did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            eprintln!(
                "loading cache repeat: total_ms={:.3}",
                again.elapsed().as_secs_f64() * 1000.
            );
        }
    }
}

/// The app lends its world to a restart worker only while the loading screen
/// owns event handling. No simulation step can run until it is returned.
pub(crate) struct WorldSlot(pub Option<crate::world::World>);
impl std::ops::Deref for WorldSlot {
    type Target = crate::world::World;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("world is in the loader")
    }
}
impl std::ops::DerefMut for WorldSlot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("world is in the loader")
    }
}

/// Reports remaining synchronous stalls without touching simulation clocks.
pub(crate) struct Timing(&'static str, Instant);
pub(crate) fn timing(name: &'static str) -> Timing {
    Timing(name, Instant::now())
}
impl Drop for Timing {
    fn drop(&mut self) {
        measured(self.0, self.1.elapsed());
    }
}

pub(crate) fn measured(name: &str, elapsed: std::time::Duration) {
    log::info!(
        "loading timing: {name} {:.3} ms",
        elapsed.as_secs_f64() * 1000.
    );
    #[cfg(test)]
    eprintln!(
        "loading timing: {name} {:.3} ms",
        elapsed.as_secs_f64() * 1000.
    );
}

/// Command-line view options restored only after the creator's worker completes.
pub(crate) type CreatorView = (u8, crate::flight_views::Reference, [f32; 2], f32);

/// The existing creator smoke check, performed before the first flight frame.
/// It uses the same restart as a player and leaves the restarted world ready.
pub(crate) fn verify_restart(
    world: &mut crate::world::World,
    aircraft: &tore_world::aircraft_type::AircraftType,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<crate::world::Restarted, String> {
    let own = &world.cockpits[crate::OWN];
    let initial = (
        own.flight.position,
        own.flight.yaw,
        own.flight.speed,
        own.flight.gear,
        own.flight.fuel,
        own.flight.payload_lbs,
        own.airport_service.selected(),
    );
    let targets: Vec<_> = world
        .combat
        .state
        .targets
        .iter()
        .map(|t| (t.id, t.position, t.hp))
        .collect();
    let restart = world
        .restart(aircraft, resources)
        .map_err(|e| e.to_string())?;
    let own = &world.cockpits[crate::OWN];
    let restarted = (
        own.flight.position,
        own.flight.yaw,
        own.flight.speed,
        own.flight.gear,
        own.flight.fuel,
        own.flight.payload_lbs,
        own.airport_service.selected(),
    );
    let restarted_targets: Vec<_> = world
        .combat
        .state
        .targets
        .iter()
        .map(|t| (t.id, t.position, t.hp))
        .collect();
    if initial != restarted || targets != restarted_targets {
        return Err("Quick Mission restart did not restore the accepted start".into());
    }
    println!("Quick Mission restart: PASS");
    Ok(restart)
}
