//! The wording of a native crash report, separate from the Windows calls so it
//! can be checked on every platform. Nothing here allocates: the reporter runs
//! in a process that has just crashed, possibly holding the heap lock.
use core::fmt::{self, Write};

/// A fixed text buffer. Writes past the end are dropped, never an error, and a
/// partial character is never stored.
pub struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn clear(&mut self) {
        self.length = 0;
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut take = text.len().min(N - self.length);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.length..self.length + take].copy_from_slice(&text.as_bytes()[..take]);
        self.length += take;
        Ok(())
    }
}

/// The Windows name of an exception code.
pub fn exception_name(code: u32) -> &'static str {
    match code {
        0x8000_0002 => "EXCEPTION_DATATYPE_MISALIGNMENT",
        0x8000_0003 => "EXCEPTION_BREAKPOINT",
        0x8000_0004 => "EXCEPTION_SINGLE_STEP",
        0xC000_0005 => "EXCEPTION_ACCESS_VIOLATION",
        0xC000_0006 => "EXCEPTION_IN_PAGE_ERROR",
        0xC000_001D => "EXCEPTION_ILLEGAL_INSTRUCTION",
        0xC000_0025 => "EXCEPTION_NONCONTINUABLE_EXCEPTION",
        0xC000_0026 => "EXCEPTION_INVALID_DISPOSITION",
        0xC000_008C => "EXCEPTION_ARRAY_BOUNDS_EXCEEDED",
        0xC000_008D => "EXCEPTION_FLT_DENORMAL_OPERAND",
        0xC000_008E => "EXCEPTION_FLT_DIVIDE_BY_ZERO",
        0xC000_008F => "EXCEPTION_FLT_INEXACT_RESULT",
        0xC000_0090 => "EXCEPTION_FLT_INVALID_OPERATION",
        0xC000_0091 => "EXCEPTION_FLT_OVERFLOW",
        0xC000_0092 => "EXCEPTION_FLT_STACK_CHECK",
        0xC000_0093 => "EXCEPTION_FLT_UNDERFLOW",
        0xC000_0094 => "EXCEPTION_INT_DIVIDE_BY_ZERO",
        0xC000_0095 => "EXCEPTION_INT_OVERFLOW",
        0xC000_0096 => "EXCEPTION_PRIV_INSTRUCTION",
        0xC000_00FD => "EXCEPTION_STACK_OVERFLOW",
        0xC000_0374 => "STATUS_HEAP_CORRUPTION",
        0xC000_0409 => "STATUS_STACK_BUFFER_OVERRUN",
        0xC000_041D => "STATUS_FATAL_USER_CALLBACK_EXCEPTION",
        0xE06D_7363 => "unhandled C++ exception",
        _ => "unrecognized exception code",
    }
}

/// The exception facts, copied by the filter before the helper thread wakes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Exception {
    pub code: u32,
    pub address: usize,
    pub parameter_count: u32,
    pub parameters: [usize; 2],
}

impl Exception {
    /// `read of 0x10`, for the two codes that carry an access description.
    fn write_access(&self, out: &mut impl Write) -> fmt::Result {
        if !matches!(self.code, 0xC000_0005 | 0xC000_0006) || self.parameter_count < 2 {
            return Ok(());
        }
        let kind = match self.parameters[0] {
            0 => "read of",
            1 => "write to",
            8 => "execution at",
            _ => "access to",
        };
        writeln!(out, "Access: {kind} {:#x}", self.parameters[1])
    }
}

/// What the report knows before asking the OS anything that could block.
pub struct Facts<'a> {
    pub unix_seconds: u64,
    pub milliseconds: u32,
    pub identity: &'a str,
    pub stage: &'a str,
    pub step: &'a str,
    pub adapter: &'a str,
    pub exception: Exception,
    pub thread: u32,
    pub main_thread: bool,
}

/// The line a report adds when the failure happened during GPU setup: the player
/// can do something about those. The startup stages that touch the GPU name
/// graphics or the first frame, and a step still running covers the pipelines
/// built after startup. `step` is the last GPU step as `gpu_setup` records it.
pub fn gpu_setup_hint(stage: &str, step: &str) -> Option<&'static str> {
    (stage.contains("graphics") || stage.contains("first frame") || step.starts_with("in "))
        .then_some(GPU_SETUP_HINT)
}
const GPU_SETUP_HINT: &str = "The graphics API can be changed: the next launch offers it, or start with --graphics-api vulkan or dx12.";

fn or_none(text: &str) -> &str {
    if text.is_empty() { "none" } else { text }
}

/// The part of the report written first, in the form `diagnostics::fatal` uses.
pub fn write_summary(out: &mut impl Write, facts: &Facts<'_>) -> fmt::Result {
    writeln!(out, "{}.{:03}Z", facts.unix_seconds, facts.milliseconds)?;
    writeln!(out, "{}", facts.identity)?;
    writeln!(
        out,
        "Stage: {}",
        if facts.stage.is_empty() {
            "running (no startup stage active)"
        } else {
            facts.stage
        }
    )?;
    write_cause(out, facts.exception)?;
    writeln!(out, "Faulting address: {:#x}", facts.exception.address)?;
    facts.exception.write_access(out)?;
    writeln!(
        out,
        "Thread: {} (id {})",
        if facts.main_thread { "main" } else { "other" },
        facts.thread
    )?;
    writeln!(out, "Last GPU step: {}", or_none(facts.step))?;
    writeln!(out, "Graphics adapter: {}", or_none(facts.adapter))?;
    if let Some(hint) = gpu_setup_hint(facts.stage, facts.step) {
        writeln!(out, "{hint}")?;
    }
    Ok(())
}

/// One line naming the exception, shared by the report, event and dialog.
pub fn write_cause(out: &mut impl Write, exception: Exception) -> fmt::Result {
    writeln!(
        out,
        "Native crash: {:#010x} {}",
        exception.code,
        exception_name(exception.code)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_names_cover_the_common_crashes() {
        assert_eq!(exception_name(0xC000_0005), "EXCEPTION_ACCESS_VIOLATION");
        assert_eq!(exception_name(0xC000_00FD), "EXCEPTION_STACK_OVERFLOW");
        assert_eq!(exception_name(0xC000_001D), "EXCEPTION_ILLEGAL_INSTRUCTION");
        assert_eq!(exception_name(1), "unrecognized exception code");
    }

    #[test]
    fn text_truncates_on_a_character_boundary() {
        let mut text = Text::<4>::new();
        write!(text, "ab\u{e9}\u{e9}").unwrap();
        assert_eq!(text.as_str(), "ab\u{e9}");
        write!(text, "more").unwrap();
        assert_eq!(text.len(), 4);
    }

    #[test]
    fn summary_names_everything_the_contract_asks_for() {
        let mut text = Text::<1024>::new();
        write_summary(
            &mut text,
            &Facts {
                unix_seconds: 1_000,
                milliseconds: 7,
                identity: "T.O.R.E-Fighters 1.2.3 commit=abc target=x",
                stage: "game graphics resources and pipelines",
                step: "in render pipeline 'Menu'",
                adapter: "AMD Radeon RX 7600 backend=Vulkan",
                exception: Exception {
                    code: 0xC000_0005,
                    address: 0x7ff8_1000,
                    parameter_count: 2,
                    parameters: [1, 0x10],
                },
                thread: 42,
                main_thread: true,
            },
        )
        .unwrap();
        let report = text.as_str();
        for wanted in [
            "1000.007Z\n",
            "commit=abc",
            "Stage: game graphics resources and pipelines",
            "Native crash: 0xc0000005 EXCEPTION_ACCESS_VIOLATION",
            "Faulting address: 0x7ff81000",
            "Access: write to 0x10",
            "Thread: main (id 42)",
            "Last GPU step: in render pipeline 'Menu'",
            "Graphics adapter: AMD Radeon RX 7600 backend=Vulkan",
        ] {
            assert!(report.contains(wanted), "{wanted} missing from {report}");
        }
    }

    #[test]
    fn only_gpu_setup_failures_mention_the_graphics_api() {
        for stage in [
            "game graphics resources and pipelines",
            "game graphics device creation",
            "first-run graphics adapter selection",
            "game first frame presentation",
            "aircraft graphics preparation",
        ] {
            assert!(gpu_setup_hint(stage, "").is_some(), "{stage}");
        }
        // A pipeline built after startup has no stage, but its step is running.
        assert!(gpu_setup_hint("", "in render pipeline 'Menu'").is_some());
        // A step that returned, or the self-test's note, is not a GPU failure.
        assert!(gpu_setup_hint("", "after render pipeline 'Menu' returned").is_none());
        for stage in [
            "diagnostics self-test: deliberate native crash",
            "asset loading or import",
            "game window creation",
            "",
        ] {
            assert!(
                gpu_setup_hint(stage, "deliberate self-test, no GPU step").is_none(),
                "{stage}"
            );
        }
    }

    #[test]
    fn a_crash_in_gpu_setup_says_the_api_can_change() {
        let mut text = Text::<1024>::new();
        write_summary(
            &mut text,
            &Facts {
                unix_seconds: 0,
                milliseconds: 0,
                identity: "",
                stage: "game graphics resources and pipelines",
                step: "in render pipeline 'Menu'",
                adapter: "",
                exception: Exception::default(),
                thread: 1,
                main_thread: true,
            },
        )
        .unwrap();
        assert!(text.as_str().contains("--graphics-api vulkan or dx12"));
    }

    #[test]
    fn a_crash_outside_any_stage_says_so() {
        let mut text = Text::<512>::new();
        write_summary(
            &mut text,
            &Facts {
                unix_seconds: 0,
                milliseconds: 0,
                identity: "",
                stage: "",
                step: "",
                adapter: "",
                exception: Exception {
                    code: 0xC000_00FD,
                    ..Exception::default()
                },
                thread: 1,
                main_thread: false,
            },
        )
        .unwrap();
        let report = text.as_str();
        assert!(report.contains("Stage: running (no startup stage active)"));
        assert!(report.contains("Thread: other (id 1)"));
        assert!(report.contains("Last GPU step: none"));
        assert!(!report.contains("Access:"));
    }
}
