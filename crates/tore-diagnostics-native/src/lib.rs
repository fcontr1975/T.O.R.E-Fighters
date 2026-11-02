//! OS fatal-report boundary, independent of the game's window and GPU.
//! Policy (headless suppression and which failures are fatal) belongs to the caller.
pub mod crash_context;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

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
