//! Which graphics API the game asks wgpu for. On Windows the player chooses
//! between Vulkan and DirectX 12, and the game never changes that by itself.
//! Requested by John on 2026-11-02 after issue #8; docs/spec/graphics-options.md
//! holds the contract. Other platforms have no choice and use wgpu's own.
use crate::graphics::{GraphicsApi, Options};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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

/// The API for this process. A flag beats the saved value, and with neither (or
/// `--graphics-api auto`) it is Vulkan when a Vulkan adapter exists, otherwise
/// DirectX 12. `vulkan_available` runs only when that default is needed.
pub(crate) fn resolve(
    platform: Platform,
    flag: Option<GraphicsApi>,
    saved: GraphicsApi,
    vulkan_available: impl FnOnce() -> bool,
) -> Choice {
    if platform == Platform::Other {
        return Choice::Automatic;
    }
    match flag.unwrap_or(saved) {
        GraphicsApi::Vulkan => Choice::Vulkan,
        GraphicsApi::Dx12 => Choice::Dx12,
        GraphicsApi::Auto if vulkan_available() => Choice::Vulkan,
        GraphicsApi::Auto => Choice::Dx12,
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
pub(crate) fn select(data: &Path, flag: Option<GraphicsApi>, saved: GraphicsApi, saves: bool) {
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
    let _ = RUNTIME.set(Runtime {
        data: data.to_owned(),
        flag,
        saves,
        saved: Mutex::new(saved),
        current: Mutex::new(None),
    });
}

/// Whether a Vulkan adapter exists. Creating the instance and listing adapters is
/// what wgpu does for any launch, and it does not create a device.
fn vulkan_available() -> bool {
    let _step = crate::gpu_setup::begin("adapter enumeration", "Vulkan");
    #[allow(clippy::disallowed_methods)]
    let instance = wgpu::Instance::new(&descriptor(Choice::Vulkan));
    !instance
        .enumerate_adapters(wgpu::Backends::VULKAN)
        .is_empty()
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
    let choice = resolve(platform(), runtime.flag, saved, vulkan_available);
    log::info!(
        "Graphics API: {choice:?} (flag={:?} saved={})",
        runtime.flag.map(GraphicsApi::key),
        saved.key()
    );
    *current = Some(choice);
    choice
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
