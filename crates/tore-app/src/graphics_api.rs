//! Which graphics API the game asks wgpu for, and the automatic fallback to
//! DirectX 12 on Windows after a launch dies during GPU setup. Requested by John
//! on 2026-11-02 after issue #8; docs/spec/graphics-options.md holds the contract.
//! The states and the wording are agent decisions.
//!
//! The marker is one file per process under `<data>/gpu-setup/`, holding an OS
//! file lock for as long as the process lives. A later launch that can take the
//! lock knows its owner is gone, so a sibling process still in its own setup is
//! never mistaken for a dead one, and a kill or crash needs no cleanup to count.
use crate::graphics::GraphicsApi;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The backend set handed to `wgpu::Instance::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Every backend: the first adapter wgpu chooses, the behaviour before this
    /// setting existed.
    Automatic,
    Vulkan,
    Dx12,
}

/// The fallback state remembered between launches, in `gpu-fallback-v1.conf`.
/// It never touches the player's own setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Saved {
    #[default]
    None,
    /// A launch died on the automatic choice; DirectX 12 has not finished setup.
    Trying,
    /// DirectX 12 finished setup after that: later launches go straight to it.
    Dx12,
    /// DirectX 12 failed too. No more switching until a new build or a good run.
    Exhausted,
}

/// What dead markers from earlier launches say had been running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Evidence {
    /// A launch died on something other than DirectX 12.
    pub(crate) other: bool,
    /// A launch died on DirectX 12.
    pub(crate) dx12: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Other,
}

/// Why this launch uses what it uses, for the log and a one-time report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Nothing unusual.
    Plain,
    /// The player asked for this API.
    Explicit,
    /// The previous launch died in setup: switching to DirectX 12 now.
    FellBack,
    /// DirectX 12 has not yet finished setup, and the last try did not die.
    StillTrying,
    /// DirectX 12 is remembered from an earlier launch.
    Remembered,
    /// Everything failed, just now: this is when the report is written.
    JustExhausted,
    /// Everything failed on an earlier launch.
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) choice: Choice,
    /// Whether this launch writes a marker around GPU setup.
    pub(crate) arm: bool,
    pub(crate) saved: Saved,
    pub(crate) event: Event,
}

/// The launch decision: no files, no GPU. An automated launch is an `attended`
/// false: it neither arms a marker nor acts on what earlier launches left.
pub(crate) fn decide(
    setting: GraphicsApi,
    platform: Platform,
    attended: bool,
    evidence: Evidence,
    saved: Saved,
) -> Decision {
    let plain = |choice, arm, saved, event| Decision {
        choice,
        arm,
        saved,
        event,
    };
    if platform == Platform::Other {
        return plain(Choice::Automatic, false, Saved::None, Event::Plain);
    }
    match setting {
        GraphicsApi::Vulkan => plain(Choice::Vulkan, false, saved, Event::Explicit),
        GraphicsApi::Dx12 => plain(Choice::Dx12, false, saved, Event::Explicit),
        GraphicsApi::Auto if !attended => plain(Choice::Automatic, false, saved, Event::Plain),
        GraphicsApi::Auto => match (saved, evidence) {
            (Saved::Exhausted, _) => {
                plain(Choice::Automatic, false, Saved::Exhausted, Event::Exhausted)
            }
            (Saved::None, Evidence { other: true, .. }) => {
                plain(Choice::Dx12, true, Saved::Trying, Event::FellBack)
            }
            // The automatic choice was already DirectX 12: nothing left to try.
            // A DirectX 12 death in the remembered or trying states ends it too.
            (_, Evidence { dx12: true, .. }) => plain(
                Choice::Automatic,
                false,
                Saved::Exhausted,
                Event::JustExhausted,
            ),
            (Saved::None, _) => plain(Choice::Automatic, true, Saved::None, Event::Plain),
            (Saved::Trying, _) => plain(Choice::Dx12, true, Saved::Trying, Event::StillTrying),
            (Saved::Dx12, _) => plain(Choice::Dx12, true, Saved::Dx12, Event::Remembered),
        },
    }
}

/// The state after a game frame has been presented on `backend`. Anything but
/// DirectX 12 working means whatever failed before is fixed; DirectX 12 working
/// after a fallback is the thing to remember.
pub(crate) fn after_success(saved: Saved, fell_back: bool, is_dx12: bool) -> Saved {
    match (saved, is_dx12) {
        (_, false) => Saved::None,
        (Saved::Trying, true) if fell_back => Saved::Dx12,
        (saved, true) => saved,
    }
}

const STATE_FILE: &str = "gpu-fallback-v1.conf";
const MARKER_DIRECTORY: &str = "gpu-setup";
const MARKER_SUFFIX: &str = ".marker";
/// More dead markers than this are left for a later launch: the scan stays
/// bounded however the directory came to be full.
const MAX_MARKERS_SCANNED: usize = 64;

/// A `Saved` from file text. Anything unrecognized means no state. `Exhausted`
/// lapses with a new build, which may be the fix; a remembered DirectX 12 does
/// not, because Vulkan crashing once per update would be worse than staying put.
pub(crate) fn parse_state(text: &str, build: &str) -> Saved {
    let mut lines = text.lines();
    if lines.next() != Some("tore-gpu-fallback 1") {
        return Saved::None;
    }
    let mut state = None;
    let mut saved_build = None;
    for line in lines {
        match line.split_once(' ') {
            Some(("state", value)) => state = Some(value),
            Some(("build", value)) => saved_build = Some(value),
            _ => {}
        }
    }
    match state {
        Some("trying") => Saved::Trying,
        Some("dx12") => Saved::Dx12,
        Some("exhausted") if saved_build == Some(build) => Saved::Exhausted,
        _ => Saved::None,
    }
}

pub(crate) fn state_text(saved: Saved, build: &str) -> Option<String> {
    let state = match saved {
        Saved::None => return None,
        Saved::Trying => "trying",
        Saved::Dx12 => "dx12",
        Saved::Exhausted => "exhausted",
    };
    Some(format!(
        "tore-gpu-fallback 1\nstate {state}\nbuild {build}\n"
    ))
}

/// What one marker says was running.
fn marker_text(attempt: &str) -> String {
    format!("tore-gpu-setup 1\nbackend {attempt}\n")
}

/// Adds a dead marker's contents to the evidence. An unreadable marker still
/// means something died, so it counts as the automatic choice.
fn marker_evidence(text: &str, evidence: &mut Evidence) {
    if text.lines().any(|line| line == "backend dx12") {
        evidence.dx12 = true;
    } else {
        evidence.other = true;
    }
}

/// Reads and removes the markers of processes that are gone. A marker another
/// live process holds is skipped: its lock is the proof it is alive.
pub(crate) fn collect_dead_markers(data: &Path) -> Evidence {
    let mut evidence = Evidence::default();
    let Ok(entries) = fs::read_dir(data.join(MARKER_DIRECTORY)) else {
        return evidence;
    };
    for entry in entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(MARKER_SUFFIX))
        .take(MAX_MARKERS_SCANNED)
    {
        let path = entry.path();
        let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        if file.try_lock().is_err() {
            continue;
        }
        let mut text = String::new();
        let _ = file.read_to_string(&mut text);
        marker_evidence(&text, &mut evidence);
        log::info!(
            "Graphics API: {} was left by a launch that did not finish GPU setup",
            path.display()
        );
        drop(file);
        if let Err(error) = fs::remove_file(&path) {
            log::warn!("Could not remove {}: {error}", path.display());
        }
    }
    evidence
}

/// A marker this process holds; dropping the file releases the lock.
pub(crate) struct Marker {
    file: File,
    path: PathBuf,
}

impl Marker {
    pub(crate) fn create(data: &Path, attempt: &str) -> std::io::Result<Self> {
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
        file.try_lock().map_err(std::io::Error::other)?;
        file.write_all(marker_text(attempt).as_bytes())?;
        Ok(Self { file, path })
    }
    /// Replaces the recorded backend once the adapter is known.
    pub(crate) fn record(&mut self, attempt: &str) -> std::io::Result<()> {
        let text = marker_text(attempt);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(text.len() as u64)?;
        self.file.write_all(text.as_bytes())
    }
    pub(crate) fn remove(self) {
        let Self { file, path } = self;
        drop(file);
        if let Err(error) = fs::remove_file(&path) {
            log::warn!("Could not remove {}: {error}", path.display());
        }
    }
}

struct Runtime {
    decision: Decision,
    setting: GraphicsApi,
    data: PathBuf,
    /// Whether this launch reads and writes the remembered state.
    tracked: bool,
    saved: Mutex<Saved>,
    marker: Mutex<Option<Marker>>,
    /// What a new marker records: the choice, then the adapter's backend.
    attempt: Mutex<&'static str>,
    notice: Mutex<Option<String>>,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    }
}

fn write_state(data: &Path, saved: Saved) {
    let path = data.join(STATE_FILE);
    let result = match state_text(saved, crate::version::version()) {
        Some(text) => crate::preferences::write(&path, &text),
        None => match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    };
    if let Err(error) = result {
        log::warn!("Could not save {}: {error}", path.display());
    }
}

/// The retry instruction, in one place because the log repeats it.
fn retry_hint(data: &Path) -> String {
    format!(
        "To try Vulkan again, start the game once with --graphics-api vulkan (a good run returns to automatic), or choose Vulkan in Pref > Graphics, or delete {}",
        data.join(STATE_FILE).display()
    )
}

/// Decides this process's graphics API, once. Later sessions in the same process
/// (a re-import) keep it. Runs before the first window, so that the first-run
/// screen and the game agree. `setting` is the flag if given, else the saved
/// choice. `attended` says this is a player's launch and not an automated one.
pub(crate) fn select(data: &Path, setting: GraphicsApi, attended: bool, interactive: bool) {
    if RUNTIME.get().is_some() {
        return;
    }
    let tracked = attended && platform() == Platform::Windows;
    let evidence = if tracked && setting == GraphicsApi::Auto {
        collect_dead_markers(data)
    } else {
        Evidence::default()
    };
    let saved = if tracked {
        crate::preferences::read(&data.join(STATE_FILE)).map_or(Saved::None, |text| {
            parse_state(&text, crate::version::version())
        })
    } else {
        Saved::None
    };
    let decision = decide(setting, platform(), attended, evidence, saved);
    log::info!(
        "Graphics API: setting={} platform={:?} attended={attended} choice={:?} fallback_state={:?} event={:?} marker={}",
        setting.key(),
        platform(),
        decision.choice,
        decision.saved,
        decision.event,
        decision.arm
    );
    if setting != GraphicsApi::Auto && platform() == Platform::Other {
        log::warn!(
            "Graphics API: the saved choice {} applies only on Windows; using automatic",
            setting.key()
        );
    }
    if tracked && decision.saved != saved {
        write_state(data, decision.saved);
    }
    let mut notice = None;
    match decision.event {
        Event::FellBack => {
            log::warn!(
                "Graphics API: the previous launch died during GPU setup, so this launch uses DirectX 12. Shader compilation can take a minute or more with no window activity. See the crash report in the log directory."
            );
            notice = Some("Graphics problem last time: using DirectX 12 for this launch.".into());
        }
        Event::StillTrying => log::warn!(
            "Graphics API: DirectX 12 has not finished setup yet after the earlier failure; trying it again"
        ),
        Event::Remembered => log::info!(
            "Graphics API: using DirectX 12 because an earlier launch died during GPU setup. {}",
            retry_hint(data)
        ),
        Event::Exhausted => log::warn!(
            "Graphics API: automatic fallback is out of options for this build, so the automatic choice is used and nothing is switched. {}",
            retry_hint(data)
        ),
        Event::JustExhausted => {
            let message = "T.O.R.E-Fighters could not finish graphics setup with Vulkan or with DirectX 12. It will stop switching and use the automatic choice. Updating the graphics driver is the next thing to try. Please send this report with any bug report.";
            log::warn!("Graphics API: {message}");
            crate::diagnostics::report_fallback_exhausted(message, interactive);
        }
        Event::Plain | Event::Explicit => {}
    }
    let attempt = match decision.choice {
        Choice::Dx12 => "dx12",
        Choice::Vulkan => "vulkan",
        Choice::Automatic => "auto",
    };
    let _ = RUNTIME.set(Runtime {
        decision,
        setting,
        data: data.to_owned(),
        tracked,
        saved: Mutex::new(decision.saved),
        marker: Mutex::new(None),
        attempt: Mutex::new(attempt),
        notice: Mutex::new(notice),
    });
}

fn choice() -> Choice {
    RUNTIME
        .get()
        .map_or(Choice::Automatic, |runtime| runtime.decision.choice)
}

/// The instance every player-reachable graphics path uses. The automatic choice
/// is exactly the default descriptor, so it behaves as it did before this setting.
pub(crate) fn descriptor(choice: Choice) -> wgpu::InstanceDescriptor {
    match choice {
        Choice::Automatic => wgpu::InstanceDescriptor::default(),
        Choice::Vulkan => wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        },
        // wgpu compiles shaders with FXC here (no extra DLLs to ship). FXC's
        // optimizer needed about 9 minutes for the game's shaders on the machine
        // measured, one of them nearly 6, with the window frozen, on every launch.
        // InstanceFlags::DEBUG is wgpu's only way to pass skip-optimization, which
        // brought it under a minute; the driver still optimizes what it receives.
        // Measurements: docs/spec/graphics-options.md.
        Choice::Dx12 => wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::from_build_config() | wgpu::InstanceFlags::DEBUG,
            ..Default::default()
        },
    }
}

pub(crate) fn instance() -> wgpu::Instance {
    let choice = choice();
    let _step = crate::gpu_setup::begin("instance", "graphics API choice");
    #[allow(clippy::disallowed_methods)]
    wgpu::Instance::new(&descriptor(choice))
}

/// True when this launch asked for DirectX 12, whose first compile is slow.
pub(crate) fn dx12_chosen() -> bool {
    choice() == Choice::Dx12
}

/// Arms the marker, if this launch has one, before GPU setup can die.
pub(crate) fn setup_starting() {
    let Some(runtime) = RUNTIME.get() else { return };
    if !runtime.decision.arm {
        return;
    }
    let Ok(mut marker) = runtime.marker.lock() else {
        return;
    };
    if marker.is_some() {
        return;
    }
    let attempt = runtime.attempt.lock().map_or("auto", |attempt| *attempt);
    match Marker::create(&runtime.data, attempt) {
        Ok(created) => *marker = Some(created),
        Err(error) => log::warn!(
            "Graphics API: could not write the GPU setup marker, so automatic fallback will not see this launch: {error}"
        ),
    }
}

/// Records what the automatic choice turned out to be.
pub(crate) fn adapter_selected(backend: wgpu::Backend) {
    let Some(runtime) = RUNTIME.get() else { return };
    let attempt = match backend {
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Vulkan => "vulkan",
        _ => "other",
    };
    if let Ok(mut recorded) = runtime.attempt.lock() {
        *recorded = attempt;
    }
    let Ok(mut marker) = runtime.marker.lock() else {
        return;
    };
    if let Some(marker) = marker.as_mut()
        && let Err(error) = marker.record(attempt)
    {
        log::warn!("Graphics API: could not update the GPU setup marker: {error}");
    }
}

/// The setup window ended without dying. Removes the marker, if any.
pub(crate) fn setup_ended() {
    let Some(runtime) = RUNTIME.get() else { return };
    let Ok(mut marker) = runtime.marker.lock() else {
        return;
    };
    if let Some(marker) = marker.take() {
        marker.remove();
    }
}

/// The first game frame reached the screen: setup is over and it worked. The
/// first-run canvas does not count, because issue #8's canvas worked too.
pub(crate) fn game_frame_presented(backend: wgpu::Backend) {
    setup_ended();
    let Some(runtime) = RUNTIME.get() else { return };
    if !runtime.tracked {
        return;
    }
    if backend != wgpu::Backend::Dx12 {
        // An explicit Vulkan run did not look at old markers. It has now shown
        // they are out of date, and a later automatic launch must not act on them.
        collect_dead_markers(&runtime.data);
    }
    let Ok(mut saved) = runtime.saved.lock() else {
        return;
    };
    let fell_back = matches!(runtime.decision.event, Event::FellBack | Event::StillTrying);
    let next = after_success(*saved, fell_back, backend == wgpu::Backend::Dx12);
    if next == *saved {
        return;
    }
    write_state(&runtime.data, next);
    match next {
        Saved::Dx12 => log::warn!(
            "Graphics API: DirectX 12 finished setup after the earlier failure, so later launches go straight to it. {}",
            retry_hint(&runtime.data)
        ),
        _ => log::info!(
            "Graphics API: {backend:?} finished setup, so the remembered fallback state is cleared"
        ),
    }
    *saved = next;
}

/// How the process is ending, for a marker still armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    /// A quit, or anything else that returned success.
    Clean,
    /// The returned-error path, which already reports to the player.
    Returned,
}

/// Normal endings. A panic does not call this: the marker stays and counts, since
/// a wgpu validation failure in setup arrives as a panic and a fallback can help.
///
/// A returned error does not count on the automatic choice. It has its own dialog
/// and report, and it cannot be told from an error that had nothing to do with
/// the GPU. It does count while DirectX 12 is the fallback under trial: otherwise
/// a DirectX 12 that keeps returning errors would be retried forever. The log
/// gives the manual route either way.
pub(crate) fn process_ending(ending: Ending) {
    let Some(runtime) = RUNTIME.get() else { return };
    let trial = matches!(runtime.decision.event, Event::FellBack | Event::StillTrying);
    let armed = runtime.marker.lock().is_ok_and(|marker| marker.is_some());
    if ending == Ending::Returned && armed {
        log::warn!(
            "Graphics API: this launch ended with an error during GPU setup. On Windows you can choose the API with --graphics-api auto, vulkan or dx12."
        );
        if trial {
            return;
        }
    }
    setup_ended();
}

/// One brief message for the player, once, when a fallback has just happened.
pub(crate) fn take_notice() -> Option<String> {
    RUNTIME
        .get()
        .and_then(|runtime| runtime.notice.lock().ok()?.take())
}

/// What the Graphics screen says about this run, when it is not what the row shows.
pub(crate) fn status_note() -> Option<String> {
    let runtime = RUNTIME.get()?;
    match runtime.decision.event {
        Event::Explicit if runtime.setting != GraphicsApi::Auto => {
            Some(format!("This run uses {}.", runtime.setting.label()))
        }
        Event::FellBack | Event::StillTrying | Event::Remembered => Some(
            "This run uses DX12 after a failed start. Choosing Vulkan retries it next launch."
                .into(),
        ),
        Event::JustExhausted | Event::Exhausted => {
            Some("Automatic fallback found nothing that worked. See the log.".into())
        }
        Event::Plain | Event::Explicit => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_EVIDENCE: Evidence = Evidence {
        other: false,
        dx12: false,
    };
    const DIED_ON_VULKAN: Evidence = Evidence {
        other: true,
        dx12: false,
    };
    const DIED_ON_DX12: Evidence = Evidence {
        other: false,
        dx12: true,
    };

    fn auto(evidence: Evidence, saved: Saved) -> Decision {
        decide(GraphicsApi::Auto, Platform::Windows, true, evidence, saved)
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

    #[test]
    fn automatic_with_nothing_to_go_on_is_todays_behaviour_with_a_marker() {
        assert_eq!(
            auto(NO_EVIDENCE, Saved::None),
            Decision {
                choice: Choice::Automatic,
                arm: true,
                saved: Saved::None,
                event: Event::Plain,
            }
        );
    }

    #[test]
    fn a_death_on_the_automatic_choice_tries_dx12_with_a_marker() {
        assert_eq!(
            auto(DIED_ON_VULKAN, Saved::None),
            Decision {
                choice: Choice::Dx12,
                arm: true,
                saved: Saved::Trying,
                event: Event::FellBack,
            }
        );
    }

    #[test]
    fn a_trial_that_ended_without_dying_is_tried_again() {
        let decision = auto(NO_EVIDENCE, Saved::Trying);
        assert_eq!((decision.choice, decision.arm), (Choice::Dx12, true));
        assert_eq!(decision.saved, Saved::Trying);
        assert_eq!(decision.event, Event::StillTrying);
        // A stale Vulkan marker from a sibling launch is not a DirectX 12 failure.
        assert_eq!(auto(DIED_ON_VULKAN, Saved::Trying), decision);
    }

    #[test]
    fn a_remembered_dx12_goes_straight_there() {
        let decision = auto(NO_EVIDENCE, Saved::Dx12);
        assert_eq!(decision.choice, Choice::Dx12);
        assert_eq!(decision.saved, Saved::Dx12);
        assert_eq!(decision.event, Event::Remembered);
        assert_eq!(auto(DIED_ON_VULKAN, Saved::Dx12), decision);
    }

    #[test]
    fn dx12_dying_too_stops_switching_for_good() {
        let expected = Decision {
            choice: Choice::Automatic,
            arm: false,
            saved: Saved::Exhausted,
            event: Event::JustExhausted,
        };
        assert_eq!(auto(DIED_ON_DX12, Saved::Trying), expected);
        assert_eq!(auto(DIED_ON_DX12, Saved::Dx12), expected);
        // The automatic choice was DirectX 12 already: nothing else to try.
        assert_eq!(auto(DIED_ON_DX12, Saved::None), expected);
        // Only one report: later launches stay put, whatever they find.
        for evidence in [NO_EVIDENCE, DIED_ON_VULKAN, DIED_ON_DX12] {
            assert_eq!(
                auto(evidence, Saved::Exhausted),
                Decision {
                    choice: Choice::Automatic,
                    arm: false,
                    saved: Saved::Exhausted,
                    event: Event::Exhausted,
                }
            );
        }
    }

    #[test]
    fn the_whole_cycle_never_loops() {
        // Every launch dies during setup, on whatever it was given.
        let mut saved = Saved::None;
        let mut evidence = NO_EVIDENCE;
        let mut seen = Vec::new();
        for _ in 0..10 {
            let decision = auto(evidence, saved);
            seen.push(decision.choice);
            saved = decision.saved;
            evidence = match (decision.arm, decision.choice) {
                (true, Choice::Dx12) => DIED_ON_DX12,
                (true, _) => DIED_ON_VULKAN,
                (false, _) => NO_EVIDENCE,
            };
        }
        assert_eq!(
            seen[..4],
            [
                Choice::Automatic,
                Choice::Dx12,
                Choice::Automatic,
                Choice::Automatic
            ]
        );
        assert!(seen[2..].iter().all(|choice| *choice == Choice::Automatic));
        assert_eq!(saved, Saved::Exhausted);
    }

    #[test]
    fn an_explicit_choice_is_obeyed_and_never_arms_or_changes_state() {
        for evidence in [NO_EVIDENCE, DIED_ON_VULKAN, DIED_ON_DX12] {
            for saved in [Saved::None, Saved::Trying, Saved::Dx12, Saved::Exhausted] {
                for attended in [true, false] {
                    assert_eq!(
                        decide(
                            GraphicsApi::Vulkan,
                            Platform::Windows,
                            attended,
                            evidence,
                            saved
                        ),
                        Decision {
                            choice: Choice::Vulkan,
                            arm: false,
                            saved,
                            event: Event::Explicit,
                        }
                    );
                    assert_eq!(
                        decide(
                            GraphicsApi::Dx12,
                            Platform::Windows,
                            attended,
                            evidence,
                            saved
                        )
                        .choice,
                        Choice::Dx12
                    );
                }
            }
        }
    }

    #[test]
    fn automated_runs_and_other_platforms_never_act_on_state() {
        for evidence in [NO_EVIDENCE, DIED_ON_VULKAN, DIED_ON_DX12] {
            for saved in [Saved::None, Saved::Trying, Saved::Dx12, Saved::Exhausted] {
                let automated =
                    decide(GraphicsApi::Auto, Platform::Windows, false, evidence, saved);
                assert_eq!(
                    (automated.choice, automated.arm, automated.saved),
                    (Choice::Automatic, false, saved)
                );
                for setting in [GraphicsApi::Auto, GraphicsApi::Vulkan, GraphicsApi::Dx12] {
                    for attended in [true, false] {
                        let other = decide(setting, Platform::Other, attended, evidence, saved);
                        assert_eq!(
                            (other.choice, other.arm, other.saved),
                            (Choice::Automatic, false, Saved::None)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn success_clears_or_remembers_as_it_should() {
        // The fallback worked: remember it.
        assert_eq!(after_success(Saved::Trying, true, true), Saved::Dx12);
        // Vulkan or the automatic choice worked: whatever failed is fixed.
        for saved in [Saved::None, Saved::Trying, Saved::Dx12, Saved::Exhausted] {
            assert_eq!(after_success(saved, false, false), Saved::None);
        }
        // An explicit DirectX 12 run proves nothing about the automatic choice.
        assert_eq!(after_success(Saved::None, false, true), Saved::None);
        assert_eq!(after_success(Saved::Trying, false, true), Saved::Trying);
        assert_eq!(
            after_success(Saved::Exhausted, false, true),
            Saved::Exhausted
        );
        assert_eq!(after_success(Saved::Dx12, false, true), Saved::Dx12);
    }

    #[test]
    fn state_files_round_trip_and_damage_means_no_state() {
        for saved in [Saved::Trying, Saved::Dx12, Saved::Exhausted] {
            let text = state_text(saved, "1.0").unwrap();
            assert_eq!(parse_state(&text, "1.0"), saved);
        }
        assert_eq!(state_text(Saved::None, "1.0"), None);
        assert_eq!(parse_state("", "1.0"), Saved::None);
        assert_eq!(parse_state("rubbish\nstate dx12\n", "1.0"), Saved::None);
        assert_eq!(
            parse_state("tore-gpu-fallback 1\nstate sideways\n", "1.0"),
            Saved::None
        );
        // A new build gets a fresh try after everything failed, but a working
        // DirectX 12 stays: Vulkan crashing once per update helps nobody.
        let exhausted = state_text(Saved::Exhausted, "1.0").unwrap();
        assert_eq!(parse_state(&exhausted, "1.1"), Saved::None);
        let remembered = state_text(Saved::Dx12, "1.0").unwrap();
        assert_eq!(parse_state(&remembered, "1.1"), Saved::Dx12);
    }

    #[test]
    fn a_live_processs_marker_is_left_alone_and_a_dead_ones_counts() {
        let data = profile("live");
        // Held by this process through its own handle: another launch sees a
        // locked file and must walk past it, as a sibling game does.
        let live = Marker::create(&data, "auto").unwrap();
        assert_eq!(collect_dead_markers(&data), NO_EVIDENCE);
        assert!(live.path.exists());
        // The holder goes away without any clean-up, as in a crash or kill.
        let Marker { file, path } = live;
        drop(file);
        assert_eq!(collect_dead_markers(&data), DIED_ON_VULKAN);
        assert!(!path.exists(), "a counted marker is consumed");
        assert_eq!(collect_dead_markers(&data), NO_EVIDENCE);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn a_clean_end_leaves_nothing_behind() {
        let data = profile("clean");
        let marker = Marker::create(&data, "auto").unwrap();
        let path = marker.path.clone();
        marker.remove();
        assert!(!path.exists());
        assert_eq!(collect_dead_markers(&data), NO_EVIDENCE);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn markers_record_the_backend_that_was_running() {
        let data = profile("record");
        let mut marker = Marker::create(&data, "auto").unwrap();
        marker.record("dx12").unwrap();
        let Marker { file, .. } = marker;
        drop(file);
        assert_eq!(collect_dead_markers(&data), DIED_ON_DX12);
        // A marker that lost its contents still means something died.
        let directory = data.join(MARKER_DIRECTORY);
        fs::write(directory.join(format!("empty{MARKER_SUFFIX}")), "").unwrap();
        assert_eq!(collect_dead_markers(&data), DIED_ON_VULKAN);
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn profiles_do_not_see_each_others_markers() {
        let first = profile("first");
        let second = profile("second");
        let Marker { file, .. } = Marker::create(&first, "auto").unwrap();
        drop(file);
        assert_eq!(collect_dead_markers(&second), NO_EVIDENCE);
        assert_eq!(collect_dead_markers(&first), DIED_ON_VULKAN);
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn instances_ask_for_only_what_was_chosen() {
        // The automatic choice must stay exactly the default descriptor.
        assert_eq!(
            descriptor(Choice::Automatic).backends,
            wgpu::InstanceDescriptor::default().backends
        );
        assert_eq!(descriptor(Choice::Vulkan).backends, wgpu::Backends::VULKAN);
        assert_eq!(descriptor(Choice::Dx12).backends, wgpu::Backends::DX12);
        assert!(
            descriptor(Choice::Dx12)
                .flags
                .contains(wgpu::InstanceFlags::DEBUG),
            "skip FXC's optimizer: minutes per launch otherwise"
        );
    }
}
