//! Windows unhandled-exception reporting: report file first, then the optional
//! event and dialog, then back to Windows so its own "Application Error" entry
//! and any dump settings still happen. Contract: docs/spec/startup-diagnostics.md.
//!
//! This module uses `core` only. The filter runs on the crashed thread, which
//! may hold the heap lock and may have almost no stack, so it copies the facts
//! into atomics and wakes a thread started at install time. That helper has its
//! own stack and does the slow work. Every wait in the filter is bounded: a
//! helper that blocks (the loader lock or heap lock held by the crashed thread)
//! costs the report's later steps, not the Windows crash record.
//!
//! Unhandled-filter semantics give the rest of the guarantees. Vectored and
//! `__try` handlers, including deliberate driver exceptions, run first and the
//! filter never sees what they handle. Rust panics are caught long before it.
//! Rust's own stack-overflow vectored handler prints a line and continues the
//! search, so the filter still runs after it.
use crate::crash_context as context;
use crate::crash_report::{self, Exception, Facts, Text};
use core::ffi::c_void;
use core::fmt::{self, Write};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};

type Handle = *mut c_void;

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    parameter_count: u32,
    information: [usize; 15],
}
#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}
type Filter = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;
type ThreadStart = unsafe extern "system" fn(*mut c_void) -> u32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
    fn CreateEventW(
        attributes: *const c_void,
        manual_reset: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CreateThread(
        attributes: *const c_void,
        stack: usize,
        start: ThreadStart,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        attributes: u32,
        template: Handle,
    ) -> Handle;
    fn WriteFile(
        file: Handle,
        data: *const u8,
        size: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn FlushFileBuffers(file: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Handle) -> i32;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn ReadProcessMemory(
        process: Handle,
        address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn GetStdHandle(which: u32) -> Handle;
    fn GetSystemTimeAsFileTime(time: *mut u64);
    fn OpenThread(access: u32, inherit: i32, id: u32) -> Handle;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(window: *mut c_void, text: *const u16, title: *const u16, flags: u32) -> i32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegisterEventSourceW(server: *const u16, source: *const u16) -> Handle;
    fn DeregisterEventSource(handle: Handle) -> i32;
    fn ReportEventW(
        handle: Handle,
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

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const WAIT_FOREVER: u32 = 0xFFFF_FFFF;
const INVALID_HANDLE: Handle = usize::MAX as Handle;
/// The agent-chosen limits on how long a crash can delay Windows' own handling.
/// Files normally take milliseconds; ten seconds covers a stalled antivirus scan.
const REPORT_WAIT_MS: u32 = 10_000;
/// Event logging afterwards, when no dialog is waiting for the player.
const FINISH_WAIT_MS: u32 = 5_000;
/// A dialog nobody answers: the game has already stopped, so give up eventually.
const DIALOG_WAIT_MS: u32 = 120_000;
/// Reserved for the helper: its text buffers and the OS calls it makes.
const HELPER_STACK: usize = 512 * 1024;
const STACK_SIZE_PARAM_IS_A_RESERVATION: u32 = 0x0001_0000;
/// How far up the crashed stack to look for code addresses.
const SCAN_BYTES: usize = 64 * 1024;
const SCAN_LINES: usize = 24;

/// A path both as Windows wants it (NUL-terminated) and as the report shows it.
#[derive(Clone, Copy)]
pub struct ReportPath {
    pub wide: &'static [u16],
    pub text: &'static str,
}

pub struct Config {
    /// `T.O.R.E-Fighters 0.2.2 commit=... target=...`.
    pub identity: &'static str,
    pub fatal: Option<ReportPath>,
    pub session_log: Option<ReportPath>,
    pub last_error: Option<ReportPath>,
    /// Beside `last_error`, for an atomic replacement.
    pub last_error_temporary: Option<ReportPath>,
    pub dialog: bool,
}

static CONFIG: AtomicPtr<Config> = AtomicPtr::new(ptr::null_mut());
static REQUEST: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static REPORTED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static FINISHED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static HELPER_THREAD: AtomicU32 = AtomicU32::new(0);
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static PREVIOUS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static CRASHING: AtomicBool = AtomicBool::new(false);
static CRASH_THREAD: AtomicU32 = AtomicU32::new(0);
static CODE: AtomicU32 = AtomicU32::new(0);
static ADDRESS: AtomicUsize = AtomicUsize::new(0);
static PARAMETER_COUNT: AtomicU32 = AtomicU32::new(0);
static PARAMETER_0: AtomicUsize = AtomicUsize::new(0);
static PARAMETER_1: AtomicUsize = AtomicUsize::new(0);
static STACK_POINTER: AtomicUsize = AtomicUsize::new(0);

/// Where `CONTEXT` keeps the stack pointer. The layouts differ between the two
/// Windows targets we ship (`Rsp` and `Esp`); elsewhere the stack is not scanned.
#[cfg(target_arch = "x86_64")]
const STACK_POINTER_OFFSET: Option<usize> = Some(0x98);
#[cfg(target_arch = "x86")]
const STACK_POINTER_OFFSET: Option<usize> = Some(0xC4);
#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
const STACK_POINTER_OFFSET: Option<usize> = None;

/// Starts the helper and installs the filter. Call once, on the main thread.
pub fn install(config: &'static Config) -> Result<(), &'static str> {
    // SAFETY: null attributes and name are documented. The events are never closed.
    let events = unsafe {
        [
            CreateEventW(ptr::null(), 1, 0, ptr::null()),
            CreateEventW(ptr::null(), 1, 0, ptr::null()),
            CreateEventW(ptr::null(), 1, 0, ptr::null()),
        ]
    };
    if events.iter().any(|event| event.is_null()) {
        return Err("CreateEventW failed");
    }
    REQUEST.store(events[0], Ordering::Release);
    REPORTED.store(events[1], Ordering::Release);
    FINISHED.store(events[2], Ordering::Release);
    CONFIG.store(ptr::from_ref(config).cast_mut(), Ordering::Release);
    // SAFETY: no out parameters on this call.
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let mut helper_id = 0;
    // SAFETY: `helper` has the ThreadStart ABI and takes no parameter; the id
    // outlives the call. The handle is deliberately never closed or joined, and
    // process exit ends the waiting thread.
    let helper_thread = unsafe {
        CreateThread(
            ptr::null(),
            HELPER_STACK,
            helper,
            ptr::null_mut(),
            STACK_SIZE_PARAM_IS_A_RESERVATION,
            &mut helper_id,
        )
    };
    if helper_thread.is_null() {
        return Err("CreateThread failed");
    }
    HELPER_THREAD.store(helper_id, Ordering::Release);
    // SAFETY: `filter` has the documented LPTOP_LEVEL_EXCEPTION_FILTER ABI and
    // lives as long as the process.
    let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    remember_previous(previous);
    Ok(())
}

fn remember_previous(previous: Option<Filter>) {
    PREVIOUS.store(
        previous.map_or(ptr::null_mut(), |function| function as *mut c_void),
        Ordering::Release,
    );
}

/// Puts the filter back if something (a driver DLL, say) replaced it, keeping
/// that filter next in the chain. True when it had been replaced. Before install
/// this does nothing.
pub fn reassert() -> bool {
    if CONFIG.load(Ordering::Acquire).is_null() {
        return false;
    }
    // SAFETY: as in `install`.
    let current = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    match current {
        Some(function) if function as usize == filter as *const () as usize => false,
        other => {
            remember_previous(other);
            true
        }
    }
}

/// The filter. It must stay small: after a stack overflow only the stack that
/// the OS and Rust's guarantee left is available. No heap, no locks.
unsafe extern "system" fn filter(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: no pointers involved.
    let thread = unsafe { GetCurrentThreadId() };
    if thread == HELPER_THREAD.load(Ordering::Acquire) {
        // The helper itself faulted: nothing to wait for, and nobody else will write.
        return chain(pointers);
    }
    if CRASHING.swap(true, Ordering::AcqRel) {
        if CRASH_THREAD.load(Ordering::Acquire) != thread {
            // A second crash on another thread: the first owns the report. Hold
            // this thread until it is done so the process does not die under it.
            // SAFETY: the event handle stays open for the process lifetime.
            unsafe { WaitForSingleObject(FINISHED.load(Ordering::Acquire), REPORT_WAIT_MS) };
        }
        return EXCEPTION_CONTINUE_SEARCH;
    }
    CRASH_THREAD.store(thread, Ordering::Release);
    // SAFETY: the OS passes valid pointers. The record and context live on this
    // thread's stack and are read here, before anything else happens.
    unsafe {
        if !pointers.is_null() && !(*pointers).record.is_null() {
            let record = &*(*pointers).record;
            CODE.store(record.code, Ordering::Relaxed);
            ADDRESS.store(record.address as usize, Ordering::Relaxed);
            PARAMETER_COUNT.store(record.parameter_count, Ordering::Relaxed);
            PARAMETER_0.store(record.information[0], Ordering::Relaxed);
            PARAMETER_1.store(record.information[1], Ordering::Relaxed);
            if let Some(offset) = STACK_POINTER_OFFSET
                && !(*pointers).context.is_null()
            {
                let at = (*pointers).context.cast::<u8>().add(offset).cast::<usize>();
                STACK_POINTER.store(at.read_unaligned(), Ordering::Relaxed);
            }
        }
    }
    let config = CONFIG.load(Ordering::Acquire);
    // SAFETY: event handles stay open for the process lifetime; `config` is
    // leaked at install and never null once the filter is installed.
    unsafe {
        SetEvent(REQUEST.load(Ordering::Acquire));
        WaitForSingleObject(REPORTED.load(Ordering::Acquire), REPORT_WAIT_MS);
        let finish = if (*config).dialog {
            DIALOG_WAIT_MS
        } else {
            FINISH_WAIT_MS
        };
        WaitForSingleObject(FINISHED.load(Ordering::Acquire), finish);
    }
    chain(pointers)
}

/// Lets a filter installed before ours run, as it would have without us. The
/// C runtime's one, for example, handles only C++ terminate cases.
fn chain(pointers: *mut ExceptionPointers) -> i32 {
    let previous = PREVIOUS.load(Ordering::Acquire);
    if previous.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // SAFETY: it was returned by SetUnhandledExceptionFilter as a filter of this ABI.
    let previous: Filter = unsafe { core::mem::transmute(previous) };
    // SAFETY: the same pointers the OS gave us, on the same thread.
    unsafe { previous(pointers) }
}

unsafe extern "system" fn helper(_: *mut c_void) -> u32 {
    // SAFETY: the event handle stays open for the process lifetime.
    unsafe { WaitForSingleObject(REQUEST.load(Ordering::Acquire), WAIT_FOREVER) };
    report();
    // SAFETY: as above. The filter continues as soon as these are set.
    unsafe { SetEvent(FINISHED.load(Ordering::Acquire)) };
    0
}

/// UTF-16 shown as text, lossily.
struct Wide<'a>(&'a [u16]);
impl fmt::Display for Wide<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        for unit in char::decode_utf16(self.0.iter().copied()) {
            out.write_char(unit.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        Ok(())
    }
}

fn append(path: Option<ReportPath>, data: &[u8], flush: bool) {
    let Some(path) = path else { return };
    // SAFETY: the path is NUL-terminated and leaked at install. FILE_APPEND_DATA,
    // sharing everything, OPEN_ALWAYS, normal attributes. `data` outlives the
    // synchronous write, which is all of it or nothing we can do anything about.
    unsafe {
        let file = CreateFileW(
            path.wide.as_ptr(),
            4,
            7,
            ptr::null(),
            4,
            0x80,
            ptr::null_mut(),
        );
        if file == INVALID_HANDLE {
            return;
        }
        let mut written = 0;
        WriteFile(
            file,
            data.as_ptr(),
            data.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
        if flush {
            FlushFileBuffers(file);
        }
        CloseHandle(file);
    }
}

/// Replaces the file through a temporary, as `preferences::write` does, so a
/// reader never sees half a report.
fn replace(path: Option<ReportPath>, temporary: Option<ReportPath>, first: &[u8], second: &[u8]) {
    let (Some(path), Some(temporary)) = (path, temporary) else {
        return;
    };
    // SAFETY: NUL-terminated leaked paths; GENERIC_WRITE, CREATE_ALWAYS. Buffers
    // outlive the synchronous writes. MOVEFILE_REPLACE_EXISTING | WRITE_THROUGH.
    unsafe {
        let file = CreateFileW(
            temporary.wide.as_ptr(),
            0x4000_0000,
            0,
            ptr::null(),
            2,
            0x80,
            ptr::null_mut(),
        );
        if file == INVALID_HANDLE {
            return;
        }
        let mut written = 0;
        WriteFile(
            file,
            first.as_ptr(),
            first.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
        WriteFile(
            file,
            second.as_ptr(),
            second.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
        FlushFileBuffers(file);
        CloseHandle(file);
        MoveFileExW(temporary.wide.as_ptr(), path.wide.as_ptr(), 1 | 8);
    }
}

/// The loaded module an address belongs to, as its base address.
fn module_of(address: usize) -> Option<usize> {
    if address == 0 {
        return None;
    }
    let mut module = ptr::null_mut();
    // SAFETY: FROM_ADDRESS (4) | UNCHANGED_REFCOUNT (2) treats the name as an
    // address and does not dereference it; the out pointer is valid.
    let found = unsafe { GetModuleHandleExW(6, address as *const u16, &mut module) };
    (found != 0 && !module.is_null()).then_some(module as usize)
}

/// The module's path, in `buffer`.
fn module_path(base: usize, buffer: &mut [u16]) -> &[u16] {
    // SAFETY: the buffer is valid for the size given.
    let length =
        unsafe { GetModuleFileNameW(base as Handle, buffer.as_mut_ptr(), buffer.len() as u32) };
    &buffer[..(length as usize).min(buffer.len())]
}

/// The part after the last backslash.
fn base_name(path: &[u16]) -> &[u16] {
    let start = path
        .iter()
        .rposition(|unit| *unit == u16::from(b'\\'))
        .map_or(0, |at| at + 1);
    &path[start..]
}

/// Lists values on the crashed stack that point into loaded modules. These are
/// candidates, not a verified call chain: the stack also holds stale values.
/// With the matching symbols they usually show who called the faulting code.
fn write_stack_scan(out: &mut impl Write, stack_pointer: usize) -> fmt::Result {
    if stack_pointer == 0 {
        return Ok(());
    }
    writeln!(out, "Code addresses on the crashed stack, newest first:")?;
    let word = core::mem::size_of::<usize>();
    let mut at = stack_pointer & !(word - 1);
    let end = at.saturating_add(SCAN_BYTES);
    let mut page = [0u8; 4096];
    let mut name = [0u16; 512];
    let mut lines = 0;
    let mut last_value = 0;
    let mut last_base = 0;
    let mut name_length = 0;
    while at < end && lines < SCAN_LINES {
        let chunk = (4096 - at % 4096).min(page.len());
        let mut read = 0;
        // SAFETY: reading our own process through the OS, which fails rather
        // than faults at the end of the stack. The buffer holds `chunk` bytes.
        let readable = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                at as *const c_void,
                page.as_mut_ptr().cast(),
                chunk,
                &mut read,
            )
        };
        if readable == 0 || read < word {
            break;
        }
        for bytes in page[..read].chunks_exact(word) {
            let mut value = 0usize;
            for (shift, byte) in bytes.iter().enumerate() {
                value |= usize::from(*byte) << (shift * 8);
            }
            if value == last_value || lines == SCAN_LINES {
                continue;
            }
            let Some(base) = module_of(value) else {
                continue;
            };
            if base != last_base {
                let path = module_path(base, &mut name);
                name_length = path.len();
                last_base = base;
            }
            let short = base_name(&name[..name_length]);
            writeln!(out, "  {}+{:#x}", Wide(short), value - base)?;
            last_value = value;
            lines += 1;
        }
        at += chunk;
    }
    if lines == 0 {
        writeln!(out, "  none found")?;
    }
    Ok(())
}

fn system_time() -> (u64, u32) {
    let mut filetime = 0u64;
    // SAFETY: a valid out pointer. FILETIME is two 32-bit halves, low first,
    // laid out as one little-endian u64 at an alignment the OS does not rely on.
    unsafe { GetSystemTimeAsFileTime(&mut filetime) };
    let milliseconds = filetime.saturating_sub(116_444_736_000_000_000) / 10_000;
    (milliseconds / 1000, (milliseconds % 1000) as u32)
}

/// Encodes `text` into `out` with a final NUL; too-long text is cut.
fn to_wide<'a>(text: &str, out: &'a mut [u16]) -> &'a [u16] {
    let mut length = 0;
    for unit in text.encode_utf16() {
        if length + 1 == out.len() {
            break;
        }
        out[length] = if unit == 0 { 0xFFFD } else { unit };
        length += 1;
    }
    out[length] = 0;
    &out[..=length]
}

fn report() {
    let config = CONFIG.load(Ordering::Acquire);
    if config.is_null() {
        return;
    }
    // SAFETY: leaked at install and never written again.
    let config = unsafe { &*config };
    let exception = Exception {
        code: CODE.load(Ordering::Relaxed),
        address: ADDRESS.load(Ordering::Relaxed),
        parameter_count: PARAMETER_COUNT.load(Ordering::Relaxed),
        parameters: [
            PARAMETER_0.load(Ordering::Relaxed),
            PARAMETER_1.load(Ordering::Relaxed),
        ],
    };
    let thread = CRASH_THREAD.load(Ordering::Acquire);
    let (unix_seconds, milliseconds) = system_time();
    let mut stage = [0; context::CELL_BYTES];
    let mut step = [0; context::CELL_BYTES];
    let mut adapter = [0; context::CELL_BYTES];
    let stage = context::STAGE.read(&mut stage);
    let step = context::GPU_STEP.read(&mut step);
    let adapter = context::ADAPTER.read(&mut adapter);
    let marker_armed = context::MARKER_ARMED.load(Ordering::Acquire);

    let mut text = Text::<16384>::new();
    let _ = crash_report::write_summary(
        &mut text,
        &Facts {
            unix_seconds,
            milliseconds,
            identity: config.identity,
            stage,
            step,
            adapter,
            marker_armed,
            exception,
            thread,
            main_thread: thread == MAIN_THREAD.load(Ordering::Acquire),
        },
    );
    // The part that needs nothing from the loader goes to disk first.
    let mut location = Text::<2048>::new();
    if let Some(path) = config.session_log {
        let _ = writeln!(location, "Session log: {}", path.text);
    }
    let mut report_location = Text::<1024>::new();
    match config.fatal {
        Some(path) => {
            let _ = writeln!(report_location, "Fatal report: {}", path.text);
        }
        None => {
            let _ = writeln!(report_location, "Fatal report could not be saved.");
        }
    }
    let early = text.len();
    append(config.fatal, text.as_str().as_bytes(), true);
    append(config.session_log, text.as_str().as_bytes(), false);
    replace(
        config.last_error,
        config.last_error_temporary,
        text.as_str().as_bytes(),
        report_location.as_str().as_bytes(),
    );

    // GetModuleHandleExW can wait for the loader lock, which the crashed thread
    // may hold; the bounded wait in the filter covers that.
    let mut path_buffer = [0u16; 1024];
    let module = module_of(exception.address);
    let module_text_at = text.len();
    match module {
        Some(base) => {
            let path = module_path(base, &mut path_buffer);
            let _ = writeln!(
                text,
                "Faulting module: {}+{:#x}",
                Wide(path),
                exception.address - base
            );
        }
        None => {
            let _ = writeln!(
                text,
                "Faulting module: none (no loaded module at that address)"
            );
        }
    }
    let module_text_end = text.len();
    let _ = write_stack_scan(&mut text, STACK_POINTER.load(Ordering::Relaxed));
    let _ = write!(text, "{}", location.as_str());
    let complete = text.as_str();
    append(config.fatal, complete.as_bytes()[early..].as_ref(), true);
    append(
        config.session_log,
        complete.as_bytes()[early..].as_ref(),
        false,
    );
    replace(
        config.last_error,
        config.last_error_temporary,
        complete.as_bytes(),
        report_location.as_str().as_bytes(),
    );
    // SAFETY: the event handle stays open for the process lifetime.
    unsafe { SetEvent(REPORTED.load(Ordering::Acquire)) };

    // From here on calls may take OS locks or allocate inside the OS. The report
    // is already safe; the filter stops waiting at its bounded timeout.
    let module_line = complete
        .get(module_text_at..module_text_end)
        .unwrap_or_default();
    write_stderr(complete);
    write_thread_name(config.fatal, thread);
    write_event(
        config,
        stage,
        step,
        exception,
        module_line,
        report_location.as_str(),
    );
    if config.dialog {
        show_dialog(
            stage,
            step,
            marker_armed,
            exception,
            module_line,
            report_location.as_str(),
        );
    }
}

fn write_stderr(text: &str) {
    // SAFETY: STD_ERROR_HANDLE. A GUI launch has none; a closed pipe just fails.
    unsafe {
        let handle = GetStdHandle(0xFFFF_FFF4);
        if !handle.is_null() && handle != INVALID_HANDLE {
            let mut written = 0;
            WriteFile(
                handle,
                text.as_ptr(),
                text.len() as u32,
                &mut written,
                ptr::null_mut(),
            );
        }
    }
}

/// Adds the crashed thread's description. GetThreadDescription allocates in the
/// OS, so this comes after the report is safe. It is absent before Windows 10
/// 1607, hence the lookup.
fn write_thread_name(fatal: Option<ReportPath>, thread: u32) {
    type GetThreadDescription = unsafe extern "system" fn(Handle, *mut *mut u16) -> i32;
    let mut kernel32 = [0u16; 16];
    let kernel32 = to_wide("kernel32.dll", &mut kernel32);
    // SAFETY: NUL-terminated name, loaded in every process.
    let module = unsafe { GetModuleHandleW(kernel32.as_ptr()) };
    if module.is_null() {
        return;
    }
    // SAFETY: valid module and NUL-terminated symbol name.
    let function = unsafe { GetProcAddress(module, c"GetThreadDescription".as_ptr().cast()) };
    if function.is_null() {
        return;
    }
    // SAFETY: the export has this signature on every version that has it.
    let function: GetThreadDescription = unsafe { core::mem::transmute(function) };
    // SAFETY: THREAD_QUERY_LIMITED_INFORMATION; a failed open returns null.
    let handle = unsafe { OpenThread(0x0800, 0, thread) };
    if handle.is_null() {
        return;
    }
    let mut name = ptr::null_mut();
    // SAFETY: valid handle and out pointer. On success the name is NUL-terminated
    // and owned by us until LocalFree. The handle is closed exactly once.
    unsafe {
        if function(handle, &mut name) >= 0 && !name.is_null() {
            let mut length = 0;
            while length < 256 && *name.add(length) != 0 {
                length += 1;
            }
            if length > 0 {
                let mut line = Text::<1024>::new();
                let _ = writeln!(
                    line,
                    "Thread name: {}",
                    Wide(core::slice::from_raw_parts(name, length))
                );
                append(fatal, line.as_str().as_bytes(), true);
            }
            LocalFree(name.cast());
        }
        CloseHandle(handle);
    }
}

fn write_event(
    config: &Config,
    stage: &str,
    step: &str,
    exception: Exception,
    module_line: &str,
    report_location: &str,
) {
    let mut text = Text::<4096>::new();
    let _ = writeln!(text, "{}", config.identity);
    let _ = writeln!(text, "Stage: {stage}");
    let _ = write!(text, "{report_location}");
    if let Some(path) = config.session_log {
        let _ = writeln!(text, "Session log: {}", path.text);
    }
    let _ = write!(text, "Reason: ");
    let _ = crash_report::write_cause(&mut text, exception);
    let _ = write!(text, "{module_line}");
    let _ = writeln!(text, "Last GPU step: {step}");
    let mut wide = [0u16; 4096];
    let wide = to_wide(text.as_str(), &mut wide);
    let mut source = [0u16; 32];
    let source = to_wide("T.O.R.E-Fighters", &mut source);
    // SAFETY: the same call shape as `windows::event_error`: local machine, one
    // live NUL-terminated string, no SID or binary data, handle released once.
    unsafe {
        let handle = RegisterEventSourceW(ptr::null(), source.as_ptr());
        if handle.is_null() {
            return;
        }
        let strings = [wide.as_ptr()];
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
        );
        DeregisterEventSource(handle);
    }
}

fn show_dialog(
    stage: &str,
    step: &str,
    marker_armed: bool,
    exception: Exception,
    module_line: &str,
    report_location: &str,
) {
    let mut text = Text::<4096>::new();
    if stage.is_empty() {
        let _ = write!(text, "T.O.R.E-Fighters crashed.\n\n");
    } else {
        let _ = write!(text, "T.O.R.E-Fighters crashed during {stage}.\n\n");
    }
    let _ = crash_report::write_cause(&mut text, exception);
    let _ = write!(text, "{module_line}");
    let _ = writeln!(text, "Last GPU step: {step}");
    if let Some(hint) = crash_report::gpu_setup_hint(stage, step, marker_armed) {
        let _ = writeln!(text, "\n{hint}");
    }
    let _ = write!(text, "\n{report_location}");
    let mut wide = [0u16; 4096];
    let wide = to_wide(text.as_str(), &mut wide);
    let mut title = [0u16; 32];
    let title = to_wide("T.O.R.E-Fighters error", &mut title);
    // SAFETY: both NUL-terminated buffers outlive the synchronous call. A null
    // owner is documented. MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST, because
    // the frozen game window may be fullscreen.
    unsafe { MessageBoxW(ptr::null_mut(), wide.as_ptr(), title.as_ptr(), 0x5_0010) };
}

/// A real access violation for the self-test: a write to an unmapped low page.
#[inline(never)]
pub fn deliberate_access_violation() -> ! {
    // SAFETY: not safe at all, and that is the point of this deliberate test: it
    // faults and never returns. The address is non-null so Rust's debug-build
    // null check does not turn the crash into a panic.
    unsafe {
        ptr::without_provenance_mut::<u8>(0x10).write_volatile(1);
    }
    unreachable!("the write above faults")
}

/// Real stack exhaustion for the self-test, on the calling thread.
pub fn deliberate_stack_overflow() -> ! {
    let _ = core::hint::black_box(recurse(1));
    unreachable!("the recursion above faults")
}

#[inline(never)]
fn recurse(depth: usize) -> usize {
    // Smaller than a page, so each frame touches the guard page in turn.
    let mut frame = [0u8; 1024];
    frame[depth % 1024] = depth as u8;
    let frame = core::hint::black_box(&mut frame);
    let deeper = if depth == usize::MAX {
        0
    } else {
        recurse(core::hint::black_box(depth + 1))
    };
    deeper + usize::from(frame[depth % 1024])
}

/// Keeps a deliberate test from waiting on the Windows crash window while it
/// still records the crash: WER_FAULT_REPORTING_NO_UI.
pub fn suppress_fault_window() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WerSetFlags(flags: u32) -> i32;
    }
    // SAFETY: no pointers; failure only means the window may appear.
    unsafe { WerSetFlags(0x20) };
}
