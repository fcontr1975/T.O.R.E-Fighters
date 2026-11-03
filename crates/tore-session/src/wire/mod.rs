//! The game's wire messages: the sections a Payload carries (Inputs,
//! Snapshot, Events, Own state) and the reliable message bodies, with the
//! bookkeeping each end keeps per connection.
//!
//! The specification is [`docs/formats/net-protocol.md`](../../../../docs/formats/net-protocol.md)
//! from "Inputs" on; where the build settled a detail the design left open,
//! that document says so. Everything here is plain data and pure functions of
//! it: no clock, socket or `World` step.
//!
//! - [`inputs`]: the Inputs section, the controls' quantization and the
//!   numbered commands.
//! - [`entity`]: the entities a snapshot carries, their quantized states and
//!   their records against a baseline.
//! - [`snapshot`]: the Snapshot section, its header, and the per-connection
//!   baselines: [`snapshot::EntitySender`] on the host and
//!   [`snapshot::EntityReceiver`] on the client.
//! - [`priority`]: the relevance bands and the priority accumulator.
//! - [`readout`]: the cockpit readout inside a snapshot, against the one
//!   the client acknowledged.
//! - [`space`]: how a snapshot packet's 1,200 bytes are shared.
//! - [`events`]: the Events section, the host's queue and the client's
//!   receiver.
//! - [`names`]: the name table the Names message fills.
//! - [`own_state`]: the Own state section, the exact state of the player's
//!   plane against an acknowledged one.
//! - [`messages`]: the reliable message bodies.
//! - [`migration`]: stage K's message bodies, host migration and rejoin.
//! - [`connection`]: the host's and the client's per-connection wire state,
//!   which tie the parts above to the transport's packet numbers.
//! - [`from_world`]: filling the wire's plain data from `tore-world`'s.
//!
//! Every decoder is bounded: it checks each count and length against the
//! protocol's limits before reading on, and returns a [`WireError`] for any
//! bytes, never panicking.

pub mod chat;
pub mod connection;
pub mod entity;
pub mod events;
pub mod feedback;
pub mod from_world;
pub mod inputs;
pub mod messages;
pub mod migration;
pub mod names;
pub mod own_state;
pub mod priority;
pub mod readout;
pub mod snapshot;
pub mod space;

pub(crate) mod bits;
mod flat;
mod row;

#[cfg(test)]
mod compat_tests;
#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod lossy_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod phase2_tests;
#[cfg(test)]
pub(crate) mod readout_a2g_tests;
#[cfg(test)]
mod readout_tests;
#[cfg(test)]
mod round_trip_tests;
#[cfg(test)]
pub(crate) mod samples;

use std::fmt;
use tore_codec::CodecError;

/// The operating system a player's game runs on: the transport's, which the
/// lobby's player list carries too (protocol 7).
pub use tore_net::Platform;

/// How a player reached the host: the master's path codes, which the
/// Challenge answer and the lobby's player list carry too (protocol 9 and 12).
pub use tore_net::master::Path;

/// A path in the words the lobby, the logs and `tore-bot` use: "local
/// network", "by address", "mapped port", "IPv6", "punched", "relay".
pub fn path_words(path: Path) -> &'static str {
    match path {
        Path::LocalNetwork => "local network",
        Path::ByAddress => "by address",
        Path::MappedPort => "mapped port",
        Path::Ipv6 => "IPv6",
        Path::Punched => "punched",
        Path::Relay => "relay",
    }
}

/// The protocol version: one number for every byte of the protocol, the
/// transport's included. Any change to the bytes raises it; the wire golden
/// test fails until it is raised and the committed copy refreshed
/// (`TORE_UPDATE_WIRE_GOLDEN=1`). 2 since the readout's coding, 3 since the
/// lobby (EF4), 4 since chat (EF6), 5 since the transport's Keepalive packet
/// (EF-K), 6 since the exact flight state's overspeed countdown and failure
/// random stream, 7 since each player's platform (the Challenge answer and
/// the lobby's player list), 8 since stage F phase 2's messages, settings,
/// slot locks and wing replies (F2-0), 9 since the transport's Punch packet
/// and the path byte of the Challenge answer (stage J's slice J2), 10 since
/// stage L's Content and Content gaps messages and each lobby player's
/// Fighters Anthology build (slice L2), 12 since each lobby player's
/// connection path (slice J6), 13 since stage K's host migration and rejoin:
/// the transport's Reach and Reach answer packets, the Challenge answer's
/// token, disconnect reason 8, the Filler section, messages 39 to 54, the
/// lobby's standby marks and reserved slots, and setting 21 (slice K0), 14
/// since the standby stream codes a seat's controls as the Inputs section
/// does, its commands behind a presence bit, its view as Inputs does and its
/// command number applied as a bit when unchanged (slice K3), 15 since the
/// flight data link (slice G7): the readout's Link group and its tracks,
/// marks and mates, the Link event, the radio events' net, and the Sort order
/// and Battle net command the golden now lists, 16 since the baseline field
/// of the readout and the entity records is 7 bits, a window of 127
/// snapshots (slice B2), 17 since setting 22, the King's snapshot rate, is
/// in the lobby state's list and a flight starts from it (slice R1), 18
/// since the aircraft variety import: 6-bit aircraft codes for the 37
/// selectable aircraft, the Inputs section's powered-lift block and its
/// axis commands (24 to 26), the actual lift, vector and gun-mount devices,
/// the readout's gun aim and linked-gun mask, gun-group commands, the
/// loadout's tank list and the exact flight state's powered-lift controls,
/// 19 since the VTOL overhaul: the exact flight state's powered-lift state
/// (body rates, rotors and engines, stability level and trim, warning
/// timers, the corridor hold), the powered-lift command 27 with its
/// sub-codes, and switch 11, hover hold, 20 since the lobby pass: the slot
/// request's side (value 3, `SlotRequest::Side`), setting 7's `balanced`,
/// setting 12's 50 to 150 nautical miles and setting 23, AI respawn, in the
/// lobby state's list (slice W0), 21 since the AC-130 gunsight: the Inputs
/// frame's sight slew and zoom step, the sight commands 28 and 29, the
/// readout's Gunsight group (sight, look, aim point, pipper impacts, per-gun
/// readiness with TERRAIN MASK, notice and zoom) and the standby stream's
/// sight (gunsight slice S4), 22 since the surface round (slice N1): the
/// fifth entity kind, a moving surface unit, with the kind's code widened
/// to 3 bits in the Inputs section's view subject; the Surface burst,
/// Surface burst end and Surface unit events (codes 18 to 20); the surface
/// digest in the Seated message and the Resumed flight; the debrief pilot's
/// shot-down-by name; and the readout's painting radars (scalar group 14).
/// The ground target's text lines (`ground-target`, `defenses`,
/// `surface-seed`, `enemy-nationality`, `start ground auto`) travel in the
/// mission text, which the mission hash covers. 23 since the air-to-ground
/// round (slice NA): the readout's Sight, Sight sensor, Laser, Bomb sight,
/// Guided, HARM cue and Weapons in flight groups (scalar groups 15 to 21),
/// the ground pages' flags at the end of the Sensors group and the ground
/// page's contacts (list 16); the extended combat commands after code 31
/// (`NearestGround` 0, `ToggleLaser` 1, `SightNext` 2), combat command 30
/// (`ToggleAirGround`), the HARM channel (code 3) and readiness codes 25
/// (`NoEmitter`) and 26 (`TrackLost`), which the round's slices added under
/// 22, are part of 23's definition; a loadout row may name a laser
/// designator pod's SEE record. 24 since the mp-fixes round (slice FX-A):
/// every game builds the redrawn airports (docs/formats/redrawn-airports.md),
/// so the airfields' buildings, their added hangars and vehicles (new target
/// ids), the parked aircraft and the AI's airfield points differ from a 23
/// game's; no byte layout changed for it. Folded under 24: the extended
/// combat commands 3 (`GunKeyDown`) and 4 (`GunKeyUp`) after code 31 (slice
/// FX-T) and the readout's Guided sight group's level T, 15 to 18 values
/// (slice FX-H). 25 since the surface close-out (slice CL-CF): every game
/// builds the city fill (`tore_world::city`), whose buildings aircraft
/// crash into and ground bursts knock flat, which a 24 game lacks. Folded
/// under 25: the mission text's `enemy-start` line (slice CL-R), which a 24
/// game refuses; `ground_trim` in the AI airfield sequence's checkpoint
/// (slice CL-AP); the front line in the surface digest (slice CL-FL); and
/// the City flattened event (code 21) with the world checkpoint's city fill
/// section (section 12), which carry the buildings ground bursts knocked
/// flat (review fixes). 26 since the bug-fix round's slice BF: the Effect
/// event (code 11) carries the size it is drawn at, percent of its
/// explosion type's (`tore_sim::combat::blast_scale`), as a one-bit flag and
/// a byte when it is not 100, which a 25 game cannot read.
/// 27 since the stores slice (BP3): the Stores event (code 22), every
/// aircraft's drawn stations and the whole stores on each, sent when they
/// change and at a seat, so a client draws every aircraft's pylons and
/// stores (docs/spec/stores-on-pylons.md).
/// 28 since the networking slice NET2: the transport's Keepalive carries a
/// flags byte, and a host sends it marked loading to every player while it
/// builds a mission, so its players keep the connection and its standbys
/// hold off a migration through a long build.
/// 29 since the AC-130 sensor console (slice nr-orbit): seat command 28
/// (SensorConsole), and the exact own-plane state's autopilot carries the
/// orbit hold (mode Orbit 4 and its plan and memory).
/// 30 since the sensor console's search and keys (slice nr-cona): seat
/// command 29 (Console, a sub-code and a value), and the orbit in the exact
/// own-plane state carries `pinned`, `heading` and `altitude`.
/// 31 since the gun lock and the centre's pause (slice nr-cona follow-up):
/// Console sub-code 5 (GunLock), the orbit's `still_s` in the exact
/// own-plane state, and the gun lock in the readout's gunsight group.
/// 32 since the airport merge (slice nr-apmerge): no message changes, but
/// the shared world differs (84 paired theater airfields are single fields
/// with new runways and ids aliased), so an older game must not join.
/// 33 since the missile identities pass (slice nr-fox2): no message
/// changes, but the store catalog (new R-27 variants, corrected names and
/// carrier rules), IR seeker response and radio cooldown keys differ, so an
/// older game must not join.
/// 34 since the player release pass (slice nr-fox3): no message changes,
/// but player missile release outcomes (advisory MAX RANGE and lock, active
/// maddog, SARH support recovery) differ, so an older game must not join.
/// 35 since the F-15/F-16 stations pass (slice nr-f15): no message changes,
/// but the F-15C, F-16C and Mi-24 store stations (release positions) moved,
/// so an older game must not join.
/// 36 since the wingman loadouts pass (slice nr-wingload): no message
/// changes, but AI wingmen take their human lead's loadout (more
/// FlightLoadouts entries, mission text `share-flight-loadout`), so an
/// older game must not join.
/// 37 since jettison (slice nr-jett): the extended live command 5
/// (JettisonTanks), tank quantities in store stations 55 to 63, the
/// sound-only jettison blast and inert falling bodies, so an older game must
/// not join.
/// 38 since the bomb flight model (slice nr-bombphys): bombs carry their
/// body (mass, drag, retarder, steering) and fly one shared fitted model, and
/// sights read it, so an older game must not join.
/// 39 since the small guided bombs (slice nr-bombs2): GBU-12, GBU-16 and
/// GBU-38 join the store catalog, so an older game must not join.
/// 40 since the AC-130 damage pass (slice nr-nightfx): the 40 mm and 105 mm
/// shells do twice the damage, so an older game must not join.
/// 41 since the contact bounds fix (slice nr-mav): thin line geometry no
/// longer inflates surface objects' solid contact boxes, so an older game
/// must not join.
/// 42 since the cleanup round (slices nr-ac105, nr-aijett, nr-fx2,
/// nr-sndfix): the 105 mm splash and AI jettison change the shared
/// simulation, so an older game must not join.
/// 43 since the extra stations (slice nr-stations): F-15C centreline and
/// wing tank rows, the F-16C inner wet pair and two tank records, so an
/// older game must not join.
/// 44 since the second stations pass (slice nr-stations2): F-15C centreline
/// bomb rack, F-22 wing stations, MiG-29 and Rafale centreline tanks and
/// three tank records, so an older game must not join.
/// 45 since the ordnance page pass (slice nr-ordui): the Pave Knife is no
/// longer a weapon item and a loadout naming it loads the Pave Spike, so an
/// older game must not join.
/// 46 since the store signature (slice nr-rcs): external stores raise the
/// radar signature every radar and seeker reads, so an older game must not
/// join.
/// 47 since the F-16C wingtip AIM-120 (slice nr-f16tip): an older game
/// refuses the load, so an older game must not join.
/// 48 since Player Choice and AI off (slice nr-mpchoice): the ai setting,
/// Choice slots, aircraft in loadouts and revivals, and the despawn and
/// replacement commands, so an older game must not join.
/// 49 since the collision credit fix (slice nr-collkill): splash on a
/// bystander no longer makes its shooter the last attacker, which changes
/// kill credit and the combat checkpoint, so an older game must not join.
/// 50 since the F-4B gun pod option (slice nr-f4gun): the F-4B's centreline
/// row takes one SUU-16, which an older game refuses, and the `Stores` event
/// lists a hung gun pod as one store, so an older game must not join.
/// 51 since the Stores station index (slice nr-basefix): a Stores event's
/// station index is 7 bits and can name tank hardpoint 9 (code 64, the
/// F-16C inner wet pair and the Rafale centreline), so an older game must
/// not join.
/// 52 since min-players and the King's force start (slices nr-minplayers, nr-lobbyui): the lobby settings list gains min-players, and the Lobby message gains the force-start seconds, so an older game must not join.
/// 63 since shared ground-pad foundations change terrain contact and placement.
/// 64 since F-22N bay/wing carriage, magazine release origins and bay-only trigger gating.
/// 71 combines compatibility-code handshakes with private reservation preflight
/// packets 14 and 15 for confirmed lounge return slots.
/// 72 adds human visual designation and its checkpoint retention state.
/// 73 adds cumulative flight reports and persistent human/AI score identities.
/// 74 changes rules only: tanks against aircraft and host migration resume.
/// 75 for 0.2.2: the energy model for guided ordnance (new checkpoint state),
/// paired store loading, AI missile defense state, bridge grounding and
/// Quick Mission placement rules and seed retry.
/// 76 adds the readout's range-cue minimum, reachable range regions in the
/// prediction checkpoint, AI countermeasure pair spacing and the chaff buff.
/// 77 doubles the chaff multiplier to 2.60 (rules only).
/// 78 adds the Quick Mission `situation` line to the mission text and the
/// fixed distance table with the server's distance adjustment.
/// 79 adds the AI strike's early retarget and 5 s release commitment
/// (new strike checkpoint state).
/// 80 keeps AI helicopters out of known threat rings they planned through
/// (rules only).
/// 81 sizes helicopter threat rings from each site's real launch reach,
/// advances by the store's flight time, keeps ambushes to unknown shooters
/// and releases stand-off stores outside known rings (new checkpoint state).
/// 82 hides shoulder-fired SAM teams until spotted and adds the helicopter
/// move-ahead ambush on moving targets (new checkpoint state).
pub const PROTOCOL_VERSION: u16 = 82;

/// Section kinds after the transport's own Messages (kind 1).
/// The tick of each interval at which a seat's snapshots are built: ticks
/// where the tick modulo `ticks_per_snapshot` is this phase. Seats are spread
/// over the interval by their numbers, so a host with many players never
/// builds every snapshot on one tick (D10 follow-up, agent decision). The
/// client keeps the own-state hash at the same ticks.
pub fn snapshot_phase(seat: u8, ticks_per_snapshot: u32) -> u64 {
    u64::from(seat) % u64::from(ticks_per_snapshot.max(1))
}

pub const SECTION_INPUTS: u8 = 2;
/// The Snapshot section.
pub const SECTION_SNAPSHOT: u8 = 3;
/// The Events section.
pub const SECTION_EVENTS: u8 = 4;
/// The Own state section.
pub const SECTION_OWN_STATE: u8 = 5;
/// The Filler section (protocol 13, client to host): zero bytes a game sends
/// only for an Upload test, which the host ignores and counts.
pub const SECTION_FILLER: u8 = 6;

/// Why a section or a message could not be written or read.
#[derive(Debug, Clone, PartialEq)]
pub enum WireError {
    /// The bits themselves: the data ended early, a field was out of its
    /// width, or a form the writer never produces.
    Codec(CodecError),
    /// A count over the protocol's limit.
    TooMany {
        /// What was counted.
        what: &'static str,
        /// The limit.
        limit: usize,
    },
    /// A value the protocol does not allow, named.
    Invalid(&'static str),
    /// Bits other than zero padding after the body.
    Trailing,
    /// The own plane's exact state could not be coded or rebuilt.
    Exact(String),
    /// More unacknowledged events than the protocol allows: the connection is
    /// too far behind and ends.
    TooFarBehind,
    /// A journal record's coding by the checkpoint trait failed (stage K).
    Journal(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => write!(f, "{error}"),
            Self::TooMany { what, limit } => write!(f, "more than {limit} {what}"),
            Self::Invalid(what) => write!(f, "invalid {what}"),
            Self::Trailing => f.write_str("data after the end of the body"),
            Self::Exact(error) => write!(f, "own state: {error}"),
            Self::TooFarBehind => f.write_str("too many unacknowledged events"),
            Self::Journal(error) => write!(f, "journal: {error}"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<CodecError> for WireError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

impl From<tore_sim::checkpoint::CheckpointError> for WireError {
    fn from(error: tore_sim::checkpoint::CheckpointError) -> Self {
        Self::Journal(error.to_string())
    }
}

impl From<tore_sim::flight::exact::ExactError> for WireError {
    fn from(error: tore_sim::flight::exact::ExactError) -> Self {
        Self::Exact(format!("{error:?}"))
    }
}

/// A result of the wire.
pub type WireResult<T> = Result<T, WireError>;

/// The protocol's limits (net-protocol.md, "Limits") that the game's
/// sections check.
pub mod limits {
    /// Aircraft records in one snapshot.
    pub const AIRCRAFT: usize = 64;
    /// Projectile records in one snapshot.
    pub const PROJECTILES: usize = 256;
    /// Debris records in one snapshot.
    pub const DEBRIS: usize = 256;
    /// Ejected pilot records in one snapshot.
    pub const PILOTS: usize = 64;
    /// Moving surface unit records in one snapshot (protocol 22).
    pub const SURFACE: usize = 128;
    /// Entries of a connection's name table.
    pub const NAMES: usize = 4096;
    /// Input ticks in one Inputs section.
    pub const INPUT_TICKS: usize = 24;
    /// Commands in one Inputs section.
    pub const COMMANDS: usize = 64;
    /// Unacknowledged events per connection.
    pub const EVENTS: usize = 1024;
    /// Seats a host has.
    pub const SEATS: usize = 30;
    /// A reassembled reliable message, bytes.
    pub const MESSAGE: usize = 65_536;
    /// Recording stems in one radio call or order voice.
    pub const STEMS: usize = 32;
    /// Hardpoints of one surface unit in a Surface unit event (protocol 22).
    pub const MOUNTS: usize = 64;
    /// Buildings in one City flattened event (protocol 25); more go in
    /// several.
    pub const FLATTENED: usize = 256;
    /// Stations in one Stores event (protocol 27): a loadout's stations.
    pub const STORE_STATIONS: usize = 64;
}
