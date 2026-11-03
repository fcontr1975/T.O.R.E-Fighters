//! OS fatal-report boundary, independent of the game's window and GPU.
//! Policy (headless suppression and which failures are fatal) belongs to the caller.
pub mod crash_context;
pub mod crash_report;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_crash;

use std::path::PathBuf;

/// What a native crash report needs, decided while the process is still healthy.
#[derive(Clone, Debug, Default)]
pub struct CrashConfig {
    /// `T.O.R.E-Fighters 0.2.2 commit=... target=...`.
    pub identity: String,
    pub fatal_path: Option<PathBuf>,
    pub session_log: Option<PathBuf>,
    /// `last-error.txt` in the data directory.
    pub last_error: Option<PathBuf>,
    /// Show the native dialog as well.
    pub dialog: bool,
}

/// Installs the Windows native crash reporter. Native crashes are not reported
/// on other platforms, where this does nothing.
pub fn install_crash_reporter(config: CrashConfig) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return windows::install_crash_reporter(config);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = config;
        Ok(())
    }
}

/// Puts the crash filter back if a library replaced it after install. True when
/// it had been replaced. Always false off Windows.
pub fn reassert_crash_reporter() -> bool {
    #[cfg(target_os = "windows")]
    return windows_crash::reassert();
    #[cfg(not(target_os = "windows"))]
    false
}

/// The deliberate failures of `--diagnostics-self-test`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliberateCrash {
    AccessViolation,
    StackOverflow,
}

/// Crashes the process on Windows, on the calling thread, and does not return.
/// Elsewhere it returns the reason it cannot run.
pub fn deliberate_crash(kind: DeliberateCrash) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_crash::suppress_fault_window();
        match kind {
            DeliberateCrash::AccessViolation => windows_crash::deliberate_access_violation(),
            DeliberateCrash::StackOverflow => windows_crash::deliberate_stack_overflow(),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = kind;
        Err("this deliberate native crash test runs only on Windows; native crashes are not reported on this platform".into())
    }
}

/// Show an interactive error. Windows and macOS wait for acknowledgement.
/// macOS requires the main thread. Linux notification delivery has a two-second limit.
pub fn show_error(title: &str, message: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return windows::show_error(title, message);
    #[cfg(target_os = "macos")]
    return macos::show_error(title, message);
    #[cfg(target_os = "linux")]
    return linux::show_error(title, message);
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, message);
        Err("native error UI is unavailable on this platform".into())
    }
}

/// Best-effort Windows Application event, source T.O.R.E-Fighters, event 1000.
/// Other platforms intentionally do nothing.
pub fn event_error(message: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return windows::event_error(message);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = message;
        Ok(())
    }
}
