//! What the process was doing, readable from a crash report without a lock or
//! the heap: the active startup stage, the last GPU step and the chosen adapter.
//!
//! Each cell is a fixed row of atomic bytes. A reader racing a writer can see a
//! mix of two strings, which a crash report tolerates; it can never see invalid
//! memory or block. Text is reduced to printable ASCII when stored, so a torn
//! read is still valid UTF-8.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// Long enough for a GPU step line or an adapter, driver and backend line.
pub const CELL_BYTES: usize = 240;

pub struct TextCell {
    bytes: [AtomicU8; CELL_BYTES],
    length: AtomicUsize,
}

impl TextCell {
    const fn new() -> Self {
        Self {
            bytes: [const { AtomicU8::new(0) }; CELL_BYTES],
            length: AtomicUsize::new(0),
        }
    }
    pub fn set(&self, text: &str) {
        self.set_parts(&[text]);
    }
    /// Stores the concatenation of `parts`, without allocating.
    pub fn set_parts(&self, parts: &[&str]) {
        let mut at = 0;
        for byte in parts.iter().flat_map(|part| part.bytes()) {
            if at == CELL_BYTES {
                break;
            }
            let shown = if (0x20..0x7f).contains(&byte) {
                byte
            } else {
                b'?'
            };
            self.bytes[at].store(shown, Ordering::Relaxed);
            at += 1;
        }
        self.length.store(at, Ordering::Release);
    }
    /// Copies the text into `out` and returns it as a string slice.
    pub fn read<'a>(&self, out: &'a mut [u8; CELL_BYTES]) -> &'a str {
        let length = self.length.load(Ordering::Acquire).min(CELL_BYTES);
        for (slot, byte) in out.iter_mut().zip(&self.bytes[..length]) {
            *slot = byte.load(Ordering::Relaxed);
        }
        core::str::from_utf8(&out[..length]).unwrap_or("(unreadable)")
    }
}

/// The active startup stage; empty when none is active.
pub static STAGE: TextCell = TextCell::new();
/// The last GPU setup step and whether it was still running.
pub static GPU_STEP: TextCell = TextCell::new();
/// The selected adapter, driver and backend once known.
pub static ADAPTER: TextCell = TextCell::new();
/// True while this process holds a GPU setup marker, so a report can promise what
/// the next launch will do only when it will do it.
pub static MARKER_ARMED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_bounded_and_printable() {
        let cell = TextCell::new();
        let mut out = [0; CELL_BYTES];
        assert_eq!(cell.read(&mut out), "");
        cell.set_parts(&["in render pipeline '", "Menu", "'"]);
        assert_eq!(cell.read(&mut out), "in render pipeline 'Menu'");
        cell.set("caf\u{e9}\nline");
        assert_eq!(cell.read(&mut out), "caf???line");
        cell.set(&"x".repeat(CELL_BYTES * 2));
        assert_eq!(cell.read(&mut out).len(), CELL_BYTES);
    }
}
