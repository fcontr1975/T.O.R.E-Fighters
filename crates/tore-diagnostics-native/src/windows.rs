//! Direct Win32 ABI. Handles and UTF-16 buffers never escape this module.
use crate::CrashConfig;
use crate::windows_crash::{self, Config, ReportPath};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(window: *mut c_void, text: *const u16, title: *const u16, flags: u32) -> i32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegisterEventSourceW(server: *const u16, source: *const u16) -> *mut c_void;
    fn DeregisterEventSource(handle: *mut c_void) -> i32;
    fn ReportEventW(
        handle: *mut c_void,
        kind: u16,
        category: u16,
        id: u32,
        user: *const c_void,
        string_count: u16,
        data_size: u32,
        strings: *const *const u16,
        data: *const c_void,
    ) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    // Bound strings below ReportEventW's 31,839 UTF-16 character limit.
    // Replace embedded NULs so a filename or panic cannot hide the report suffix.
    let mut out = Vec::new();
    for character in text.chars() {
        let character = if character == '\0' {
            '\u{fffd}'
        } else {
            character
        };
        if out.len() + character.len_utf16() > 30_000 {
            break;
        }
        let mut buffer = [0; 2];
        out.extend_from_slice(character.encode_utf16(&mut buffer));
    }
    out.push(0);
    out
}

/// Copies a path into memory the crash reporter can read for the process
/// lifetime, because a crashed process must not allocate.
fn leaked_path(path: &Path) -> ReportPath {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    ReportPath {
        wide: Box::leak(wide.into_boxed_slice()),
        text: Box::leak(path.display().to_string().into_boxed_str()),
    }
}

pub fn install_crash_reporter(config: CrashConfig) -> Result<(), String> {
    let last_error_temporary = config.last_error.as_ref().map(|path| {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".native-{}.tmp", std::process::id()));
        leaked_path(Path::new(&name))
    });
    let leaked = Box::leak(Box::new(Config {
        identity: Box::leak(config.identity.into_boxed_str()),
        fatal: config.fatal_path.as_deref().map(leaked_path),
        session_log: config.session_log.as_deref().map(leaked_path),
        last_error: config.last_error.as_deref().map(leaked_path),
        last_error_temporary,
        dialog: config.dialog,
    }));
    windows_crash::install(leaked).map_err(str::to_owned)
}

pub fn show_error(title: &str, message: &str) -> Result<(), String> {
    let title = wide(title);
    let message = wide(message);
    // SAFETY: both buffers are NUL-terminated and live throughout the synchronous
    // call. A null owner is documented. MB_OK | MB_ICONERROR | MB_SETFOREGROUND.
    let result = unsafe { MessageBoxW(ptr::null_mut(), message.as_ptr(), title.as_ptr(), 0x10010) };
    if result == 0 {
        Err(format!("MessageBoxW: {}", std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

pub fn event_error(message: &str) -> Result<(), String> {
    let source = wide("T.O.R.E-Fighters");
    let message = wide(message);
    // SAFETY: local machine, valid terminated source name, synchronous call.
    let handle = unsafe { RegisterEventSourceW(ptr::null(), source.as_ptr()) };
    if handle.is_null() {
        return Err(format!(
            "RegisterEventSourceW: {}",
            std::io::Error::last_os_error()
        ));
    }
    let strings = [message.as_ptr()];
    // SAFETY: handle comes from RegisterEventSourceW. Exactly one live string
    // matches string_count; no SID or binary data. EVENTLOG_ERROR_TYPE is 1.
    let result = unsafe {
        ReportEventW(
            handle,
            1,
            0,
            1000,
            ptr::null(),
            1,
            0,
            strings.as_ptr(),
            ptr::null(),
        )
    };
    let error = (result == 0).then(std::io::Error::last_os_error);
    // SAFETY: release the valid handle exactly once, after ReportEventW returns.
    unsafe { DeregisterEventSource(handle) };
    error.map_or(Ok(()), |error| Err(format!("ReportEventW: {error}")))
}
