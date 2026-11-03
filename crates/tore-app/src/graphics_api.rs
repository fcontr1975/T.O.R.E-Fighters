//! Which graphics API the game asks wgpu for. On Windows the player chooses
//! between Vulkan and DirectX 12, and the game never changes that by itself.
//! Requested by John on 2026-11-02 after issue #8; docs/spec/graphics-options.md
//! holds the contract. Other platforms have no choice and use wgpu's own.
//!
//! A player whose game dies during GPU setup never reaches a screen with the
//! choice, so a launch that finds a dead marker asks first (`crate::graphics_prompt`).
//! The marker is one file per process under `<data>/gpu-setup/`, holding an OS file
//! lock for as long as the process lives. A later launch that can take the lock
//! knows its owner is gone, so a sibling process still in its own setup is never
//! mistaken for a dead one, and a kill or crash needs no cleanup to count. It
//! records the API that was running, and the prompt draws on the other one.
use crate::graphics::{GraphicsApi, Options};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tore_diagnostics_native::crash_context::MARKER_ARMED;

/// The backend set handed to `wgpu::Instance::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Every backend, the first adapter wgpu chooses. Only outside Windows.
    Automatic,
    Vulkan,
    Dx12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Other,
}

/// The API when the flag or saved value settles it, with no driver involved. None
/// means nothing is settled, so the default rule has to look for a Vulkan adapter.
pub(crate) fn choice_without_probe(
    platform: Platform,
    flag: Option<GraphicsApi>,
    saved: GraphicsApi,
) -> Option<Choice> {
    if platform == Platform::Other {
        return Some(Choice::Automatic);
    }
    match flag.unwrap_or(saved) {
        GraphicsApi::Vulkan => Some(Choice::Vulkan),
        GraphicsApi::Dx12 => Some(Choice::Dx12),
        GraphicsApi::Auto => None,
    }
}

/// The API for this process. A flag beats the saved value, and with neither (or
/// `--graphics-api auto`) it is Vulkan when a hardware Vulkan adapter exists,
/// otherwise DirectX 12. `vulkan_available` runs only when that default is needed.
pub(crate) fn resolve(
    platform: Platform,
    flag: Option<GraphicsApi>,
    saved: GraphicsApi,
    vulkan_available: impl FnOnce() -> bool,
) -> Choice {
    choice_without_probe(platform, flag, saved).unwrap_or_else(|| {
        if vulkan_available() {
            Choice::Vulkan
        } else {
            Choice::Dx12
        }
    })
}

/// Whether this launch writes a marker around GPU setup: only a player's launch.
pub(crate) fn arms(platform: Platform, attended: bool) -> bool {
    platform == Platform::Windows && attended
}

/// Whether this launch asks the player first. A flag naming an API has already
/// answered; `--graphics-api auto` has not.
pub(crate) fn prompts(
    platform: Platform,
    attended: bool,
    flag: Option<GraphicsApi>,
    dead: DeadMarkers,
) -> bool {
    arms(platform, attended) && flag.is_none_or(|flag| flag == GraphicsApi::Auto) && dead.any
}

/// What the prompt shows and what it draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PromptPlan {
    /// The option highlighted: the saved value, or with nothing saved the API that
    /// did not die, so that Continue tries something different.
    pub(crate) highlight: GraphicsApi,
    /// The prompt's own canvas always draws on the API that did not die. If it dies
    /// too, its marker names this API, so the next prompt uses the other one, and it
    /// can never get stuck retrying one.
    pub(crate) canvas: GraphicsApi,
}

pub(crate) fn plan_prompt(saved: GraphicsApi, dead: DeadMarkers) -> PromptPlan {
    // A marker that names no API is from something else writing it by hand, or was
    // cut short. The probe and the default rule both start on Vulkan.
    let assumed = match saved {
        GraphicsApi::Auto => GraphicsApi::Vulkan,
        saved => saved,
    };
    let canvas = other(dead.died.unwrap_or(assumed));
    PromptPlan {
        highlight: match saved {
            GraphicsApi::Auto => canvas,
            saved => saved,
        },
        canvas,
    }
}

fn other(api: GraphicsApi) -> GraphicsApi {
    match api {
        GraphicsApi::Dx12 => GraphicsApi::Vulkan,
        _ => GraphicsApi::Dx12,
    }
}

/// What a launch does before anything can touch a driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) arm: bool,
    pub(crate) prompt: Option<PromptPlan>,
    /// Whether the first use has to look for a Vulkan adapter. Never after a death:
    /// the probe may be what died, and a launch that died in it again would alternate
    /// with this one and never show the prompt.
    pub(crate) probes: bool,
}

pub(crate) fn plan_launch(
    platform: Platform,
    attended: bool,
    flag: Option<GraphicsApi>,
    saved: GraphicsApi,
    dead: DeadMarkers,
) -> Launch {
    let prompt = prompts(platform, attended, flag, dead).then(|| plan_prompt(saved, dead));
    Launch {
        arm: arms(platform, attended),
        prompt,
        probes: prompt.is_none() && choice_without_probe(platform, flag, saved).is_none(),
    }
}

/// What markers left by processes that are gone say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeadMarkers {
    pub(crate) any: bool,
    /// The API the most recent of them was running, if it says.
    pub(crate) died: Option<GraphicsApi>,
}

const MARKER_DIRECTORY: &str = "gpu-setup";
const MARKER_SUFFIX: &str = ".marker";
/// More dead markers than this are left for a later launch: the scan stays
/// bounded however the directory came to be full.
const MAX_MARKERS_SCANNED: usize = 64;

/// What a marker says the process was setting up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Looking for a Vulkan adapter, for the default rule.
    Probe,
    Canvas,
    Game,
}
impl Screen {
    fn key(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Canvas => "canvas",
            Self::Game => "game",
        }
    }
}

fn marker_text(screen: Screen, api: GraphicsApi) -> String {
    format!(
        "tore-gpu-setup 1\nscreen {}\napi {}\n",
        screen.key(),
        api.key()
    )
}

/// The API a marker's text names, if it names one the player could pick.
fn marker_api(text: &str) -> Option<GraphicsApi> {
    text.lines()
        .find_map(|line| line.strip_prefix("api "))
        .and_then(GraphicsApi::parse)
        .filter(|api| GraphicsApi::CHOICES.contains(api))
}

/// Reads and removes the markers of processes that are gone. A marker another live
/// process holds is skipped: its lock is the proof it is alive. So is one that cannot
/// be opened or locked, since that proves nothing about its owner. One that opens and
/// locks but cannot be read still means something died.
pub(crate) fn collect_dead_markers(data: &Path) -> DeadMarkers {
    let mut dead = DeadMarkers::default();
    let mut newest = None;
    let Ok(entries) = fs::read_dir(data.join(MARKER_DIRECTORY)) else {
        return dead;
    };
    for entry in entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(MARKER_SUFFIX))
        .take(MAX_MARKERS_SCANNED)
    {
        let path = entry.path();
        let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::warn!("Could not open {}: {error}", path.display());
                }
                continue;
            }
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(error)) => {
                log::warn!("Could not lock {}: {error}", path.display());
                continue;
            }
        }
        let mut text = String::new();
        let _ = file.read_to_string(&mut text);
        let modified = file.metadata().and_then(|meta| meta.modified()).ok();
        dead.any = true;
        if let Some(api) = marker_api(&text)
            && (dead.died.is_none() || modified >= newest)
        {
            dead.died = Some(api);
            newest = modified;
        }
        log::info!(
            "Graphics API: {} was left by a launch that did not finish GPU setup",
            path.display()
        );
        drop(file);
        if let Err(error) = fs::remove_file(&path) {
            log::warn!(
                "Could not remove {}, so the next launch may ask again: {error}",
                path.display()
            );
        }
    }
    dead
}

/// Takes the marker's lock, which is the whole point of the file.
fn lock_marker(file: &File) -> io::Result<()> {
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
        TryLockError::Error(error) => error,
    })
}

/// A marker this process holds; dropping the file releases the lock.
pub(crate) struct Marker {
    file: File,
    path: PathBuf,
    screen: Screen,
}

impl Marker {
    pub(crate) fn create(data: &Path, screen: Screen, api: GraphicsApi) -> io::Result<Self> {
        Self::create_with(data, screen, api, lock_marker)
    }
    /// A file left without its lock would read as a dead process next launch, so any
    /// failure after creating it removes it again.
    fn create_with(
        data: &Path,
        screen: Screen,
        api: GraphicsApi,
        lock: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        let directory = data.join(MARKER_DIRECTORY);
        fs::create_dir_all(&directory)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = directory.join(format!("{}-{stamp}{MARKER_SUFFIX}", std::process::id()));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let ready = lock(&file).and_then(|()| file.write_all(marker_text(screen, api).as_bytes()));
        if let Err(error) = ready {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(Self { file, path, screen })
    }
    fn write(&mut self, screen: Screen, api: GraphicsApi) -> io::Result<()> {
        let text = marker_text(screen, api);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(text.len() as u64)?;
        self.file.write_all(text.as_bytes())
    }
    pub(crate) fn remove(self) {
        let Self { file, path, .. } = self;
        drop(file);
        if let Err(error) = fs::remove_file(&path) {
            log::warn!(
                "Could not remove {}, so the next launch may ask again: {error}",
                path.display()
            );
        }
    }
}

struct Runtime {
    data: PathBuf,
    flag: Option<GraphicsApi>,
    /// Whether a choice made on screen is written to the graphics options file.
    /// False for the runs that ignore saved choices.
    saves: bool,
    saved: Mutex<GraphicsApi>,
    /// Resolved on first use, so a run with no window never touches a driver.
    current: Mutex<Option<Choice>>,
    /// Whether this launch writes markers. Cleared if the file system has no locks.
    armed: AtomicBool,
    marker: Mutex<Option<Marker>>,
    /// Set when this launch should ask the player before anything else.
    prompt: Mutex<Option<PromptPlan>>,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    }
}

/// Records the launch's flag and saved choice, before the first window, so that
/// the first-run screen and the game agree. A later session in the same process
/// (a re-import) picks up a value saved since, since it builds a new renderer too.
/// `attended` says a player is there to answer: not a headless, capture, validation,
/// diagnostic, scripted or dialog-free run. Those neither write nor read markers.
pub(crate) fn select(
    data: &Path,
    flag: Option<GraphicsApi>,
    saved: GraphicsApi,
    saves: bool,
    attended: bool,
) {
    if let Some(runtime) = RUNTIME.get() {
        if runtime.flag.is_none()
            && saved != GraphicsApi::Auto
            && let (Ok(mut known), Ok(mut current)) = (runtime.saved.lock(), runtime.current.lock())
        {
            *known = saved;
            *current = None;
        }
        return;
    }
    let dead = if arms(platform(), attended) {
        collect_dead_markers(data)
    } else {
        DeadMarkers::default()
    };
    let launch = plan_launch(platform(), attended, flag, saved, dead);
    if let Some(prompt) = launch.prompt {
        log::warn!(
            "Graphics API: the last launch did not finish graphics setup (on {:?}); asking first, drawing on {}",
            dead.died,
            prompt.canvas.key()
        );
    }
    let _ = RUNTIME.set(Runtime {
        data: data.to_owned(),
        flag,
        saves,
        saved: Mutex::new(saved),
        // A prompt settles the API before anything can look for an adapter.
        current: Mutex::new(launch.prompt.map(|prompt| choice_of(prompt.canvas))),
        armed: AtomicBool::new(launch.arm),
        marker: Mutex::new(None),
        prompt: Mutex::new(launch.prompt),
    });
}

/// Whether these adapter types make Vulkan the default. A software Vulkan driver
/// does not: it would win over a hardware DirectX 12 adapter and draw on the CPU.
pub(crate) fn hardware_adapter(types: impl IntoIterator<Item = wgpu::DeviceType>) -> bool {
    types
        .into_iter()
        .any(|device_type| device_type != wgpu::DeviceType::Cpu)
}

/// Whether a hardware Vulkan adapter exists. Creating the instance and listing
/// adapters is what wgpu does for any launch, and it does not create a device.
fn vulkan_available() -> bool {
    let _step = crate::gpu_setup::begin("adapter enumeration", "Vulkan");
    #[allow(clippy::disallowed_methods)]
    let instance = wgpu::Instance::new(&descriptor(Choice::Vulkan));
    hardware_adapter(
        instance
            .enumerate_adapters(wgpu::Backends::VULKAN)
            .iter()
            .map(|adapter| adapter.get_info().device_type),
    )
}

fn current() -> Choice {
    let Some(runtime) = RUNTIME.get() else {
        // The offline tools that return before arguments are parsed.
        return resolve(platform(), None, GraphicsApi::Auto, vulkan_available);
    };
    let Ok(mut current) = runtime.current.lock() else {
        return Choice::Automatic;
    };
    if let Some(choice) = *current {
        return choice;
    }
    let saved = runtime
        .saved
        .lock()
        .map_or(GraphicsApi::Auto, |saved| *saved);
    let choice = choice_without_probe(platform(), runtime.flag, saved).unwrap_or_else(|| {
        // The probe can be what kills a broken driver, so it is setup like the rest.
        let armed_here = probe_starting(runtime);
        let choice = resolve(platform(), runtime.flag, saved, vulkan_available);
        probe_ended(runtime, armed_here, api_of(choice));
        choice
    });
    log::info!(
        "Graphics API: {choice:?} (flag={:?} saved={})",
        runtime.flag.map(GraphicsApi::key),
        saved.key()
    );
    *current = Some(choice);
    choice
}

/// Before a probe: writes a marker saying so, or changes the one already held. True
/// when this made a new one.
fn probe_starting(runtime: &Runtime) -> bool {
    if let Ok(mut marker) = runtime.marker.lock()
        && let Some(marker) = marker.as_mut()
    {
        if let Err(error) = marker.write(Screen::Probe, GraphicsApi::Vulkan) {
            log::warn!("Graphics API: could not update the GPU setup marker: {error}");
        }
        return false;
    }
    arm(runtime, Screen::Probe, GraphicsApi::Vulkan)
}

/// After a probe that returned: the API is known now. A marker made only for the
/// probe goes; one that was already held names the API that will be used.
fn probe_ended(runtime: &Runtime, armed_here: bool, api: GraphicsApi) {
    if armed_here {
        setup_ended();
    } else if let Ok(mut marker) = runtime.marker.lock()
        && let Some(marker) = marker.as_mut()
    {
        let screen = marker.screen;
        if let Err(error) = marker.write(screen, api) {
            log::warn!("Graphics API: could not update the GPU setup marker: {error}");
        }
    }
}

/// wgpu compiles DirectX 12 shaders with FXC (`d3dcompiler_47.dll`, part of
/// Windows, nothing to ship), again on every launch, on the main thread.
/// `InstanceFlags::DEBUG` is wgpu's only way to ask it to skip optimization.
/// Without it one shader took over 230 s on a Windows VM, with the window frozen;
/// with it, 24 to 38 s. Figures: docs/spec/graphics-options.md.
pub(crate) fn descriptor(choice: Choice) -> wgpu::InstanceDescriptor {
    match choice {
        Choice::Automatic => wgpu::InstanceDescriptor::default(),
        Choice::Vulkan => wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        },
        Choice::Dx12 => wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::from_build_config() | wgpu::InstanceFlags::DEBUG,
            ..Default::default()
        },
    }
}

/// The instance every player-reachable graphics path uses. Each call reads the
/// choice in effect now, which is how a choice on the import screen reaches the
/// game renderer that follows it.
pub(crate) fn instance() -> wgpu::Instance {
    let choice = current();
    let _step = crate::gpu_setup::begin("instance", "graphics API choice");
    #[allow(clippy::disallowed_methods)]
    wgpu::Instance::new(&descriptor(choice))
}

/// True when this launch uses DirectX 12, whose shader compile is slow.
pub(crate) fn dx12_chosen() -> bool {
    current() == Choice::Dx12
}

fn choice_of(api: GraphicsApi) -> Choice {
    match api {
        GraphicsApi::Vulkan => Choice::Vulkan,
        GraphicsApi::Dx12 => Choice::Dx12,
        GraphicsApi::Auto => Choice::Automatic,
    }
}

fn api_of(choice: Choice) -> GraphicsApi {
    match choice {
        Choice::Automatic => GraphicsApi::Auto,
        Choice::Vulkan => GraphicsApi::Vulkan,
        Choice::Dx12 => GraphicsApi::Dx12,
    }
}

/// What the import screen offers: the API in effect, or nothing off Windows.
pub(crate) fn screen_choice() -> Option<GraphicsApi> {
    (platform() == Platform::Windows).then(|| api_of(current()))
}

/// The player chose on a screen that comes before the game renderer: it applies
/// to the renderer built next, in this process, and it is saved for later ones.
pub(crate) fn choose(api: GraphicsApi) {
    let Some(runtime) = RUNTIME.get() else { return };
    if let Ok(mut current) = runtime.current.lock() {
        *current = Some(resolve(platform(), Some(api), api, || true));
    }
    log::info!("Graphics API: the player chose {}", api.key());
    if runtime.saves {
        let path = runtime.data.join("graphics-v1.conf");
        let options = Options {
            graphics_api: api,
            ..Options::load(&path)
        };
        if let Err(error) = options.save(&path) {
            log::warn!("Could not save {}: {error}", path.display());
        }
    }
}

/// Writes a marker, if this launch has them and holds none. True when it did.
fn arm(runtime: &Runtime, screen: Screen, api: GraphicsApi) -> bool {
    if !runtime.armed.load(Ordering::Relaxed) {
        return false;
    }
    let Ok(mut marker) = runtime.marker.lock() else {
        return false;
    };
    if marker.is_some() {
        return false;
    }
    match Marker::create(&runtime.data, screen, api) {
        Ok(created) => {
            *marker = Some(created);
            MARKER_ARMED.store(true, Ordering::Release);
            true
        }
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            // Without locks a live sibling would look dead, so no marker at all.
            runtime.armed.store(false, Ordering::Relaxed);
            log::warn!(
                "Graphics API: this file system has no file locks, so this launch writes no GPU setup marker: {error}"
            );
            false
        }
        Err(error) => {
            log::warn!(
                "Graphics API: could not write the GPU setup marker, so a failure here will not be noticed next launch: {error}"
            );
            false
        }
    }
}

/// Writes a marker, if this launch has them, before GPU setup can die. It names the
/// API this setup is about to use. Nothing has probed if that is still unknown, and
/// a probe starts on Vulkan and corrects the marker when it returns.
pub(crate) fn setup_starting(screen: Screen) {
    let Some(runtime) = RUNTIME.get() else { return };
    let known = runtime.current.lock().ok().and_then(|current| *current);
    let saved = runtime
        .saved
        .lock()
        .map_or(GraphicsApi::Auto, |saved| *saved);
    let api = match known.map(api_of).unwrap_or(runtime.flag.unwrap_or(saved)) {
        GraphicsApi::Auto => GraphicsApi::Vulkan,
        api => api,
    };
    arm(runtime, screen, api);
}

/// The setup window ended well, or the process is closing cleanly. A returned
/// error and a panic do not come here: their marker stays, so the next launch
/// offers the choice, and the cost of being wrong is one question.
pub(crate) fn setup_ended() {
    let Some(runtime) = RUNTIME.get() else { return };
    let Ok(mut marker) = runtime.marker.lock() else {
        return;
    };
    if let Some(marker) = marker.take() {
        MARKER_ARMED.store(false, Ordering::Release);
        marker.remove();
    }
}

/// If this launch should ask the player, the option to highlight. The API the prompt
/// draws on was settled in [`select`].
pub(crate) fn begin_prompt() -> Option<GraphicsApi> {
    let plan = RUNTIME.get()?.prompt.lock().ok()?.take()?;
    Some(plan.highlight)
}

/// What the Graphics screen shows for a draft value: the saved choice, or the API
/// in use when nothing is saved yet. Tests have no launch and see Vulkan.
pub(crate) fn shown(draft: GraphicsApi) -> GraphicsApi {
    match draft {
        GraphicsApi::Auto => running(),
        chosen => chosen,
    }
}

fn running() -> GraphicsApi {
    RUNTIME
        .get()
        .and_then(|runtime| *runtime.current.lock().ok()?)
        .map_or(GraphicsApi::Vulkan, api_of)
}

/// What the Graphics screen adds when this run differs from the row, because the
/// row applies only after a restart.
pub(crate) fn status_note(draft: GraphicsApi) -> Option<String> {
    let (running, shown) = (running(), shown(draft));
    (running != shown).then(|| {
        format!(
            "This run uses {}. Restart the game to use {}.",
            running.label(),
            shown.label()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use GraphicsApi::{Auto, Dx12, Vulkan};

    fn on_windows(flag: Option<GraphicsApi>, saved: GraphicsApi, vulkan: bool) -> Choice {
        resolve(Platform::Windows, flag, saved, || vulkan)
    }

    #[test]
    fn nothing_saved_means_vulkan_when_it_exists_and_dx12_otherwise() {
        assert_eq!(on_windows(None, Auto, true), Choice::Vulkan);
        assert_eq!(on_windows(None, Auto, false), Choice::Dx12);
    }

    #[test]
    fn software_vulkan_alone_does_not_count_as_having_vulkan() {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};
        assert!(!hardware_adapter([]));
        assert!(!hardware_adapter([Cpu]));
        assert!(!hardware_adapter([Cpu, Cpu]));
        for hardware in [DiscreteGpu, IntegratedGpu, VirtualGpu, Other] {
            assert!(hardware_adapter([hardware]), "{hardware:?}");
            assert!(hardware_adapter([Cpu, hardware]), "{hardware:?}");
        }
    }

    #[test]
    fn a_saved_value_is_obeyed_without_looking_for_an_adapter() {
        let never = || -> bool { panic!("the default was not needed") };
        assert_eq!(
            resolve(Platform::Windows, None, Vulkan, never),
            Choice::Vulkan
        );
        assert_eq!(resolve(Platform::Windows, None, Dx12, never), Choice::Dx12);
    }

    #[test]
    fn a_flag_beats_the_saved_value() {
        assert_eq!(on_windows(Some(Dx12), Vulkan, true), Choice::Dx12);
        assert_eq!(on_windows(Some(Vulkan), Dx12, false), Choice::Vulkan);
        // Auto on the command line asks for the default, whatever is saved.
        assert_eq!(on_windows(Some(Auto), Dx12, true), Choice::Vulkan);
        assert_eq!(on_windows(Some(Auto), Vulkan, false), Choice::Dx12);
    }

    #[test]
    fn other_platforms_stay_on_wgpus_own_choice() {
        for flag in [None, Some(Auto), Some(Vulkan), Some(Dx12)] {
            for saved in [Auto, Vulkan, Dx12] {
                let choice = resolve(Platform::Other, flag, saved, || {
                    panic!("no adapter lookup off Windows")
                });
                assert_eq!(choice, Choice::Automatic);
            }
        }
    }

    fn profile(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tore-graphics-api-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    const NONE: DeadMarkers = DeadMarkers {
        any: false,
        died: None,
    };
    const DIED_VULKAN: DeadMarkers = DeadMarkers {
        any: true,
        died: Some(Vulkan),
    };
    const DIED_DX12: DeadMarkers = DeadMarkers {
        any: true,
        died: Some(Dx12),
    };
    const DIED_UNKNOWN: DeadMarkers = DeadMarkers {
        any: true,
        died: None,
    };

    #[test]
    fn a_dead_marker_prompts_only_a_player_whose_flag_has_not_answered() {
        assert!(prompts(Platform::Windows, true, None, DIED_VULKAN));
        assert!(prompts(Platform::Windows, true, Some(Auto), DIED_VULKAN));
        assert!(!prompts(Platform::Windows, true, None, NONE));
        assert!(!prompts(Platform::Windows, false, None, DIED_VULKAN));
        assert!(!prompts(Platform::Other, true, None, DIED_VULKAN));
        for flag in [Vulkan, Dx12] {
            assert!(!prompts(Platform::Windows, true, Some(flag), DIED_VULKAN));
        }
    }

    #[test]
    fn only_a_players_windows_launch_writes_a_marker() {
        assert!(arms(Platform::Windows, true));
        assert!(!arms(Platform::Windows, false));
        assert!(!arms(Platform::Other, true));
        assert!(!arms(Platform::Other, false));
    }

    #[test]
    fn a_dead_marker_means_no_probe() {
        for dead in [DIED_VULKAN, DIED_DX12, DIED_UNKNOWN] {
            for flag in [None, Some(Auto)] {
                let launch = plan_launch(Platform::Windows, true, flag, Auto, dead);
                assert!(launch.prompt.is_some(), "{dead:?} {flag:?}");
                assert!(!launch.probes, "{dead:?} {flag:?}");
            }
        }
        // With no death, nothing saved is what the probe is for.
        let healthy = plan_launch(Platform::Windows, true, None, Auto, NONE);
        assert!(healthy.probes);
        assert_eq!(healthy.prompt, None);
        assert!(healthy.arm, "the probe itself is covered by a marker");
        // A saved value or a flag needs no probe either way.
        assert!(!plan_launch(Platform::Windows, true, None, Dx12, NONE).probes);
        assert!(!plan_launch(Platform::Windows, true, Some(Vulkan), Auto, NONE).probes);
        // A run with no player reads no markers and writes none, so it still probes.
        let unattended = plan_launch(Platform::Windows, false, None, Auto, NONE);
        assert!(unattended.probes && !unattended.arm);
        assert_eq!(unattended.prompt, None);
        let other = plan_launch(Platform::Other, true, None, Auto, NONE);
        assert!(!other.probes && !other.arm);
    }

    #[test]
    fn the_prompt_always_draws_on_the_api_that_did_not_die() {
        for saved in [Auto, Vulkan, Dx12] {
            assert_eq!(plan_prompt(saved, DIED_VULKAN).canvas, Dx12, "{saved:?}");
            assert_eq!(plan_prompt(saved, DIED_DX12).canvas, Vulkan, "{saved:?}");
        }
    }

    #[test]
    fn the_saved_value_only_sets_the_highlight() {
        assert_eq!(
            plan_prompt(Vulkan, DIED_VULKAN),
            PromptPlan {
                highlight: Vulkan,
                canvas: Dx12
            }
        );
        assert_eq!(
            plan_prompt(Dx12, DIED_VULKAN),
            PromptPlan {
                highlight: Dx12,
                canvas: Dx12
            }
        );
        // With nothing saved, Continue tries something different.
        assert_eq!(plan_prompt(Auto, DIED_VULKAN).highlight, Dx12);
        assert_eq!(plan_prompt(Auto, DIED_DX12).highlight, Vulkan);
    }

    #[test]
    fn a_marker_naming_no_api_assumes_the_saved_one_or_vulkan() {
        assert_eq!(plan_prompt(Auto, DIED_UNKNOWN).canvas, Dx12);
        assert_eq!(plan_prompt(Vulkan, DIED_UNKNOWN).canvas, Dx12);
        assert_eq!(plan_prompt(Dx12, DIED_UNKNOWN).canvas, Vulkan);
    }

    #[test]
    fn a_prompt_that_keeps_dying_never_retries_one_api() {
        // Each launch's prompt canvas dies, leaving a marker naming what it drew on.
        let mut dead = DIED_VULKAN;
        let mut drawn = Vec::new();
        for _ in 0..6 {
            let canvas = plan_launch(Platform::Windows, true, None, Vulkan, dead)
                .prompt
                .unwrap()
                .canvas;
            drawn.push(canvas);
            dead.died = Some(canvas);
        }
        assert_eq!(drawn, [Dx12, Vulkan, Dx12, Vulkan, Dx12, Vulkan]);
    }

    #[test]
    fn a_marker_says_what_was_running_and_only_the_player_choices_count() {
        assert_eq!(marker_api(&marker_text(Screen::Game, Vulkan)), Some(Vulkan));
        assert_eq!(marker_api(&marker_text(Screen::Canvas, Dx12)), Some(Dx12));
        assert_eq!(
            marker_api(&marker_text(Screen::Probe, Vulkan)),
            Some(Vulkan)
        );
        assert_eq!(marker_api(""), None);
        assert_eq!(marker_api("tore-gpu-setup 1\nscreen game\n"), None);
        assert_eq!(marker_api("api auto\n"), None);
        assert_eq!(marker_api("api metal\n"), None);
    }

    #[test]
    fn a_siblings_marker_is_left_alone_and_a_dead_ones_counts_once() {
        let data = profile("sibling");
        // Held through its own handle: another launch sees a locked file and must
        // walk past it, as it does for a sibling game still in its own setup.
        let live = Marker::create(&data, Screen::Game, Vulkan).unwrap();
        assert_eq!(collect_dead_markers(&data), NONE);
        assert!(live.path.exists());
        // The holder goes away with no clean-up, as in a crash or kill.
        let Marker { file, path, .. } = live;
        drop(file);
        assert_eq!(collect_dead_markers(&data), DIED_VULKAN);
        assert!(!path.exists(), "a counted marker is consumed");
        assert_eq!(collect_dead_markers(&data), NONE);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_clean_end_leaves_nothing_behind() {
        let data = profile("clean");
        Marker::create(&data, Screen::Game, Vulkan)
            .unwrap()
            .remove();
        assert_eq!(collect_dead_markers(&data), NONE);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_marker_is_corrected_when_the_probe_finds_no_vulkan() {
        let data = profile("corrected");
        let mut marker = Marker::create(&data, Screen::Game, Vulkan).unwrap();
        marker.write(Screen::Probe, Vulkan).unwrap();
        marker.write(Screen::Game, Dx12).unwrap();
        let Marker { file, .. } = marker;
        drop(file);
        assert_eq!(collect_dead_markers(&data), DIED_DX12);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_newest_marker_says_which_api_died() {
        let data = profile("newest");
        let directory = data.join(MARKER_DIRECTORY);
        fs::create_dir_all(&directory).unwrap();
        let write = |name: &str, api: GraphicsApi, age: u64| {
            let path = directory.join(format!("{name}{MARKER_SUFFIX}"));
            fs::write(&path, marker_text(Screen::Game, api)).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - std::time::Duration::from_secs(age))
                .unwrap();
        };
        write("a", Dx12, 100);
        write("b", Vulkan, 10);
        write("c", Dx12, 1000);
        assert_eq!(collect_dead_markers(&data), DIED_VULKAN);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_marker_with_nothing_readable_still_counts() {
        let data = profile("empty");
        let directory = data.join(MARKER_DIRECTORY);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("x{MARKER_SUFFIX}")), "").unwrap();
        assert_eq!(collect_dead_markers(&data), DIED_UNKNOWN);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_marker_that_cannot_be_locked_leaves_no_file() {
        for kind in [io::ErrorKind::Unsupported, io::ErrorKind::WouldBlock] {
            let data = profile("no-lock");
            let result =
                Marker::create_with(&data, Screen::Game, Vulkan, |_| Err(io::Error::from(kind)));
            assert_eq!(result.err().map(|error| error.kind()), Some(kind));
            let left = fs::read_dir(data.join(MARKER_DIRECTORY)).unwrap().count();
            assert_eq!(left, 0, "a stray file would read as a dead process");
            assert_eq!(collect_dead_markers(&data), NONE);
            fs::remove_dir_all(data).unwrap();
        }
    }

    #[test]
    fn profiles_do_not_see_each_others_markers() {
        let (first, second) = (profile("first"), profile("second"));
        let Marker { file, .. } = Marker::create(&first, Screen::Game, Vulkan).unwrap();
        drop(file);
        assert_eq!(collect_dead_markers(&second), NONE);
        assert_eq!(collect_dead_markers(&first), DIED_VULKAN);
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn a_leftover_state_file_from_the_test_build_changes_nothing() {
        let data = profile("leftover");
        fs::write(
            data.join("gpu-fallback-v1.conf"),
            "tore-gpu-fallback 1\nstate exhausted\nbuild 0.2.2\n",
        )
        .unwrap();
        assert_eq!(collect_dead_markers(&data), NONE);
        assert_eq!(on_windows(None, Auto, true), Choice::Vulkan);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_other_api_is_the_other_one() {
        assert_eq!(other(Vulkan), Dx12);
        assert_eq!(other(Dx12), Vulkan);
    }

    #[test]
    fn instances_ask_for_only_what_was_chosen() {
        assert_eq!(
            descriptor(Choice::Automatic).backends,
            wgpu::InstanceDescriptor::default().backends
        );
        assert_eq!(descriptor(Choice::Vulkan).backends, wgpu::Backends::VULKAN);
        assert_eq!(descriptor(Choice::Dx12).backends, wgpu::Backends::DX12);
    }

    #[test]
    fn every_dx12_instance_skips_fxc_optimization() {
        assert!(
            descriptor(Choice::Dx12)
                .flags
                .contains(wgpu::InstanceFlags::DEBUG)
        );
    }

    #[test]
    fn the_row_shows_a_saved_choice_as_it_is() {
        assert_eq!(shown(Dx12), Dx12);
        assert_eq!(shown(Vulkan), Vulkan);
    }
}
