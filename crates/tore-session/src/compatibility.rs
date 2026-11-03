//! Portable compatibility identity. See docs/ARCHITECTURE.md, Compatibility code.
use tore_world::content::Content;

/// Raise for changes to simulation, rules, checkpoint or replay interpretation
/// that can make peers disagree. Also raise the network protocol for such changes.
/// Release tags, documentation and presentation changes do not raise this value.
pub const SIMULATION_VERSION: u32 = 11;

/// FNV-1a/32 over a domain, protocol, simulation version and sorted playable
/// content item digests. Integers are little endian; keys are length prefixed.
/// This is a compact comparison hint, not authentication or a replacement for
/// the detailed per-mission manifest check.
pub fn code(protocol: u16, simulation: u32, content: &Content) -> u32 {
    let mut hash = 0x811c9dc5u32;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(0x01000193);
        }
    };
    feed(b"TORE-COMPAT-1\0");
    feed(&protocol.to_le_bytes());
    feed(&simulation.to_le_bytes());
    feed(&(content.items().len() as u32).to_le_bytes());
    for item in content.items() {
        let kind = match item.kind {
            tore_world::content::Kind::Aircraft => 0,
            tore_world::content::Kind::Theater => 1,
            tore_world::content::Kind::Weapon => 2,
            tore_world::content::Kind::Shared => 3,
        };
        feed(&[kind]);
        feed(&(item.key.len() as u32).to_le_bytes());
        feed(item.key.as_bytes());
        feed(&item.digest.to_le_bytes());
    }
    hash
}

pub fn current(content: &Content) -> u32 {
    code(crate::wire::PROTOCOL_VERSION, SIMULATION_VERSION, content)
}

/// Reads the selected import for command-line version and discovery commands.
/// No import is represented by the empty content set, labelled by the caller.
pub fn installed(data_dir: &std::path::Path) -> Result<u32, String> {
    let loaded = tore_import::load_with(data_dir, &|r| tore_import::check_markers(r))
        .map_err(|e| e.to_string())?;
    Ok(current(&Content::of(&loaded.resources)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_world::{
        content::{Item, Kind},
        resources::Manifest,
    };

    #[test]
    fn content_digests_change_the_code_and_input_order_does_not() {
        let a = Item::new(Kind::Aircraft, "F18.PT", Manifest::default());
        let b = Item::new(Kind::Shared, "shared", Manifest::default());
        let original = Content::from_items(vec![a.clone(), b.clone()]);
        let reversed = Content::from_items(vec![b.clone(), a.clone()]);
        assert_eq!(current(&original), current(&reversed));
        let mut changed = b;
        changed.digest ^= 1;
        assert_ne!(
            current(&original),
            current(&Content::from_items(vec![a, changed]))
        );
    }

    #[test]
    fn stable_inputs_and_fixed_uppercase_width() {
        let empty = Content::default();
        let value = code(68, 1, &empty);
        assert_eq!(value, 0xE06F6F61);
        assert_ne!(value, code(69, 1, &empty));
        assert_ne!(value, code(68, 2, &empty));
        let content =
            Content::from_items(vec![Item::new(Kind::Shared, "shared", Manifest::default())]);
        assert_ne!(value, code(68, 1, &content));
        let formatted = format!("{value:08X}");
        assert_eq!(formatted.len(), 8);
        assert!(
            formatted
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        );
    }
}
