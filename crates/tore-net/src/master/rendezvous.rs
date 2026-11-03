//! A host's conversation with the master (slice I3): listing a game from its
//! game port ("Listing a game" in the architecture guide and the master
//! protocol).
//!
//! [`Rendezvous`] is a state machine like the transport's: it never reads a
//! clock or touches a socket. The host's loop reads its socket through
//! [`Rendezvous::over`], which takes the master's datagrams out of the game
//! port's stream ([`Routed`]), calls [`Rendezvous::update`] for its timers
//! and sends what [`Rendezvous::transmit`] gives on the same socket, so the
//! master sees the outside address of the very socket players join.
//! [`HostListing`] adds what needs the real world: the lookup of the
//! master's name on a thread, and the host's own addresses.
//!
//! What it does, with the master protocol's numbers:
//!
//! 1. With the master's address known it sends the mapping test's two
//!    Probes and a Register, at the same moment (*agent decision:* the
//!    listing does not wait for the test, whose result only the Report uses
//!    in stage I). A Challenge is answered with the Register again, carrying
//!    the cookie; Listed makes the game listed.
//! 2. A Heartbeat with the summary every heartbeat interval (30 seconds, or
//!    what Listed says), and one more when the summary changes, at most one
//!    every 5 seconds; a Keep when nothing else has gone out for the keep
//!    interval (15 seconds). No two of them closer than 2 seconds, the
//!    master's limit per listing (agent decision).
//! 3. An Unknown listing means register again at once: a restarted master
//!    lists the game again within one exchange.
//! 4. A request (Register or Heartbeat) unanswered is sent again every 3
//!    seconds (agent decision); no answer at all for 10 seconds means the
//!    master is silent, and the request is then repeated after 2, 4, 8 and
//!    up to 15 seconds, the next of the master's addresses each time, round
//!    and back to the first ("Finding the master again" below).
//! 5. Unlisting, or stopping, sends Unregister three times at once (*agent
//!    decision:* at once, since the socket may close right after).
//!
//! 6. A Meet (slice J2) is answered with punches to the player's addresses
//!    from the game port and a Meet ack ([`meet`](super::meet)).
//! 7. A Relay open (slice J3) is acknowledged with the listing's token, and
//!    the channel's frames reach the transport as datagrams from its relayed
//!    address ([`relay`](super::relay)); the master's Relay close, or no
//!    frame for a minute, closes it here.
//! 8. A listing follows a migrated mission (stage K, slice K8): the host
//!    exports its [`ListingPart`] (the master, the listing's id and token,
//!    the relay channels and their keys) for its standbys, the game that
//!    takes over resumes from it ([`Rendezvous::resume`]) and heartbeats
//!    with the token from its own port, which moves the listing and its
//!    channels there, and an old host that learns another hosts now lets
//!    the listing go without a word ([`Rendezvous::release`]).
//!
//! **Finding the master again** (0.2.2, agent decisions after two public
//! servers stayed unlisted on 2026-10-23: a silence moved them to the
//! master's IPv6 address on a host with no IPv6 route, and nothing ever
//! moved them back). The listing always comes back on its own once any of
//! the master's addresses answers:
//!
//! - Every request to a silent master goes to the next of its addresses,
//!   round and back to the first, so no dead address holds the listing.
//! - A send that fails (no route to the network or the host, no such local
//!   address, an address family the socket lacks, or any other error the
//!   socket reports) moves the listing to the next address at once, if one
//!   has not failed too, and asks there without waiting.
//! - Addresses the host cannot use come last: an address of a family the
//!   game port's socket cannot send to ([`Datagrams::can_send_to`]), then
//!   one the system has no route to ([`HostListing`] asks the system at
//!   each lookup), then one a send failed to since the last lookup. An
//!   IPv4-only socket never picks an IPv6 address while an IPv4 one exists.
//! - A lookup that gives the same addresses keeps the one in use only if
//!   it is usable; else the listing moves to the first usable one. While the
//!   master is silent its name is looked up again every minute.
//! - An answer from any of the master's addresses makes that address the one
//!   in use, and the longest wait between requests to a silent master is 15
//!   seconds (a Keep's interval, so a silent host sends no more than a
//!   listed one): a master that comes back lists the game again within one
//!   wait, and a restarted one within the Unknown listing's exchange.
//! - Each change of address is one [`RendezvousEvent::MasterAddress`], and
//!   each address a send fails to without a change one
//!   [`RendezvousEvent::SendFailed`] per lookup, which the host's log shows.
//!
//! The summary is the host's discovery answer without its nonce. The host's
//! loop offers it through [`Rendezvous::wants_summary`] and
//! [`Rendezvous::set_summary`], which the rendezvous asks for at most once a
//! second (agent decision), so building it costs nothing on most turns.

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use super::candidate::{Candidate, CandidateKind, MAX_CANDIDATES, MappingType, canonical};
use super::local::{
    MasterLookup, host_candidates, own_address_toward, parse_master, probe_address,
};
use super::meet::{MeetOutcome, Meets};
use super::packet::{
    Build, Heartbeat, Keep, ListingSummary, MasterPacket, MeetAck, Path, PortMapping, Probe,
    ProbePort, Register, Report, Role, Unregister,
};
use super::relay::{Channels, HostRelays, RelayCounters};
use super::routed::{MasterSide, Routed, is_relayed};
use super::{
    CHANGE_HEARTBEAT_DELAY, GOODBYE_COPIES, HEARTBEAT_INTERVAL, KEEP_INTERVAL,
    MAPPING_TEST_INTERVAL, MASTER_SILENT,
};
use crate::entropy::{Entropy, Rng};
use crate::packet::{Packet, Punch};
use crate::{Datagrams, LINK_ADDRESS, Transmit};

/// An unanswered Register or Heartbeat is sent again this often while the
/// master is not yet silent (agent decision).
pub const REQUEST_RETRY: Duration = Duration::from_secs(3);
/// The least time between two of a listing's Heartbeats and Keeps: the
/// master drops the second of two closer than this (agent decision, the
/// master protocol's limit).
pub const LISTING_GAP: Duration = Duration::from_secs(2);
/// The rendezvous asks the host's loop for the summary at most this often
/// (agent decision).
pub const SUMMARY_LOOK: Duration = Duration::from_secs(1);
/// The first wait after the master falls silent; each next one doubles.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(2);
/// The longest wait between requests to a silent master: a Keep's interval,
/// so a silent host sends no more than a listed one (agent decision, 0.2.2;
/// 60 seconds before).
pub const MAX_BACKOFF: Duration = Duration::from_secs(15);
/// The longest wait before looking up a master's name that could not be
/// looked up.
pub const MAX_LOOKUP_BACKOFF: Duration = Duration::from_secs(60);
/// The master's name is looked up again this often while listing.
pub const LOOKUP_INTERVAL: Duration = Duration::from_secs(600);
/// The master's name is looked up again this often while the master is
/// silent (agent decision, 0.2.2).
pub const SILENT_LOOKUP_INTERVAL: Duration = Duration::from_secs(60);
/// Probes of one mapping test are sent this many times at most while no
/// answer comes, [`REQUEST_RETRY`] apart (agent decision).
const PROBE_TRIES: u32 = 3;

/// What a host's rendezvous is set up with.
#[derive(Debug, Clone)]
pub struct HostRendezvous {
    /// The host's build, which the master filters by.
    pub build: Build,
    /// A dedicated server (`tore-server`), not a game that hosts.
    pub dedicated: bool,
    /// The anonymous install id when telemetry is on, else `None`: then the
    /// Register carries none and no Report is sent.
    pub install_id: Option<u64>,
    /// The host's platform code.
    pub platform: u8,
    /// The nonces' source: [`Entropy::System`] on a real network.
    pub entropy: Entropy,
}

/// Where the listing stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingState {
    /// Not asked to list.
    Off,
    /// Asked to list; the master's address is not known yet.
    FindingMaster,
    /// Registering: no Listed yet.
    Registering,
    /// On the master's list.
    Listed {
        /// The public id browsers name the listing by.
        listing_id: u64,
        /// The host's address as the master saw it.
        seen: SocketAddr,
    },
    /// No answer from the master for 10 seconds; asking again with back-off.
    Silent,
    /// The master does not speak this build's master protocol: its text.
    Refused(String),
}

/// What the rendezvous tells the host's loop, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendezvousEvent {
    /// The game is listed, first or again (after a master restart or a
    /// silence).
    Listed {
        /// The public id.
        listing_id: u64,
        /// The host's address as the master saw it.
        seen: SocketAddr,
    },
    /// The master saw the host at another address (the router gave the port
    /// a new outside address).
    SeenChanged(SocketAddr),
    /// The game is no longer listed, as asked.
    Unlisted,
    /// The master has not answered for 10 seconds.
    MasterSilent,
    /// The master's name could not be looked up, with why.
    LookupFailed(String),
    /// The master does not speak this build's master protocol: its text.
    Refused(String),
    /// The mapping test's result: how the host's router maps the game port.
    MappingTested(MappingType),
    /// The listing goes to another of the master's addresses (0.2.2).
    MasterAddress {
        /// The address it went to before.
        from: SocketAddr,
        /// The address it goes to now.
        to: SocketAddr,
        /// Why it moved.
        why: MasterSwitch,
    },
    /// A send to one of the master's addresses failed and the listing had
    /// no other address to move to (0.2.2): once per address until the
    /// master's name is looked up again.
    SendFailed {
        /// The master's address.
        to: SocketAddr,
        /// What the system said.
        error: String,
    },
}

/// Why a listing moved to another of the master's addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterSwitch {
    /// The master did not answer: the next request tries the next address.
    NoAnswer,
    /// A send to the address failed: what the system said.
    SendFailed(String),
    /// The master answered from the new address.
    Answered,
    /// A new lookup of the master's name left the old address unusable or
    /// gave it no more.
    Lookup,
}

impl MasterSwitch {
    /// Why, in a few words for a log line.
    pub fn text(&self) -> String {
        match self {
            Self::NoAnswer => "no answer".into(),
            Self::SendFailed(error) => format!("cannot send there: {error}"),
            Self::Answered => "the master answered from it".into(),
            Self::Lookup => "a new lookup of the master's name".into(),
        }
    }
}

/// What the rendezvous counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RendezvousCounters {
    /// Master packets sent.
    pub sent: u64,
    /// Datagrams from the master's addresses taken out of the stream.
    pub received: u64,
    /// Of those, ones that did not decode.
    pub malformed: u64,
    /// Good packets this side does not take, or not now (an old nonce, a
    /// token not ours).
    pub unexpected: u64,
    /// Relay opens acknowledged (slice J3), repeats included.
    pub relay_opens: u64,
    /// Relay closes from the master that closed a channel.
    pub relay_closes: u64,
    /// Meets acted on: punched and acknowledged (slice J2).
    pub meets: u64,
    /// Meets repeated for an introduction already met: acknowledged again.
    pub meets_again: u64,
    /// Meets dropped: over 10 a second, or while the game is not listed.
    pub meets_dropped: u64,
    /// Punches sent.
    pub punches: u64,
    /// Datagrams from a real socket claiming a relayed address, dropped.
    pub relayed_claims: u64,
    /// Sends by the transport to a relayed address with no open channel,
    /// dropped.
    pub relay_sends_dropped: u64,
}

/// The listing the master gave.
#[derive(Debug, Clone, Copy)]
struct Listing {
    listing_id: u64,
    token: u64,
    seen: SocketAddr,
}

/// One relay channel in a [`ListingPart`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartChannel {
    /// The channel's number: its relayed address on both ends
    /// ([`super::relayed_address`]).
    pub channel: u32,
    /// Its key.
    pub key: u32,
    /// The master's address its frames come from.
    pub master: SocketAddr,
}

/// A host's listing as the game that takes its mission over carries it on
/// (stage K, slice K8; the architecture guide's "Reaching the new host" and
/// the master protocol's "Moving a listing"): the session's state part
/// *listing*, which the standby stream carries as bytes
/// ([`ListingPart::encode`]) that `Host` never reads. The hosting thread
/// takes it from [`HostListing::part`] whenever [`HostListing::part_version`]
/// changes, and the new host's thread resumes from it
/// ([`HostListing::resume`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingPart {
    /// The master as the old host was given it (`HOST:PORT`), so the new
    /// host looks up the same master whatever its own options say.
    pub master_name: String,
    /// The master's main address the listing is on.
    pub master: SocketAddr,
    /// The public id: browsers keep seeing the same game.
    pub listing_id: u64,
    /// The secret the new host proves itself with.
    pub token: u64,
    /// The heartbeat interval the master set, in seconds.
    pub heartbeat_secs: u16,
    /// The keep interval the master set, in seconds.
    pub keep_secs: u16,
    /// Every relay channel open to the host, with its key.
    pub channels: Vec<PartChannel>,
}

/// A listing part's bytes that do not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadListingPart;

impl std::fmt::Display for BadListingPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the listing part does not decode")
    }
}

impl std::error::Error for BadListingPart {}

/// The listing part's coding's own version, its first byte.
const PART_FORMAT: u8 = 1;
/// The longest master name a part carries.
const MAX_PART_NAME: usize = 255;

impl ListingPart {
    /// The part's bytes: its format (8), the master's name (a length, 8,
    /// then the bytes), the master's address, the listing id (64) and token
    /// (64), the two intervals (16 each), and the channels (a count, 8;
    /// each its number and key, 32 each, and its master's address). An
    /// address is its family (8: 4 or 6), its octets and its port (16);
    /// numbers are little-endian. *Agent decision:* a plain byte coding of
    /// its own, since `tore-net` has no checkpoint trait and the part is
    /// read by the same build only, as every state part is. A name longer
    /// than 255 bytes, or more than 255 channels, is cut.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.channels.len() * 27);
        out.push(PART_FORMAT);
        let mut name = self.master_name.as_str();
        while name.len() > MAX_PART_NAME {
            let mut end = MAX_PART_NAME;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name = &name[..end];
        }
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        put_address(&mut out, self.master);
        out.extend_from_slice(&self.listing_id.to_le_bytes());
        out.extend_from_slice(&self.token.to_le_bytes());
        out.extend_from_slice(&self.heartbeat_secs.to_le_bytes());
        out.extend_from_slice(&self.keep_secs.to_le_bytes());
        let channels = &self.channels[..self.channels.len().min(255)];
        out.push(channels.len() as u8);
        for channel in channels {
            out.extend_from_slice(&channel.channel.to_le_bytes());
            out.extend_from_slice(&channel.key.to_le_bytes());
            put_address(&mut out, channel.master);
        }
        out
    }

    /// The part from its bytes; strict, so trailing bytes, an unknown
    /// format or family, or a name that is not UTF-8 refuse it.
    pub fn decode(bytes: &[u8]) -> Result<Self, BadListingPart> {
        let mut reader = PartReader { bytes };
        if reader.take(1)?[0] != PART_FORMAT {
            return Err(BadListingPart);
        }
        let name_len = usize::from(reader.take(1)?[0]);
        let master_name = std::str::from_utf8(reader.take(name_len)?)
            .map_err(|_| BadListingPart)?
            .to_owned();
        let master = reader.address()?;
        let listing_id = reader.u64()?;
        let token = reader.u64()?;
        let heartbeat_secs = reader.u16()?;
        let keep_secs = reader.u16()?;
        let count = usize::from(reader.take(1)?[0]);
        let mut channels = Vec::with_capacity(count);
        for _ in 0..count {
            channels.push(PartChannel {
                channel: reader.u32()?,
                key: reader.u32()?,
                master: reader.address()?,
            });
        }
        if !reader.bytes.is_empty() {
            return Err(BadListingPart);
        }
        Ok(Self {
            master_name,
            master,
            listing_id,
            token,
            heartbeat_secs,
            keep_secs,
            channels,
        })
    }
}

fn put_address(out: &mut Vec<u8>, address: SocketAddr) {
    match canonical(address) {
        SocketAddr::V4(v4) => {
            out.push(4);
            out.extend_from_slice(&v4.ip().octets());
        }
        SocketAddr::V6(v6) => {
            out.push(6);
            out.extend_from_slice(&v6.ip().octets());
        }
    }
    out.extend_from_slice(&address.port().to_le_bytes());
}

/// Reads a listing part's bytes in order.
struct PartReader<'a> {
    bytes: &'a [u8],
}

impl<'a> PartReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BadListingPart> {
        if self.bytes.len() < n {
            return Err(BadListingPart);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, BadListingPart> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| BadListingPart)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, BadListingPart> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| BadListingPart)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, BadListingPart> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| BadListingPart)?,
        ))
    }

    fn address(&mut self) -> Result<SocketAddr, BadListingPart> {
        let ip = match self.take(1)?[0] {
            4 => {
                let octets: [u8; 4] = self.take(4)?.try_into().map_err(|_| BadListingPart)?;
                IpAddr::from(octets)
            }
            6 => {
                let octets: [u8; 16] = self.take(16)?.try_into().map_err(|_| BadListingPart)?;
                let ip = std::net::Ipv6Addr::from(octets);
                // An IPv4-mapped address is written as IPv4.
                if ip.to_ipv4_mapped().is_some() {
                    return Err(BadListingPart);
                }
                IpAddr::V6(ip)
            }
            _ => return Err(BadListingPart),
        };
        Ok(SocketAddr::new(ip, self.u16()?))
    }
}

/// The mapping test under way or done.
#[derive(Debug, Clone)]
struct Mapping {
    nonce: u64,
    main: Option<SocketAddr>,
    second: Option<SocketAddr>,
    /// The last complete test's verdict.
    result: MappingType,
    /// When this test's first Probes went out.
    started: Duration,
    /// When Probes go out next.
    next: Duration,
    tries: u32,
    /// This test's verdict is in.
    done: bool,
}

impl Mapping {
    fn new(result: MappingType, next: Duration) -> Self {
        Self {
            nonce: 0,
            main: None,
            second: None,
            result,
            started: next,
            next,
            tries: 0,
            done: false,
        }
    }
}

/// A host's side of the master: see the module documentation.
#[derive(Debug)]
pub struct Rendezvous {
    config: HostRendezvous,
    rng: Rng,
    /// The master's main addresses, canonical, IPv4 first.
    masters: Vec<SocketAddr>,
    /// The one listed with now.
    current: usize,
    candidates: Vec<Candidate>,
    /// The candidates [`Rendezvous::set_masters`] was given, as given.
    own: Vec<Candidate>,
    /// The outside address a router's port mapping gave, kept apart from
    /// `own` so a new set of the host's own addresses does not lose it
    /// ([`Rendezvous::set_mapped`]).
    mapped: Option<SocketAddr>,
    wanted: bool,
    nonce: u64,
    listing: Option<Listing>,
    heartbeat_interval: Duration,
    keep_interval: Duration,
    summary: Option<ListingSummary>,
    /// Raised when the summary changes.
    change: u16,
    /// The summary changed since the last Register or Heartbeat.
    changed: bool,
    looked: Option<Duration>,
    last_heartbeat: Duration,
    last_listing_send: Option<Duration>,
    /// When the oldest unanswered request went out.
    asked_since: Option<Duration>,
    /// When the request goes out (again).
    next_ask: Duration,
    silent: bool,
    backoff: Duration,
    /// The master's addresses the system has no route to, as the last
    /// lookup found ([`Rendezvous::set_masters_routed`]).
    no_route: BTreeSet<SocketAddr>,
    /// The master's addresses the game port's socket cannot send to (its
    /// family is missing), as the last transmit found.
    no_family: BTreeSet<SocketAddr>,
    /// The master's addresses a send failed to since the last lookup.
    failed: BTreeSet<SocketAddr>,
    /// Sends that failed, by the master's address, for the next update.
    unsent: Vec<(SocketAddr, String)>,
    /// The address in use was chosen by something other than a request to
    /// a silent master (a failed send, an answer, a lookup), so the next
    /// request goes there before the round moves on.
    untried: bool,
    refused: Option<String>,
    mapping: Mapping,
    meets: Meets,
    /// The relay's channels to this host (slice J3).
    pub(super) relays: HostRelays,
    /// A listing resumed from another host's part whose move the master has
    /// not yet acknowledged (slice K8).
    moving: bool,
    /// Raised whenever the listing part changes, but for its channels,
    /// which the channels' own counts follow ([`Rendezvous::part_version`]).
    part_changes: u64,
    out: VecDeque<Transmit>,
    events: VecDeque<RendezvousEvent>,
    /// What the rendezvous counted.
    pub counters: RendezvousCounters,
}

impl Rendezvous {
    /// A host's rendezvous, not listing until [`Rendezvous::set_listed`].
    pub fn host(mut config: HostRendezvous, now: Duration) -> Self {
        // An install id of 0 means none.
        config.install_id = config.install_id.filter(|id| *id != 0);
        let mut rng = Rng::new(config.entropy);
        let nonce = rng.next_u64();
        Self {
            config,
            rng,
            masters: Vec::new(),
            current: 0,
            candidates: Vec::new(),
            own: Vec::new(),
            mapped: None,
            wanted: false,
            nonce,
            listing: None,
            heartbeat_interval: HEARTBEAT_INTERVAL,
            keep_interval: KEEP_INTERVAL,
            summary: None,
            change: 0,
            changed: false,
            looked: None,
            last_heartbeat: now,
            last_listing_send: None,
            asked_since: None,
            next_ask: now,
            silent: false,
            backoff: FIRST_BACKOFF,
            no_route: BTreeSet::new(),
            no_family: BTreeSet::new(),
            failed: BTreeSet::new(),
            unsent: Vec::new(),
            untried: false,
            refused: None,
            mapping: Mapping::new(MappingType::Unknown, now),
            meets: Meets::default(),
            relays: HostRelays::default(),
            moving: false,
            part_changes: 0,
            out: VecDeque::new(),
            events: VecDeque::new(),
            counters: RendezvousCounters::default(),
        }
    }

    /// A host's rendezvous that carries on another host's listing (stage K,
    /// slice K8): the game took a migrated mission over and was handed the
    /// old host's [`ListingPart`]. It never registers. Its first update
    /// sends a Heartbeat with the listing's token from this game's own port
    /// to the master in the part, which moves the listing there (and, from
    /// stage K, the listing's relay channels with it); the channels in the
    /// part are open here from the start, so a relayed player's frames reach
    /// this host as soon as the master forwards them. The state reads
    /// [`ListingState::Registering`] until the master acknowledges, then
    /// [`RendezvousEvent::Listed`] comes with the address the master saw.
    ///
    /// *Agent decisions:* while the move is unacknowledged the Heartbeat is
    /// sent again every [`REQUEST_RETRY`], and the master counts as silent
    /// only after a listing's expiry ([`super::LISTING_EXPIRY`], 90 seconds)
    /// rather than 10 seconds, since a master moves a listing at most once a
    /// minute and drops a second move within it unanswered: a second
    /// migration within the minute is listed at the new host within 3
    /// seconds of the minute's end. An Unknown listing (the master restarted,
    /// or the listing expired) lists the game afresh, as for any listing.
    /// The mapping test runs again from this port.
    pub fn resume(
        config: HostRendezvous,
        part: &ListingPart,
        candidates: Vec<Candidate>,
        now: Duration,
    ) -> Self {
        let mut rendezvous = Self::host(config, now);
        rendezvous.set_masters(vec![part.master], candidates, now);
        rendezvous.wanted = true;
        rendezvous.listing = Some(Listing {
            listing_id: part.listing_id,
            token: part.token,
            // Until the master's acknowledgement says where it sees this
            // game; nobody reads it before then (the state is Registering).
            seen: canonical(part.master),
        });
        rendezvous.moving = true;
        rendezvous.last_heartbeat = now;
        let seconds = |secs: u16, default: Duration| match secs {
            0 => default,
            secs => Duration::from_secs(u64::from(secs)),
        };
        rendezvous.heartbeat_interval = seconds(part.heartbeat_secs, HEARTBEAT_INTERVAL);
        rendezvous.keep_interval = seconds(part.keep_secs, KEEP_INTERVAL);
        for channel in &part.channels {
            rendezvous
                .relays
                .channels
                .open(channel.channel, channel.key, channel.master, now);
        }
        rendezvous.part_changes += 1;
        rendezvous
    }

    /// The listing as another game would carry it on if this one's mission
    /// moved to it (stage K, slice K8): the master's address, the listing's
    /// id and token, the intervals and every open relay channel with its
    /// key. `None` while the game is not listed. The master's name is left
    /// empty: [`HostListing::part`] fills it in.
    pub fn listing_part(&self) -> Option<ListingPart> {
        let listing = self
            .listing
            .filter(|_| self.wanted && self.refused.is_none())?;
        let master = self.master()?;
        let secs = |d: Duration| u16::try_from(d.as_secs()).unwrap_or(u16::MAX);
        Some(ListingPart {
            master_name: String::new(),
            master,
            listing_id: listing.listing_id,
            token: listing.token,
            heartbeat_secs: secs(self.heartbeat_interval),
            keep_secs: secs(self.keep_interval),
            channels: self
                .relays
                .channels
                .list()
                .into_iter()
                .map(|(channel, key, master)| PartChannel {
                    channel,
                    key,
                    master,
                })
                .collect(),
        })
    }

    /// A number that changes whenever [`Rendezvous::listing_part`] may have:
    /// the hosting loop hands its `Host` a new part when it differs from the
    /// last one it saw, without building the part on every turn.
    pub fn part_version(&self) -> u64 {
        let counters = self.relays.channels.counters;
        self.part_changes
            .wrapping_add(counters.opened)
            .wrapping_add(counters.closed)
    }

    /// Lets the listing go without a word (stage K, slice K8): another game
    /// hosts the mission now and carries the listing on, so no Unregister
    /// and no Relay close go out, nothing queued is sent, and the relay's
    /// channels are forgotten. Returns the part as it stood. The game can be
    /// listed again later with [`Rendezvous::set_listed`], as a new listing.
    pub fn release(&mut self) -> Option<ListingPart> {
        let part = self.listing_part();
        self.wanted = false;
        self.listing = None;
        self.moving = false;
        self.asked_since = None;
        self.silent = false;
        self.relays.channels.forget_all();
        self.meets = Meets::default();
        self.out.clear();
        self.part_changes += 1;
        self.events.push_back(RendezvousEvent::Unlisted);
        part
    }

    /// The relayed addresses of the relay's channels open to this host: one
    /// per relayed player (slice J3).
    pub fn relayed(&self) -> Vec<SocketAddr> {
        self.relays.channels.addresses()
    }

    /// What the relay's channels counted.
    pub fn relay_counters(&self) -> RelayCounters {
        self.relays.channels.counters
    }

    /// The host's connection at the relayed address `relayed` ended: its
    /// channel is closed, and the master told ([`GOODBYE_COPIES`] Relay
    /// closes). Nothing happens for an address with no channel. *Agent
    /// decision:* the host loops need not call it, since the master closes a
    /// channel idle for 30 seconds and the player's end closes its own.
    pub fn close_relayed(&mut self, relayed: SocketAddr) {
        for (to, packet) in self.relays.close_address(relayed) {
            self.send(to, &packet);
        }
    }

    /// The relay channels open now, as `(channel, key, master)`: what a
    /// host's loading thread frames its keepalives to relayed players with
    /// ([`crate::master::RelayFramings`], protocol 28).
    pub fn relay_channels(&self) -> Vec<(u32, u32, SocketAddr)> {
        self.relays.channels.list()
    }

    /// The socket as the transport should see it, for one receive or one
    /// transmit.
    pub fn over<'a, D: Datagrams + ?Sized>(
        &'a mut self,
        socket: &'a mut D,
        now: Duration,
    ) -> Routed<'a, D> {
        Routed {
            rendezvous: self,
            socket,
            now,
        }
    }

    /// The master's main addresses (IPv4 first, as a lookup gives them) and
    /// the host's own candidates, every address counted as one the system
    /// has a route to: see [`Rendezvous::set_masters_routed`].
    pub fn set_masters(
        &mut self,
        masters: Vec<SocketAddr>,
        candidates: Vec<Candidate>,
        now: Duration,
    ) {
        self.set_masters_routed(masters, candidates, |_| true, now);
    }

    /// The master's main addresses (IPv4 first, as a lookup gives them), the
    /// host's own candidates, and `routed`, which says whether the system
    /// has a route to an address: one it has none to is tried only when no
    /// other will do. The address in use is kept while it is among them and
    /// usable (a route, a socket of its family, no failed send since the
    /// last lookup); else the listing moves to the first usable address,
    /// keeping its listing (the same master), or, when the address in use
    /// is no longer among them, is made again there (the master moved). The
    /// marks of failed sends are then cleared, so a later round tries those
    /// addresses again.
    pub fn set_masters_routed(
        &mut self,
        masters: Vec<SocketAddr>,
        candidates: Vec<Candidate>,
        routed: impl Fn(SocketAddr) -> bool,
        now: Duration,
    ) {
        let mut unique: Vec<SocketAddr> = Vec::new();
        for master in masters.into_iter().map(canonical) {
            if !unique.contains(&master) {
                unique.push(master);
            }
        }
        let before = self.masters.get(self.current).copied();
        self.own = candidates;
        self.apply_mapped();
        self.no_route = unique.iter().copied().filter(|m| !routed(*m)).collect();
        self.no_family.retain(|m| unique.contains(m));
        self.masters = unique;
        let kept = before
            .and_then(|before| self.masters.iter().position(|m| *m == before))
            .filter(|&at| self.rank(self.masters[at]) == 0);
        self.current = kept
            .or_else(|| self.next_master(self.masters.len().saturating_sub(1), false))
            .unwrap_or(0);
        self.failed.clear();
        let after = self.masters.get(self.current).copied();
        if let Some(from) = before
            && after != Some(from)
        {
            if self.masters.contains(&from) {
                // The same master at another of its addresses: ask there now.
                self.last_listing_send = None;
            } else {
                self.listing = None;
                self.moving = false;
                self.asked_since = None;
            }
            self.next_ask = now;
            self.part_changes += 1;
            self.untried = true;
            if let Some(to) = after {
                self.events.push_back(RendezvousEvent::MasterAddress {
                    from,
                    to,
                    why: MasterSwitch::Lookup,
                });
            }
        }
        if self.masters.is_empty() {
            return;
        }
        if before.is_none() {
            self.next_ask = self.next_ask.max(now);
            self.mapping = Mapping::new(self.mapping.result, now);
        }
    }

    /// How usable one of the master's addresses is, best first: 0 usable, 1
    /// a send to it failed since the last lookup, 2 the system has no route
    /// to it, 3 the game port's socket cannot send to its family.
    fn rank(&self, address: SocketAddr) -> u8 {
        if self.no_family.contains(&address) {
            3
        } else if self.no_route.contains(&address) {
            2
        } else if self.failed.contains(&address) {
            1
        } else {
            0
        }
    }

    /// The master's address to try after the one at `after`, going round
    /// and back to the first: the first of the best rank, which is `after`
    /// itself when no other is as good. With `usable_only`, only a usable
    /// address other than `after`, or none.
    fn next_master(&self, after: usize, usable_only: bool) -> Option<usize> {
        let count = self.masters.len();
        let mut round = (1..=count).map(|step| (after + step) % count);
        if usable_only {
            round.find(|&at| at != after && self.rank(self.masters[at]) == 0)
        } else {
            round.min_by_key(|&at| self.rank(self.masters[at]))
        }
    }

    /// Moves the listing to the master's address at `to`, with one event;
    /// false when it is the address in use already.
    fn switch_master(&mut self, to: usize, why: MasterSwitch) -> bool {
        let (Some(&from), Some(&address)) = (self.masters.get(self.current), self.masters.get(to))
        else {
            return false;
        };
        if to == self.current {
            return false;
        }
        self.current = to;
        self.part_changes += 1;
        self.untried = true;
        self.events.push_back(RendezvousEvent::MasterAddress {
            from,
            to: address,
            why,
        });
        true
    }

    /// The master's main address `to` is one of its addresses at, either
    /// port; `None` for any other address.
    fn master_of(&self, to: SocketAddr) -> Option<SocketAddr> {
        let to = canonical(to);
        self.masters
            .iter()
            .copied()
            .find(|m| *m == to || probe_address(*m) == Some(to))
    }

    /// Tells the rendezvous that a datagram it gave ([`Rendezvous::poll_transmit`])
    /// could not be sent to `to`: one to the master's address in use moves
    /// the listing to the next usable address at the next update, and asks
    /// there at once. [`Rendezvous::transmit`] does this itself. Nothing
    /// happens for an address that is not the master's.
    pub fn send_failed(&mut self, to: SocketAddr, error: &io::Error) {
        if let Some(master) = self.master_of(to) {
            self.unsent.push((master, error.to_string()));
        }
    }

    /// Acts on the sends that failed since the last update: see the module
    /// documentation's "Finding the master again".
    fn fail_over(&mut self, now: Duration) {
        for (address, error) in std::mem::take(&mut self.unsent) {
            let fresh = self.failed.insert(address);
            let moved = self.master() == Some(address)
                && self.next_master(self.current, true).is_some_and(|next| {
                    self.switch_master(next, MasterSwitch::SendFailed(error.clone()))
                });
            if moved {
                // The failed datagram never reached the master: ask anew.
                self.next_ask = now;
                self.last_listing_send = None;
            } else if fresh {
                self.events
                    .push_back(RendezvousEvent::SendFailed { to: address, error });
            }
        }
    }

    /// The master answered from `from`: that address works, so it is the
    /// one in use from now on.
    fn answered_from(&mut self, from: SocketAddr) {
        self.failed.remove(&from);
        if let Some(at) = self.masters.iter().position(|m| *m == from) {
            self.switch_master(at, MasterSwitch::Answered);
        }
    }

    /// Tells the listing the outside address a router's port mapping gave
    /// (slice J4b), or `None` when there is none any more (the mapping was
    /// removed or lost). It goes to the master as a Mapped candidate beside
    /// the host's own ([`Rendezvous::set_masters`] keeps it when the own
    /// ones change). A change raises the change counter and sends a
    /// Heartbeat within 5 seconds, as a summary change does; the same
    /// address again changes nothing. *Agent decision:* only the IPv4
    /// mapping is a Mapped candidate, since an IPv6 address is already the
    /// GlobalIpv6 candidate.
    pub fn set_mapped(&mut self, mapped: Option<SocketAddr>, _now: Duration) {
        let mapped = mapped.map(canonical);
        if mapped == self.mapped {
            return;
        }
        self.mapped = mapped;
        self.apply_mapped();
        if self.listing.is_some() {
            self.change = self.change.wrapping_add(1);
            self.changed = true;
        }
    }

    /// The candidates the listing carries.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// The candidates the listing carries: the host's own as given, and the
    /// router's mapped address in place of any Mapped one among them, never
    /// beyond [`MAX_CANDIDATES`].
    fn apply_mapped(&mut self) {
        self.candidates = self.own.clone();
        if let Some(address) = self.mapped {
            self.candidates.retain(|c| c.kind != CandidateKind::Mapped);
            if self.candidates.len() < MAX_CANDIDATES {
                self.candidates
                    .push(Candidate::new(CandidateKind::Mapped, address));
            }
        }
    }

    /// The master's main address the listing goes to, once known.
    pub fn master(&self) -> Option<SocketAddr> {
        self.masters.get(self.current).copied()
    }

    /// True when `from` is one of the master's addresses, either port.
    pub fn is_master(&self, from: SocketAddr) -> bool {
        let from = canonical(from);
        self.masters
            .iter()
            .any(|m| *m == from || probe_address(*m) == Some(from))
    }

    /// Lists the game, or takes it off the list (three Unregisters).
    pub fn set_listed(&mut self, listed: bool, now: Duration) {
        if listed == self.wanted {
            return;
        }
        self.wanted = listed;
        if listed {
            self.nonce = self.rng.next_u64();
            self.asked_since = None;
            self.next_ask = now;
            self.silent = false;
            self.backoff = FIRST_BACKOFF;
            self.mapping = Mapping::new(self.mapping.result, now);
        } else {
            self.unregister();
            self.asked_since = None;
            self.events.push_back(RendezvousEvent::Unlisted);
        }
    }

    /// Whether the game is to be listed.
    pub fn listed_wanted(&self) -> bool {
        self.wanted
    }

    /// Takes the game off the list, as the host stops: three Unregisters to
    /// send before the socket closes.
    pub fn stop(&mut self, now: Duration) {
        self.set_listed(false, now);
    }

    /// Where the listing stands.
    pub fn state(&self) -> ListingState {
        if let Some(text) = &self.refused {
            return ListingState::Refused(text.clone());
        }
        if !self.wanted {
            return ListingState::Off;
        }
        if self.masters.is_empty() {
            return ListingState::FindingMaster;
        }
        if self.silent {
            return ListingState::Silent;
        }
        match self.listing {
            // A listing carried on from another host reads as registering
            // until the master has moved it here (slice K8).
            Some(listing) if !self.moving => ListingState::Listed {
                listing_id: listing.listing_id,
                seen: listing.seen,
            },
            _ => ListingState::Registering,
        }
    }

    /// The mapping test's result so far.
    pub fn mapping(&self) -> MappingType {
        self.mapping.result
    }

    /// True when the rendezvous wants a fresh summary from the host's loop
    /// ([`Rendezvous::set_summary`]): at most once a second while listing.
    pub fn wants_summary(&self, now: Duration) -> bool {
        self.wanted && self.looked.is_none_or(|at| now >= at + SUMMARY_LOOK)
    }

    /// The host's summary: its discovery answer without the nonce. A change
    /// raises the change counter and sends a Heartbeat within 5 seconds.
    pub fn set_summary(&mut self, now: Duration, summary: ListingSummary) {
        self.looked = Some(now);
        if self.summary.as_ref() != Some(&summary) {
            if self.summary.is_some() {
                self.change = self.change.wrapping_add(1);
                self.changed = true;
            }
            self.summary = Some(summary);
        }
    }

    /// Queues a telemetry Report for the master. Nothing is sent without an
    /// install id (telemetry off) or a master address.
    pub fn report(&mut self, mut report: Report) {
        let (Some(install_id), Some(master)) = (self.config.install_id, self.master()) else {
            return;
        };
        report.install_id = install_id;
        self.send(master, &MasterPacket::Report(report));
    }

    /// The next event, oldest first.
    pub fn poll_event(&mut self) -> Option<RendezvousEvent> {
        self.events.pop_front()
    }

    /// The next master datagram to send.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.out.pop_front()
    }

    /// Sends every queued datagram on `socket`, carrying on past failures.
    /// A send to one of the master's addresses that fails, or that the
    /// socket has no socket of its family for, is the rendezvous's to act on
    /// ([`Rendezvous::send_failed`]); the first other error is returned.
    pub fn transmit<D: Datagrams + ?Sized>(&mut self, socket: &mut D) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        for &master in &self.masters {
            if socket.can_send_to(master) {
                self.no_family.remove(&master);
            } else {
                self.no_family.insert(master);
            }
        }
        let mut first_error = None;
        while let Some(transmit) = self.out.pop_front() {
            let master = self.master_of(transmit.to);
            if let Some(master) = master
                && self.no_family.contains(&master)
            {
                self.unsent.push((
                    master,
                    "the game port has no socket of that address family".into(),
                ));
                continue;
            }
            if let Err(error) = socket.send_datagram(transmit.to, &transmit.datagram) {
                match master {
                    Some(master) => self.unsent.push((master, error.to_string())),
                    None => {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// A datagram from one of the master's addresses.
    pub fn receive(&mut self, now: Duration, from: SocketAddr, datagram: &[u8]) {
        self.counters.received += 1;
        let Ok(packet) = MasterPacket::decode(datagram) else {
            self.counters.malformed += 1;
            return;
        };
        let from = canonical(from);
        match packet {
            MasterPacket::Challenge(challenge) => {
                if challenge.nonce != self.nonce || self.listing.is_some() || !self.wanted {
                    self.counters.unexpected += 1;
                    return;
                }
                self.heard(now, true);
                // The cookie is for the address the master saw from this
                // one of its addresses: the Register goes back there.
                self.answered_from(from);
                self.send_register(challenge.cookie, now);
                // The Register with the cookie is a request of its own.
                self.asked_since = Some(now);
                self.next_ask = now + REQUEST_RETRY;
            }
            MasterPacket::Listed(listed) => {
                if listed.nonce != self.nonce || !self.wanted {
                    self.counters.unexpected += 1;
                    return;
                }
                self.heard(now, true);
                self.answered_from(from);
                let again = self
                    .listing
                    .is_some_and(|l| l.listing_id == listed.listing_id && l.token == listed.token);
                let seconds = |secs: u8, default: Duration| {
                    if secs == 0 {
                        default
                    } else {
                        Duration::from_secs(u64::from(secs))
                    }
                };
                let intervals = (self.heartbeat_interval, self.keep_interval);
                self.heartbeat_interval = seconds(listed.heartbeat_secs, HEARTBEAT_INTERVAL);
                self.keep_interval = seconds(listed.keep_secs, KEEP_INTERVAL);
                if !again || intervals != (self.heartbeat_interval, self.keep_interval) {
                    self.part_changes += 1;
                }
                if !again {
                    self.listing = Some(Listing {
                        listing_id: listed.listing_id,
                        token: listed.token,
                        seen: listed.seen,
                    });
                    self.last_heartbeat = now;
                    self.events.push_back(RendezvousEvent::Listed {
                        listing_id: listed.listing_id,
                        seen: listed.seen,
                    });
                }
            }
            MasterPacket::HeartbeatAck(ack) => {
                let Some(listing) = self.listing.as_mut() else {
                    self.counters.unexpected += 1;
                    return;
                };
                if ack.listing_id != listing.listing_id {
                    self.counters.unexpected += 1;
                    return;
                }
                if self.moving {
                    // The master moved the listing here (slice K8): it is
                    // this game's from now on.
                    self.moving = false;
                    listing.seen = ack.seen;
                    self.events.push_back(RendezvousEvent::Listed {
                        listing_id: listing.listing_id,
                        seen: ack.seen,
                    });
                } else if ack.seen != listing.seen {
                    listing.seen = ack.seen;
                    self.events
                        .push_back(RendezvousEvent::SeenChanged(ack.seen));
                }
                self.heard(now, true);
                self.answered_from(from);
            }
            MasterPacket::UnknownListing(unknown) => {
                if self.listing.is_none_or(|l| l.token != unknown.token) {
                    self.counters.unexpected += 1;
                    return;
                }
                // The master restarted, or dropped the listing: register again
                // at once.
                self.heard(now, true);
                self.answered_from(from);
                self.listing = None;
                self.moving = false;
                self.part_changes += 1;
                self.nonce = self.rng.next_u64();
                self.next_ask = now;
            }
            MasterPacket::Unsupported(unsupported) => {
                self.heard(now, true);
                if self.refused.is_none() {
                    let text = if unsupported.text.is_empty() {
                        "The Internet Lobby does not support this version of the game.".to_owned()
                    } else {
                        unsupported.text
                    };
                    self.events
                        .push_back(RendezvousEvent::Refused(text.clone()));
                    self.refused = Some(text);
                }
                self.listing = None;
                self.moving = false;
                self.part_changes += 1;
            }
            MasterPacket::ProbeAnswer(answer) => {
                if answer.nonce != self.mapping.nonce {
                    self.counters.unexpected += 1;
                    return;
                }
                self.heard(now, false);
                match answer.port {
                    ProbePort::Main => self.mapping.main = Some(answer.seen),
                    ProbePort::Second => self.mapping.second = Some(answer.seen),
                }
                self.judge_mapping(from);
            }
            MasterPacket::Meet(meet) => {
                self.heard(now, false);
                self.meet(now, from, &meet);
            }
            MasterPacket::RelayOpen(open) => {
                self.heard(now, false);
                let token = self
                    .listing
                    .filter(|_| self.wanted)
                    .map(|listing| listing.token);
                if let Some(ack) = self.relays.open(now, from, &open, token) {
                    self.counters.relay_opens += 1;
                    self.send(from, &ack);
                }
            }
            MasterPacket::RelayClose(close) => {
                self.heard(now, false);
                if self.relays.close(&close) {
                    self.counters.relay_closes += 1;
                } else {
                    self.counters.unexpected += 1;
                }
            }
            _ => self.counters.unexpected += 1,
        }
    }

    /// A Meet: punches for the player's addresses and an ack to the master,
    /// as [`Meets`] decides. Only a listed game has the token an ack needs.
    fn meet(&mut self, now: Duration, from: SocketAddr, meet: &super::packet::Meet) {
        let Some(listing) = self.listing.filter(|_| self.wanted) else {
            self.counters.meets_dropped += 1;
            return;
        };
        match self.meets.receive(now, meet) {
            MeetOutcome::Punching => self.counters.meets += 1,
            MeetOutcome::Again => self.counters.meets_again += 1,
            MeetOutcome::OverLimit => {
                self.counters.meets_dropped += 1;
                return;
            }
        }
        self.send_punches(now);
        let ack = MasterPacket::MeetAck(MeetAck {
            token: listing.token,
            introduction_id: meet.introduction_id,
        });
        self.send(from, &ack);
    }

    /// The punches due: transport packets of the game's protocol version,
    /// sent beside the master's datagrams on the game port.
    fn send_punches(&mut self, now: Duration) {
        let version = self.config.build.protocol_version;
        for (to, introduction) in self.meets.due(now) {
            if let Ok(datagram) = Packet::Punch(Punch { introduction }).encode(version) {
                self.counters.punches += 1;
                self.out.push_back(Transmit { to, datagram });
            }
        }
    }

    /// Runs the timers: requests and their retries, heartbeats, keeps, the
    /// mapping test, silence, and a Meet's punches.
    pub fn update(&mut self, now: Duration) {
        if self.meets.punching() {
            self.send_punches(now);
        }
        self.relays.channels.forget_idle(now);
        if !self.wanted || self.refused.is_some() || self.masters.is_empty() {
            self.unsent.clear();
            return;
        }
        self.fail_over(now);
        // A move the master has not acknowledged may only be waiting out
        // its once-a-minute rule (slice K8).
        let silent_after = if self.moving {
            super::LISTING_EXPIRY
        } else {
            MASTER_SILENT
        };
        if let Some(since) = self.asked_since
            && !self.silent
            && now >= since + silent_after
        {
            self.silent = true;
            self.events.push_back(RendezvousEvent::MasterSilent);
            // The next try goes to the master's next address, if it has one
            // ([`Rendezvous::ask`]).
            if self.listing.is_none() {
                self.nonce = self.rng.next_u64();
            }
            self.next_ask = now + self.backoff;
        }
        self.probe_if_due(now);
        let retry_due = self.asked_since.is_some() && now >= self.next_ask;
        match self.listing {
            None => {
                if self.summary.is_some() && now >= self.next_ask {
                    self.ask(now);
                }
            }
            Some(_) if self.summary.is_some() => {
                let gap_ok = self
                    .last_listing_send
                    .is_none_or(|at| now >= at + LISTING_GAP);
                // A listing carried on from another host heartbeats at once
                // (slice K8).
                let heartbeat_due = now >= self.last_heartbeat + self.heartbeat_interval
                    || (self.changed && now >= self.last_heartbeat + CHANGE_HEARTBEAT_DELAY)
                    || self.moving;
                if gap_ok && (retry_due || (self.asked_since.is_none() && heartbeat_due)) {
                    self.ask(now);
                } else if gap_ok
                    && self.asked_since.is_none()
                    && self
                        .last_listing_send
                        .is_none_or(|at| now >= at + self.keep_interval)
                    // A Heartbeat due within the gap keeps the mapping open
                    // itself, and a Keep now would hold it back.
                    && now + LISTING_GAP < self.last_heartbeat + self.heartbeat_interval
                {
                    self.send_keep(now);
                }
            }
            // A resumed listing waits for the hosting loop's first summary.
            Some(_) => {}
        }
    }

    /// Sends the request of the moment (a Register before Listed, a
    /// Heartbeat after) and sets its retry.
    fn ask(&mut self, now: Duration) {
        // A silent master is asked at its next address each time, round and
        // back to the first, unless the address in use is yet untried.
        if self.silent
            && !self.untried
            && let Some(next) = self.next_master(self.current, false)
        {
            self.switch_master(next, MasterSwitch::NoAnswer);
        }
        self.untried = false;
        if self.listing.is_some() {
            self.send_heartbeat(now);
        } else {
            self.send_register(0, now);
        }
        self.asked_since.get_or_insert(now);
        self.next_ask = if self.silent {
            let at = now + self.backoff;
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
            at
        } else {
            now + REQUEST_RETRY
        };
    }

    /// A packet from the master: it is not silent. `answer` when the packet
    /// answers the request under way.
    fn heard(&mut self, now: Duration, answer: bool) {
        if answer {
            self.asked_since = None;
            self.next_ask = now;
        }
        if self.silent {
            self.silent = false;
            self.backoff = FIRST_BACKOFF;
        }
    }

    fn send(&mut self, to: SocketAddr, packet: &MasterPacket) {
        match packet.encode() {
            Ok(datagram) => {
                self.counters.sent += 1;
                self.out.push_back(Transmit { to, datagram });
            }
            // Only a summary that breaks its limits could fail, and fit()
            // cuts it first; nothing is sent rather than a broken packet.
            Err(_) => self.counters.unexpected += 1,
        }
    }

    fn send_register(&mut self, cookie: u64, now: Duration) {
        let (Some(master), Some(summary)) = (self.master(), self.summary.clone()) else {
            return;
        };
        let register = Register {
            nonce: self.nonce,
            cookie,
            build: self.config.build.clone(),
            dedicated: self.config.dedicated,
            telemetry: self.config.install_id.is_some(),
            install_id: self.config.install_id.unwrap_or(0),
            platform: self.config.platform,
            candidates: self.candidates.clone(),
            summary,
        }
        .fit();
        self.send(master, &MasterPacket::Register(register));
        self.changed = false;
        self.last_heartbeat = now;
        self.last_listing_send = Some(now);
    }

    fn send_heartbeat(&mut self, now: Duration) {
        let (Some(master), Some(listing), Some(summary)) =
            (self.master(), self.listing, self.summary.clone())
        else {
            return;
        };
        let heartbeat = Heartbeat {
            token: listing.token,
            change: self.change,
            candidates: self.candidates.clone(),
            summary,
        }
        .fit();
        self.send(master, &MasterPacket::Heartbeat(heartbeat));
        self.changed = false;
        self.last_heartbeat = now;
        self.last_listing_send = Some(now);
    }

    fn send_keep(&mut self, now: Duration) {
        let (Some(master), Some(listing)) = (self.master(), self.listing) else {
            return;
        };
        self.send(
            master,
            &MasterPacket::Keep(Keep {
                token: listing.token,
            }),
        );
        self.last_listing_send = Some(now);
    }

    fn unregister(&mut self) {
        let (Some(master), Some(listing)) = (self.master(), self.listing.take()) else {
            return;
        };
        self.moving = false;
        self.part_changes += 1;
        for _ in 0..GOODBYE_COPIES {
            self.send(
                master,
                &MasterPacket::Unregister(Unregister {
                    token: listing.token,
                }),
            );
        }
    }

    /// Sends the mapping test's two Probes when they are due: when listing
    /// starts, again while an answer is missing (three tries,
    /// [`REQUEST_RETRY`] apart), and every 10 minutes.
    fn probe_if_due(&mut self, now: Duration) {
        if now < self.mapping.next {
            return;
        }
        let Some(master) = self.master() else {
            return;
        };
        if self.mapping.done || self.mapping.tries >= PROBE_TRIES {
            self.mapping = Mapping::new(self.mapping.result, now);
        }
        if self.mapping.tries == 0 {
            self.mapping.nonce = self.rng.next_u64();
            self.mapping.started = now;
        }
        let probe = MasterPacket::Probe(Probe {
            nonce: self.mapping.nonce,
        });
        if self.mapping.main.is_none() {
            self.send(master, &probe);
        }
        if let Some(second) = probe_address(master)
            && self.mapping.second.is_none()
        {
            self.send(second, &probe);
        }
        self.mapping.tries += 1;
        self.mapping.next = if self.mapping.tries < PROBE_TRIES {
            now + REQUEST_RETRY
        } else {
            self.mapping.started + MAPPING_TEST_INTERVAL
        };
    }

    /// The test's verdict once both answers are in (or the main one shows no
    /// translation).
    fn judge_mapping(&mut self, from: SocketAddr) {
        let Some(main) = self.mapping.main else {
            return;
        };
        let second = self.mapping.second;
        // The host's own candidates as given, not the router's mapped
        // address, which is not the machine's own.
        let own = self
            .own
            .iter()
            .map(|c| c.address)
            .find(|a| a.is_ipv4() == from.is_ipv4());
        let result = MappingType::from_probes(own, main, second);
        if self.mapping.done || (second.is_none() && result != MappingType::NoTranslation) {
            return;
        }
        self.mapping.done = true;
        self.mapping.result = result;
        self.mapping.next = self.mapping.started + MAPPING_TEST_INTERVAL;
        self.events
            .push_back(RendezvousEvent::MappingTested(result));
    }
}

impl MasterSide for Rendezvous {
    fn is_master(&self, from: SocketAddr) -> bool {
        Rendezvous::is_master(self, from)
    }

    fn take_master(&mut self, now: Duration, from: SocketAddr, datagram: &[u8]) {
        self.receive(now, from, datagram);
    }

    fn channels(&mut self) -> &mut Channels {
        &mut self.relays.channels
    }

    fn claimed(&mut self) {
        self.counters.relayed_claims += 1;
    }

    fn send_dropped(&mut self) {
        self.counters.relay_sends_dropped += 1;
    }
}

/// How a remote player reached the host, as far as a host in stage I can
/// tell from the player's address: the relay's prefix, a private or
/// link-local address (the local network), else a typed or found address.
/// *Agent decision:* stage J's path in the Challenge answer replaces this.
pub fn path_of(address: SocketAddr) -> Path {
    if is_relayed(address) {
        return Path::Relay;
    }
    let local = match canonical(address).ip() {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    };
    if local {
        Path::LocalNetwork
    } else {
        Path::ByAddress
    }
}

/// What a host counts for its Report at a session's end ("Reports" in the
/// master protocol): how long, the most humans at once, and its remote
/// players by their path. The hosting player's own connection over the
/// in-process link is a human but has no path.
#[derive(Debug, Clone)]
pub struct HostTally {
    started: Duration,
    present: BTreeSet<SocketAddr>,
    counted: BTreeSet<SocketAddr>,
    most: usize,
    by_path: [u8; 6],
}

impl HostTally {
    /// A session that starts at `now`.
    pub fn new(now: Duration) -> Self {
        Self {
            started: now,
            present: BTreeSet::new(),
            counted: BTreeSet::new(),
            most: 0,
            by_path: [0; 6],
        }
    }

    /// The players connected now, by their addresses: the host's loop
    /// gives them after each join or departure.
    pub fn present(&mut self, addresses: impl IntoIterator<Item = SocketAddr>) {
        self.present = addresses.into_iter().collect();
        self.most = self.most.max(self.present.len());
        for &address in &self.present {
            if address != LINK_ADDRESS && self.counted.insert(address) {
                let slot = &mut self.by_path[usize::from(path_of(address).code())];
                *slot = slot.saturating_add(1);
            }
        }
    }

    /// True when anyone joined the session.
    pub fn anyone(&self) -> bool {
        self.most > 0
    }

    /// The session's Report, its install id filled in by
    /// [`Rendezvous::report`].
    pub fn report(
        &self,
        now: Duration,
        role: Role,
        game_version: &str,
        platform: u8,
        mapping: MappingType,
    ) -> Report {
        let mut version = game_version.to_owned();
        while version.len() > super::packet::MAX_BUILD_TEXT {
            version.pop();
        }
        Report {
            install_id: 0,
            role,
            game_version: version,
            platform,
            minutes: u16::try_from(now.saturating_sub(self.started).as_secs() / 60)
                .unwrap_or(u16::MAX),
            humans: u8::try_from(self.most).unwrap_or(u8::MAX),
            path: Path::LocalNetwork,
            connect_tenths: 0,
            mapping,
            port_mapping: PortMapping::NotTried,
            relayed_kb: 0,
            players_by_path: self.by_path,
            migrations: 0,
            failed_migrations: 0,
        }
    }
}

/// A host's listing with what needs the real world: the master's name looked
/// up on a thread (again every 10 minutes, at once when the master falls
/// silent and every minute while it stays silent), the host's own addresses
/// toward it, and which of the master's addresses the system has a route
/// to. The host's loop changes
/// by a line or two:
///
/// ```ignore
/// host.receive_from(now, &mut listing.over(&mut socket, now))?;
/// host.update(now);
/// listing.update(now, || host.discover_answer(0).into());
/// host.transmit(&mut listing.over(&mut socket, now))?;
/// listing.transmit(&mut socket)?;
/// ```
#[derive(Debug)]
pub struct HostListing {
    rendezvous: Rendezvous,
    host: String,
    port: u16,
    game_port: u16,
    lookup: Option<MasterLookup>,
    next_lookup: Duration,
    lookup_backoff: Duration,
    was_silent: bool,
}

impl HostListing {
    /// A host's listing on `game_port`, with the master at `master` (a name
    /// or an address, port 26901 when none is given), not listed until
    /// [`HostListing::set_listed`].
    pub fn new(
        master: &str,
        config: HostRendezvous,
        game_port: u16,
        now: Duration,
    ) -> Result<Self, String> {
        let (host, port) = parse_master(master)?;
        Ok(Self {
            rendezvous: Rendezvous::host(config, now),
            host,
            port,
            game_port,
            lookup: None,
            next_lookup: now,
            lookup_backoff: FIRST_BACKOFF,
            was_silent: false,
        })
    }

    /// A listing carried on from another host's `part` on `game_port`
    /// (stage K, slice K8): see [`Rendezvous::resume`]. The master is the
    /// part's, by its name; its address from the part is used at once, and
    /// the name is looked up as for any listing. Fails only when the part's
    /// master name does not parse.
    pub fn resume(
        part: &ListingPart,
        config: HostRendezvous,
        game_port: u16,
        now: Duration,
    ) -> Result<Self, String> {
        let (host, port) = parse_master(&part.master_name)?;
        let candidates = host_candidates(&[part.master], game_port, own_address_toward);
        Ok(Self {
            rendezvous: Rendezvous::resume(config, part, candidates, now),
            host,
            port,
            game_port,
            lookup: None,
            next_lookup: now,
            lookup_backoff: FIRST_BACKOFF,
            was_silent: false,
        })
    }

    /// The listing part another game would carry the listing on with, with
    /// the master's name filled in: see [`Rendezvous::listing_part`].
    pub fn part(&self) -> Option<ListingPart> {
        let mut part = self.rendezvous.listing_part()?;
        part.master_name = self.master_text();
        Some(part)
    }

    /// See [`Rendezvous::part_version`].
    pub fn part_version(&self) -> u64 {
        self.rendezvous.part_version()
    }

    /// See [`Rendezvous::release`]; the part with the master's name.
    pub fn release(&mut self) -> Option<ListingPart> {
        let mut part = self.rendezvous.release()?;
        part.master_name = self.master_text();
        Some(part)
    }

    /// The master as given, for messages: `HOST:PORT`.
    pub fn master_text(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The state machine inside.
    pub fn rendezvous(&self) -> &Rendezvous {
        &self.rendezvous
    }

    /// The state machine inside, to change.
    pub fn rendezvous_mut(&mut self) -> &mut Rendezvous {
        &mut self.rendezvous
    }

    /// See [`Rendezvous::over`].
    pub fn over<'a, D: Datagrams + ?Sized>(
        &'a mut self,
        socket: &'a mut D,
        now: Duration,
    ) -> Routed<'a, D> {
        self.rendezvous.over(socket, now)
    }

    /// See [`Rendezvous::relay_channels`].
    pub fn relay_channels(&self) -> Vec<(u32, u32, SocketAddr)> {
        self.rendezvous.relay_channels()
    }

    /// See [`Rendezvous::set_listed`].
    pub fn set_listed(&mut self, listed: bool, now: Duration) {
        self.rendezvous.set_listed(listed, now);
    }

    /// See [`Rendezvous::set_mapped`]; kept across the master lookups that
    /// rebuild the host's own candidates.
    pub fn set_mapped(&mut self, mapped: Option<SocketAddr>, now: Duration) {
        self.rendezvous.set_mapped(mapped, now);
    }

    /// The lookup, the summary when it is wanted, and the rendezvous's
    /// timers. `summary` is called at most once a second.
    pub fn update(&mut self, now: Duration, summary: impl FnOnce() -> ListingSummary) {
        if let Some(lookup) = self.lookup.as_mut()
            && let Some(answer) = lookup.poll()
        {
            self.lookup = None;
            match answer {
                Ok(masters) => {
                    let candidates = host_candidates(&masters, self.game_port, own_address_toward);
                    // An address the system has no route to (an IPv6 one on a
                    // host without IPv6) is tried last.
                    self.rendezvous.set_masters_routed(
                        masters,
                        candidates,
                        |master| own_address_toward(master).is_some(),
                        now,
                    );
                    self.next_lookup = now + self.lookup_interval();
                    self.lookup_backoff = FIRST_BACKOFF;
                }
                Err(error) => {
                    self.rendezvous
                        .events
                        .push_back(RendezvousEvent::LookupFailed(error));
                    self.next_lookup = now + self.lookup_backoff;
                    self.lookup_backoff = (self.lookup_backoff * 2).min(MAX_LOOKUP_BACKOFF);
                }
            }
        }
        let silent = self.rendezvous.state() == ListingState::Silent;
        if silent && !self.was_silent {
            // The master may have moved: look its name up again now.
            self.next_lookup = now;
        }
        self.was_silent = silent;
        if self.rendezvous.listed_wanted() && self.lookup.is_none() && now >= self.next_lookup {
            self.lookup = Some(MasterLookup::start(&self.host, self.port));
            self.next_lookup = now + LOOKUP_INTERVAL;
        }
        if self.rendezvous.wants_summary(now) {
            self.rendezvous.set_summary(now, summary());
        }
        self.rendezvous.update(now);
    }

    /// See [`Rendezvous::transmit`].
    pub fn transmit<D: Datagrams + ?Sized>(&mut self, socket: &mut D) -> io::Result<()> {
        self.rendezvous.transmit(socket)
    }

    /// How long until the master's name is looked up again: a minute while
    /// the master is silent, else 10 minutes.
    fn lookup_interval(&self) -> Duration {
        if self.rendezvous.state() == ListingState::Silent {
            SILENT_LOOKUP_INTERVAL
        } else {
            LOOKUP_INTERVAL
        }
    }

    /// See [`Rendezvous::poll_event`].
    pub fn poll_event(&mut self) -> Option<RendezvousEvent> {
        self.rendezvous.poll_event()
    }

    /// See [`Rendezvous::state`].
    pub fn state(&self) -> ListingState {
        self.rendezvous.state()
    }

    /// See [`Rendezvous::stop`].
    pub fn stop(&mut self, now: Duration) {
        self.rendezvous.stop(now);
    }
}

/// Where the listing stands, as a player reads it (the words of the
/// architecture guide's "Listing a game"), for example `Listed on the
/// Internet Lobby, seen at 203.0.113.5:26900.`
pub fn state_text(state: &ListingState) -> String {
    match state {
        ListingState::Off => "Not listed on the Internet Lobby.".into(),
        ListingState::FindingMaster => "Looking up the Internet Lobby...".into(),
        ListingState::Registering => "Listing the game on the Internet Lobby...".into(),
        ListingState::Listed { seen, .. } => {
            format!("Listed on the Internet Lobby, seen at {seen}.")
        }
        ListingState::Silent => "The Internet Lobby does not answer, so the game is not \
                                 listed. Players can still join by address."
            .into(),
        ListingState::Refused(text) => format!("The Internet Lobby refused the listing: {text}"),
    }
}

#[cfg(test)]
#[path = "rendezvous_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "listing_part_tests.rs"]
mod listing_part_tests;

#[cfg(test)]
#[path = "failover_tests.rs"]
mod failover_tests;
