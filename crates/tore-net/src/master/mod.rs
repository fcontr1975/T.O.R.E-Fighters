//! The master server's protocol: what a game, a dedicated server and the
//! Internet Lobby say to `tore-master`, and what it says back.
//!
//! The master keeps the list of listed games, answers the Internet Lobby,
//! measures how a router maps the game port, introduces a joining player to
//! a host and relays the traffic of a pair that cannot reach each other. Its
//! wire is its own, separate from the game's transport: every packet is
//! checked under the id `TORE-MASTER` and carries the master protocol's
//! version ([`MASTER_VERSION`]), so a game's transport never mistakes one for
//! its own. The format is
//! [`docs/formats/master-protocol.md`](../../../../docs/formats/master-protocol.md);
//! how the pieces fit is the architecture guide's "Master server and
//! connectivity".
//!
//! This module holds the wire (slice I1): every packet's encoder and bounded
//! decoder ([`packet`]), the common fields ([`candidate`]), the constants
//! both ends share, and the cookie key the master proves addresses with
//! ([`CookieKey`]). Like the transport, nothing there reads a clock or
//! touches a socket. A host's side of listing (slice I3) is the
//! [`Rendezvous`] state machine ([`rendezvous`]), the router that takes the
//! master's datagrams out of the game port's stream ([`routed`]), and the
//! host's own addresses and the master's lookup ([`local`]). A host's answer
//! to an introduction is [`meet`] (slice J2), and a joining player's side,
//! the mapping test, the introduction and the router of its socket, is
//! [`join`].
//!
//! ```
//! use tore_net::master::{Challenge, MasterPacket};
//!
//! let challenge = MasterPacket::Challenge(Challenge { nonce: 7, cookie: 42 });
//! let bytes = challenge.encode().unwrap();
//! assert_eq!(bytes.len(), 23);
//! assert_eq!(MasterPacket::decode(&bytes).unwrap(), challenge);
//! ```

use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::time::Duration;

use crate::COOKIE_SLOT;
use crate::entropy::{self, Entropy, Rng};

/// Declares an enum of wire codes: its variants, their codes, the field's
/// width in bits, `code` and a strict `from_code`.
macro_rules! codes {
    (
        $(#[$meta:meta])*
        $name:ident: $bits:literal bits {
            $($(#[$vmeta:meta])* $variant:ident = $code:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant = $code),+
        }

        impl $name {
            /// Every value, in code order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            /// The field's width on the wire, in bits.
            pub const BITS: u32 = $bits;

            /// The wire code.
            pub const fn code(self) -> u8 {
                self as u8
            }

            /// The value for a wire code; `None` for a code the protocol
            /// does not name, which makes the packet malformed.
            pub const fn from_code(code: u64) -> Option<Self> {
                match code {
                    $($code => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

pub mod browse;
pub mod candidate;
pub mod join;
pub mod local;
pub mod meet;
pub mod packet;
pub mod relay;
pub mod rendezvous;
pub mod routed;

pub use candidate::{Candidate, CandidateKind, MAX_CANDIDATES, MappingType, relay_likely};
pub use packet::{
    Browse, Build, Challenge, CloseReason, Details, Heartbeat, HeartbeatAck, Hint, Introduce,
    Introduction, IntroductionResult, Keep, Listed, ListingDetails, ListingSummary,
    MasterDecodeError, MasterEncodeError, MasterKind, MasterPacket, Meet, MeetAck, Page, PageEntry,
    Path, PortMapping, Probe, ProbeAnswer, ProbePort, Register, Relay, RelayClose, RelayFrame,
    RelayOffer, RelayOpen, RelayOpenAck, RelayRequest, RelayResult, Report, Role, UnknownListing,
    Unregister, Unsupported,
};
pub use relay::{Channels, HostRelays, RelayCounters, RelayFraming, RelayFramings};
pub use rendezvous::{
    HostListing, HostRendezvous, HostTally, ListingState, MasterSwitch, Rendezvous,
    RendezvousCounters, RendezvousEvent,
};
pub use routed::{Routed, channel_of, is_relayed, relayed_address};

/// The master protocol version this build speaks. It is separate from the
/// game's protocol version: one master serves every build of the game.
pub const MASTER_VERSION: u16 = 2;

/// The master protocol versions this build reads. A master answers a packet
/// of a version outside it with [`Unsupported`].
pub const SUPPORTED_VERSIONS: RangeInclusive<u16> = 1..=2;

/// The master's main UDP port: listings, browsing, introductions, the relay,
/// reports (a master setting).
pub const MASTER_PORT: u16 = 26901;

/// The master's second UDP port, for the second half of the mapping test
/// only (a master setting; 0 there turns the test off).
pub const PROBE_PORT: u16 = 26902;

/// The master a game talks to unless its options name another.
///
/// The public master John runs at jroverton.com (deployed 2026-10-05). A game
/// talks to it only to list a hosted game or to browse the Internet Lobby;
/// Direct Connection and a `tore-server` without `broadcast` never do.
pub const DEFAULT_MASTER: &str = "master.jroverton.com:26901";

/// A listed host sends a Heartbeat this often (the master's Listed says so;
/// this is its default).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// A listed host sends a Keep this often, so its router keeps the game
/// port's mapping (the master's default).
pub const KEEP_INTERVAL: Duration = Duration::from_secs(15);

/// A listing not heard from for this long is dropped (the master's default).
pub const LISTING_EXPIRY: Duration = Duration::from_secs(90);

/// A host sends one more Heartbeat this long after its lobby changes, and no
/// two change heartbeats closer together.
pub const CHANGE_HEARTBEAT_DELAY: Duration = Duration::from_secs(5);

/// No answer to anything for this long means the master is silent.
pub const MASTER_SILENT: Duration = Duration::from_secs(10);

/// Unregister and Relay close are sent this many times.
pub const GOODBYE_COPIES: u32 = 3;

/// A host repeats the mapping test this often while it is listed.
pub const MAPPING_TEST_INTERVAL: Duration = Duration::from_secs(600);

/// The master forgets an introduction after this long.
pub const INTRODUCTION_LIFETIME: Duration = Duration::from_secs(30);

/// The master repeats a Meet or a Relay open this often until it is
/// acknowledged, [`MEET_TRIES`] times at most.
pub const MEET_RETRY: Duration = Duration::from_millis(250);

/// How many times the master sends a Meet or a Relay open.
pub const MEET_TRIES: u32 = 3;

/// A host punches each of a player's addresses this many times per Meet.
pub const PUNCH_COUNT: u32 = 5;

/// The gap between a host's punches to one address.
pub const PUNCH_INTERVAL: Duration = Duration::from_millis(200);

/// A joining player asks for the relay after this long without an answer
/// from any of the host's addresses.
pub const RACE_BEFORE_RELAY: Duration = Duration::from_secs(3);

/// A whole join through the master gives up after this long, the relay's
/// part included.
pub const JOIN_GIVE_UP: Duration = Duration::from_secs(15);

/// The master closes a relay channel with no frame either way for this long.
pub const RELAY_IDLE: Duration = Duration::from_secs(30);

/// The master stops counting matching listings for a Browse at this many.
pub const MAX_BROWSE_MATCHES: u16 = 200;

/// The master's key for the stateless cookie that proves a sender's address
/// ("Proving an address").
///
/// The same keyed hash as the game's handshake cookie: a hash of the
/// sender's address and port, its nonce and the current 10-second time slot
/// ([`COOKIE_SLOT`]), keyed from the system's entropy when the master starts.
/// The current and the previous slot are accepted. *Agent decision:* the
/// transport's own key stays private to it; the master gets this wrapper of
/// it, which also does the slot arithmetic, so both use one hash and the
/// transport's internals stay where they are.
#[derive(Debug, Clone)]
pub struct CookieKey(entropy::CookieKey);

impl CookieKey {
    /// A key drawn from `entropy`: [`Entropy::System`] on a real network,
    /// [`Entropy::Seeded`] for tests and the simulator.
    pub fn new(entropy: Entropy) -> Self {
        let mut rng = Rng::new(entropy);
        Self(entropy::CookieKey::new(entropy, &mut rng))
    }

    /// The 10-second slot `now` falls in.
    pub fn slot(now: Duration) -> u64 {
        now.as_secs() / COOKIE_SLOT.as_secs()
    }

    /// The cookie for a sender's address and nonce at `now`. Never 0, which
    /// a request uses for "no cookie yet".
    pub fn cookie(&self, from: SocketAddr, nonce: u64, now: Duration) -> u64 {
        self.at_slot(from, nonce, Self::slot(now))
    }

    /// True when `cookie` is the one for `from` and `nonce` in the current or
    /// the previous slot. A cookie of 0 is never good.
    pub fn check(&self, from: SocketAddr, nonce: u64, cookie: u64, now: Duration) -> bool {
        let slot = Self::slot(now);
        cookie != 0
            && [slot, slot.wrapping_sub(1)]
                .iter()
                .any(|&s| self.at_slot(from, nonce, s) == cookie)
    }

    fn at_slot(&self, from: SocketAddr, nonce: u64, slot: u64) -> u64 {
        // A hash of 0 would read as "no cookie"; 1 stands in for it.
        self.0.cookie(from, nonce, slot).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_is_good_for_its_slot_and_the_next_only() {
        let key = CookieKey::new(Entropy::Seeded(5));
        let from: SocketAddr = "198.51.100.7:26900".parse().unwrap();
        let other: SocketAddr = "198.51.100.7:26901".parse().unwrap();
        let at = Duration::from_secs(1_000);
        let cookie = key.cookie(from, 9, at);
        assert_ne!(cookie, 0);
        assert!(key.check(from, 9, cookie, at));
        assert!(key.check(from, 9, cookie, at + COOKIE_SLOT));
        assert!(!key.check(from, 9, cookie, at + COOKIE_SLOT * 2));
        assert!(!key.check(other, 9, cookie, at));
        assert!(!key.check(from, 10, cookie, at));
        assert!(!key.check(from, 9, 0, at));
        // The same seed gives the same key; the system's is its own.
        assert_eq!(
            CookieKey::new(Entropy::Seeded(5)).cookie(from, 9, at),
            cookie
        );
        let system = CookieKey::new(Entropy::System);
        assert!(system.check(from, 9, system.cookie(from, 9, at), at));
    }

    #[test]
    fn the_constants_agree_with_the_spec() {
        assert!(SUPPORTED_VERSIONS.contains(&MASTER_VERSION));
        assert!(DEFAULT_MASTER.ends_with(&format!(":{MASTER_PORT}")));
        assert!(KEEP_INTERVAL < HEARTBEAT_INTERVAL && HEARTBEAT_INTERVAL * 3 == LISTING_EXPIRY);
        assert!(RACE_BEFORE_RELAY < JOIN_GIVE_UP);
    }
}
