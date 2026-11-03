//! The strike tactics (docs/spec/ai-strike.md, slice ST-T): what a striker
//! does with its ground picture, its stores and the shared release.
//!
//! One [`Strike`] per striker (an aircraft with the Strike role or an ordered
//! ground target, [`super::awareness::ground::GroundPicture::active`]). Each
//! mission step it reads the picture, the perceived threat, its own energy
//! and the solution the host worked out from the strike cockpit (the bomb or
//! rocket cue, the guided weapon's reach, a seeker's lock), and answers with
//! an [`Order`]: where to fly (a heading, a flight-path pitch and a speed for
//! the ordinary input-only steering adapter, so the aircraft flies the pass
//! through its own controls), which store to release now, and what it is
//! doing. Nothing here moves the aircraft or makes a round.
//!
//! The flow, all deterministic (John, 2026-10-11, decisions 3 and 13):
//!
//! - **Ingress** toward the target; at 10 nm (or at once when closer) the
//!   striker **chooses**: the target ([`super::targeting::select_strike_target`],
//!   SEAD radars first for an anti-radiation carrier), the store
//!   ([`choose_store`]) and the pass ([`choose_pass`]) from the perceived
//!   threat and the available speed, limited by the bombing class and the
//!   store. A pass draws its aim offset ([`aim_draw`], unguided weapons only).
//! - **Attack**: the pass's approach (retail steering frame: a point offset
//!   above the target, flown straight at), then the final: the cue is flown
//!   onto the aim point and the store is released when the shared release
//!   test passes.
//! - **Exit**: jink inside 10,000 ft, climb out, turn back at
//!   max(30,000 ft, 2R) and choose again: re-attack while the target stands
//!   and a usable store is left, else the next target, else **home**.
//! - **Defend**: a missile or gun defence the existing B47 reactions fly, or
//!   a fighter that threatens the striker, hands control back to the air
//!   logic; once the threat has gone the striker resumes at ingress if it
//!   is safe (the spec's safe test), else egresses and goes home unless it
//!   is safe by the exit distance.
//!
//! - **Lase** ([`lase`], slice ST-L): a striker with its own designator that
//!   releases a laser weapon holds its spot on the target in an offset turn
//!   until the weapon is down, then exits; a lase duty (the wing order) and a
//!   designator lasing for its wing's carriers fly the same hold.
//!
//! Helicopters and the gunship fly their own families (slice ST-F): the
//! helicopter's pop-up and stand-off attack ([`helicopter`]) and the
//! gunship's orbit with its linked guns ([`gunship`]).
use super::Experience;
use super::awareness::ground::{BriefedKind, GroundPicture, RecentWarnings, ThreatLevel};
use super::controller::{Activity, OwnState};
use super::motion::SpeedRequest;
use super::targeting::{self, ObjectId, Side, StrikeSource};
use super::weapon_service::StationId;
use crate::attitude::Vector;
use crate::combat::delivery::DeliveryMode;
use crate::models::strike_class::StrikeClass;
use crate::sensors::FEET_PER_NAUTICAL_MILE;

#[path = "strike_lase.rs"]
mod lase;
#[path = "strike_reach.rs"]
pub mod reach;
pub use reach::{ShotLaunch, ShotTable, Shots};
#[path = "strike_standoff.rs"]
mod standoff;
pub use lase::{
    LASE_BANK_MARGIN_DEG, LASE_CONE_EXIT_FT, LASE_MAX_TICKS, LASE_MIN_TICKS, LASE_RADIUS_MIN_FT,
    LASE_RADIUS_TURNS, LASE_REACH_SHARE, LASE_WAIT_TICKS, Lase, LaseEnd, LaseInputs, LaseSensor,
    LaseUntil, WingFacts, bank_cap_deg, lase_radius_ft, orbit_speed_fps, turn_rate_deg,
};
pub use standoff::{
    ADVANCE_TURNS, PLAN_SLACK_FT, ROUND_FALLBACK_FPS, ROUND_MARGIN_S, ReleasePlan, STANDOFF_EXIT_FT,
};

/// Feet in a metre, for John's release circle (given in metres).
pub const FEET_PER_METRE: f64 = 1. / 0.3048;
/// The pass is chosen this far from the target, and again before each
/// re-attack (spec, "Pass choice": 10 nm, fitted).
pub const CHOICE_DISTANCE_FT: f64 = 10. * FEET_PER_NAUTICAL_MILE;
/// Straight run: the run ends this close, horizontally (retail).
pub const STRAIGHT_EXIT_FT: f64 = 2_000.;
/// Pop-up: the approach point's height over the target, the pull's start,
/// its angle over the line of sight and its length, and where the run ends
/// (retail; the pull holds corner speed, not retail's minimum: spec,
/// "Retail quirks", opinionated agent).
pub const POPUP_OFFSET_FT: f64 = 500.;
pub const POPUP_PULL_AT_FT: f64 = 10_000.;
/// Extra pull distance for a bomb inheriting velocity rather than nose attitude.
pub const BOMB_POPUP_PULL_LEAD_FT: f64 = 1_000.;
pub const POPUP_PULL_DEG: f64 = 25.;
pub const POPUP_PULL_TICKS: u64 = 4 * 120;
pub const POPUP_EXIT_FT: f64 = 1_500.;
/// The pull goes on past its 4 s until the target is this far below the
/// horizon (fitted, agent): the AI's terrain floor (6 s of travel ahead)
/// allows only a shallow dive from a low pop, too shallow for rockets.
pub const POPUP_DIVE_DEG: f64 = 15.;
/// Rockets pop from farther out and pull until the target is farther down
/// (slice ST-T2, fitted, agent): they fly along the nose, so the dive after
/// the pop must point at the target from inside the pods' reach.
pub const ROCKET_POPUP_PULL_AT_FT: f64 = 16_000.;
pub const ROCKET_POPUP_DIVE_DEG: f64 = 15.;
/// The pop-up's approach heads for its pull point until this much short of
/// it (fitted).
pub const POPUP_LEAD_FT: f64 = 2_000.;
/// Fast-high: the approach point's height over the target and where the
/// run ends (retail).
pub const FAST_HIGH_OFFSET_FT: f64 = 5_000.;
pub const FAST_HIGH_EXIT_FT: f64 = 2_000.;
/// Dive bomb: the gate's floor, the pull-out floor, the climb after it
/// (retail).
pub const DIVE_GATE_MIN_FT: f64 = 10_000.;
pub const DIVE_PULL_OUT_MIN_FT: f64 = 2_000.;
pub const DIVE_CLIMB_DEG: f64 = 25.;
pub const DIVE_CLIMB_TICKS: u64 = 10 * 120;
/// The steepest dive any fighter or attack jet flies (retail `maxDive`).
pub const MAX_DIVE_DEG: f64 = 80.;
/// The approach heads for the roll-in point until this much short of it
/// (fitted).
pub const DIVE_ROLLIN_LEAD_FT: f64 = 2_000.;
/// The cue's lateral steering asks for this many times the bearing between
/// the cue and the aim, so the input-only adapter, which turns at a rate
/// proportional to the heading it is asked for, closes it faster (fitted).
pub const CUE_HEADING_GAIN: f64 = 1.;
/// Flying the cue (fitted, agent): beyond this far short of the aim the pass
/// flies its own line; the cue is flown to a point this margin short of the
/// aim with this gain on the angle; within the margin (and this tolerance)
/// the flight path is held and the cue creeps across.
pub const CUE_ACQUIRE_FT: f64 = 3_000.;
pub const CUE_MARGIN_FT: f64 = 600.;
pub const CUE_HOLD_TOLERANCE_FT: f64 = 300.;
pub const CUE_PITCH_GAIN: f64 = 3.;
/// A dive's cue margin (slice ST-T2, fitted, agent): the cue creeps faster
/// in a steep dive, so it is held this far short.
pub const DIVE_CUE_MARGIN_FT: f64 = 1_500.;
/// What the cue still lies off the aim's line (a pod's or gun's offset, a
/// crab in the wind) is trimmed out: the line is moved across by this share
/// per second of the cue's miss across, at most [`CUE_TRIM_LIMIT_FT`]
/// (slice ST-T2, fitted, agent). The passes whose cue sits close and steady
/// (straight run, pop-up, dive, strafe) use this; a level-like run points
/// its far-thrown cue instead ([`CUE_POINT_GAIN`]).
pub const CUE_TRACK_INTEGRAL: f64 = 0.5;
/// The trim learns only while the track is within this of its line.
pub const CUE_TRIM_ON_LINE_DEG: f64 = 1.5;
pub const CUE_TRIM_LIMIT_FT: f64 = 1_000.;
/// A level-like run (level, laydown, fast-high) points its far-thrown cue
/// at the aim: the track turns by this share of the angle that the cue's
/// miss across subtends from the aircraft (fitted, agent, nr-bombs
/// 2026-10-13). A level run's cue lies 10,000 ft or more ahead, so the trim
/// of [`CUE_TRACK_INTEGRAL`], which learns the drift through the adapter's
/// lag, swung the cue 180 ft either side of the aim for the whole of a fast
/// pass and put it 44 ft off at the release, outside the gate; the pointing
/// is one first-order loop on the miss itself and settles inside 4 s.
pub const CUE_POINT_GAIN: f64 = 0.6;
/// Fitted damping horizon for a velocity-based bomb cue, seconds.
pub const BOMB_CUE_DAMPING_S: f64 = 1.;
/// Fitted cue tracking gain for rolled bomb runs.
pub const BOMB_CUE_TRACK_GAIN: f64 = 4.;
/// A velocity-based bomb dive rolls out above the sight line, degrees.
pub const BOMB_ROLLIN_LEAD_DEG: f64 = 5.;
/// The cue's range the pointing divides by is never taken under this.
pub const CUE_POINT_MIN_RANGE_FT: f64 = 1_500.;
/// The input-only adapter closes a pitch error over 3 s (docs/spec/ai.md,
/// "Input-only aircraft control"); a request that would ask for less than
/// [`PUSH_G`] holds the wings level (fitted, agent).
pub const ADAPTER_PITCH_S: f64 = 3.;
pub const PUSH_G: f64 = 0.3;
/// Exit: the jink while inside this horizontal distance, its legs, their
/// length, the heading step and the climb (retail: 10 to 14 degrees; the
/// middle, 12, fitted); the climb beyond it; the turn back (retail).
pub const JINK_INSIDE_FT: f64 = 10_000.;
pub const JINK_LEGS: u64 = 4;
pub const JINK_LEG_TICKS: u64 = 3 * 120;
pub const JINK_STEP_DEG: f64 = 10.;
pub const JINK_CLIMB_DEG: f64 = 12.;
pub const EXIT_CLIMB_DEG: f64 = 10.;
pub const EXIT_TURN_MIN_FT: f64 = 30_000.;
/// A heavy's exit (retail: toward 50,000 ft away).
pub const HEAVY_EXIT_FT: f64 = 50_000.;
/// The exit climb levels off this far above the altitude the strike was
/// tasked at (fitted, agent): retail climbs on, which would lift every
/// re-attack 5,000 ft higher than the last.
pub const EXIT_CLIMB_ABOVE_FT: f64 = 2_000.;
/// The turn back is over once the nose is within this of the target
/// (fitted).
pub const TURN_BACK_DEG: f64 = 15.;
/// Release floors above the ground for dive and level releases (spec:
/// 3,000 ft under threat Clear or Air, 5,000 ft under Guns or Missiles).
pub const FLOOR_CLEAR_FT: f64 = 3_000.;
pub const FLOOR_DEFENDED_FT: f64 = 5_000.;
/// A level run releases down to this much below its floor, the ground
/// under it rising a little from the target's (slice ST-T2, fitted, agent).
pub const LEVEL_FLOOR_SLACK_FT: f64 = 200.;
/// A heavy's release floor: max(5,000 ft AGL, the mission's airborne
/// height), raised by threat, at most 90% of its ceiling (spec, "Heavy
/// bombers").
pub const HEAVY_MIN_FT: f64 = 5_000.;
pub const HEAVY_RAISE_GUNS_FT: f64 = 5_000.;
pub const HEAVY_RAISE_MISSILES_FT: f64 = 10_000.;
pub const HEAVY_CEILING_SHARE: f64 = 0.9;
/// John's release circle (2026-10-11): 90% of passes inside it.
pub const IN_CIRCLE_SHARE: f64 = 0.9;
/// The AI releases once the cue is within this share of its circle of the
/// drawn aim point, but never within less than [`GATE_FLOOR_FT`] (spec,
/// fitted). 0.05 of R suited the 100 m Ace; 10 m circles need the floor.
pub const GATE_SHARE: f64 = 0.1;
/// The least the release gate may be, feet: the cue creeps up to 7 ft a tick
/// across the aim, so a narrower gate steps over it (fitted, agent
/// 2026-10-11, with the tighter circles).
pub const GATE_FLOOR_FT: f64 = 10.;
/// Warnings count this long (spec: 10 s).
pub const WARNING_TICKS: u64 = 10 * 120;
/// The defend test: an aircraft inside 5 nm facing the striker (spec,
/// fitted), "facing" meaning within this of its nose (fitted, agent).
pub const DEFEND_RANGE_FT: f64 = 5. * FEET_PER_NAUTICAL_MILE;
pub const DEFEND_FACING_DEG: f64 = 45.;
/// The safe test (spec): no hostile fighter inside 15 nm.
pub const SAFE_FIGHTER_FT: f64 = 15. * FEET_PER_NAUTICAL_MILE;
/// Retail fires a guided round only inside 10,000 ft or half its zone-1
/// maximum range, whichever is farther.
pub const GUIDED_CLOSE_FT: f64 = 10_000.;
/// Rockets: at most this many from one pass, fired at the record's cadence
/// while the cue stays inside the circle (fitted, agent).
pub const ROCKETS_PER_PASS: u32 = 8;
/// Guided weapons and seekers: the run ends this close (fitted, the
/// straight run's).
pub const GUIDED_EXIT_FT: f64 = 2_000.;
/// The descent of a level run whose glide bomb cannot reach from its height
/// or speed, feet per second (fitted, agent).
pub const GLIDE_DESCENT_FPS: f64 = 400.;
/// A laser carrier waiting for a buddy's spot closes on the target until it
/// is this many fire ranges from it, then gives up (fitted, agent).
pub const SPOT_WAIT_RANGES: f64 = 1.5;
/// While every briefed target is fully attacked the striker re-chooses this
/// often (fitted).
pub const WAIT_TICKS: u64 = 2 * 120;
/// The committed-to-release window (T.O.R.E, John, 2026-10-28): an attack
/// run this many seconds or less from its computed release point is not
/// broken off by a new defence trigger; the striker releases, then reacts.
pub const COMMIT_WINDOW_S: f64 = 5.;
/// The longest the window holds a run, whatever the estimate says (fitted,
/// agent): a run that has not released by then defends.
pub const COMMIT_HOLD_TICKS: u64 = 8 * 120;
/// A cue this many seconds past its release point no longer holds the run
/// (fitted, agent).
pub const COMMIT_PAST_S: f64 = 0.5;
/// The time constant over which the cue's measured closing speed is
/// filtered, ticks (0.5 s; fitted, agent).
pub const CUE_RATE_TICKS: f64 = 60.;
/// A striker whose target dies before its pass releases sets up its next
/// pass at least this share of the exit distance from the new target
/// (T.O.R.E, John, 2026-10-28; the share fitted, agent).
pub const RETARGET_SETUP_SHARE: f64 = 0.5;
/// After an early retarget the striker climbs back to its cruise height
/// before it chooses the new pass, within this many feet (fitted, agent):
/// a straight run begun from the old run's low height flew the line to the
/// target down to where FRAG held its bomb (Vladivostok HQ, A-10).
pub const RETARGET_HEIGHT_SLACK_FT: f64 = 300.;
/// Class word bits of a SAM and an AAA unit (retail type `+0x0d`).
pub const CLASS_SAM: u16 = 0x1000;
pub const CLASS_AAA: u16 = 0x800;
/// A bomb dive rolls in once the aim is this far below the horizon (slice
/// ST-T2, fitted, agent): with the roll and pull ([`Handling::RollAndPull`])
/// the nose comes down about 15 degrees past it, so the dive settles near
/// 60 degrees, retail's roll-in geometry (the target 63 degrees down from
/// 2R high at R out) flown short of retail's 80 degree `maxDive`.
pub const DIVE_ROLLIN_DEG: f64 = 55.;
/// A dive or pop-up's roll-in is done once the flight path is within this
/// of the line to the target (slice ST-T2, fitted, agent); the cue is flown
/// from then on.
pub const ROLLIN_DONE_DEG: f64 = 5.;
/// A rocket dive rolls in at this depression (fitted, agent): rockets fly
/// along the nose, so a moderate dive keeps the slant inside the pods' reach.
pub const ROCKET_DIVE_ROLLIN_DEG: f64 = 25.;
/// A rocket dive starts from this height over the target (fitted, agent),
/// so the slant at the roll-in is inside the pods' 12,000 ft reach.
pub const ROCKET_DIVE_HEIGHT_FT: f64 = 6_000.;
/// The planned pull-out (slice ST-T2, fitted, agent): a dive is pulled out
/// at the steady G limit with this much reaction time, and bottoms out this
/// high above the highest ground ahead.
pub const PULL_OUT_REACTION_S: f64 = 1.;
pub const PULL_OUT_CLEARANCE_FT: f64 = 1_000.;
/// Strafing (slice ST-T2, fitted, agent): the run comes in this high, rolls
/// into a dive of this angle at the aim, opens fire inside the gun's reach
/// and ceases fire (then breaks off) at this slant range or height.
pub const STRAFE_HEIGHT_FT: f64 = 3_000.;
pub const STRAFE_DIVE_DEG: f64 = 15.;
pub const STRAFE_CEASE_FT: f64 = 2_000.;
pub const STRAFE_BREAK_AGL_FT: f64 = 500.;
/// A gun run's planned pull-out bottoms out this high over the highest ground
/// ahead (slice ST-G, fitted, agent): well under a bomb dive's
/// [`PULL_OUT_CLEARANCE_FT`], since the run breaks off at
/// [`STRAFE_BREAK_AGL_FT`] and a 1,000 ft clearance left the run unable to
/// keep the gun on the target below 1,300 ft.
pub const STRAFE_CLEARANCE_FT: f64 = 200.;
/// The strafe's roll-in is done once the flight path is within this of the
/// line to the aim.
pub const STRAFE_ROLLIN_DONE_DEG: f64 = 5.;
/// Gun tracking: the flight path turns by this many times the angle between
/// the gun's ground cue and the burst's aim point.
pub const GUN_PITCH_GAIN: f64 = 3.;
/// A pilot's fine tracking (slice ST-G, opinionated, agent): a burst opens,
/// goes on and is laid exactly on its walked aim while the gun's ground cue
/// lies within this angle of the target, never less than
/// [`GUN_GATE_MIN_FT`] across. The flown cue cannot follow a point to a few
/// feet (the steering is a pitch gain and a heading law); the gunner's last
/// refinement stands for the rest.
pub const GUN_TRACK_DEG: f64 = 1.;
pub const GUN_GATE_MIN_FT: f64 = 25.;
/// Drag (retarded) bombs fly a low level pass (slice ST-T2, fitted, agent):
/// never below this height, down to it by this far short of the aim. The
/// release height also clears the fuze's arming time by [`LOW_MARGIN`] and
/// the store's fragments by [`FRAG_MARGIN`] (the sight's LOW and FRAG
/// rules, docs/spec/air-to-ground.md).
pub const LAYDOWN_MIN_FT: f64 = 500.;
pub const LAYDOWN_LEAD_FT: f64 = 8_000.;
pub const LOW_MARGIN: f64 = 1.2;
pub const FRAG_MARGIN: f64 = 1.1;

/// How the adapter flies a strike steer (slice ST-T2). Only a striker's
/// attack segments ask for more than [`Handling::PLAIN`]; every other AI
/// motion is flown as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Handling {
    /// Steer the ground-velocity heading, as a velocity-based bomb cue needs.
    pub track_heading: bool,
    /// Roll the lift onto the wanted direction and pull: a nose-down change
    /// of more than a gentle push rolls inverted and pulls (retail's dive
    /// bomb roll-in), a smaller one unloads or pushes wings level. Flown for
    /// a roll-in and a pull-out, not while the cue is flown (a banked pull
    /// swings the nose, and with it the bomb's cue, across the track).
    pub pull: bool,
    /// A planned dive: the terrain floor is the dive's planned pull-out
    /// ([`planned_pitch_floor_deg`]) instead of the general one, so the dive
    /// can be steep and still recover.
    pub planned_dive: bool,
    /// A gun run's planned dive (slice ST-G): the same floor with the gun
    /// run's own clearance, [`STRAFE_CLEARANCE_FT`], so it can press the
    /// attack down to the break-off height.
    pub strafe: bool,
}

impl Handling {
    /// The ordinary input-only adapter.
    pub const PLAIN: Self = Self {
        track_heading: false,
        pull: false,
        planned_dive: false,
        strafe: false,
    };
    pub const PULL: Self = Self {
        track_heading: false,
        pull: true,
        planned_dive: false,
        strafe: false,
    };
    /// A gun run's tracking (slice ST-G).
    pub const STRAFE: Self = Self {
        track_heading: false,
        pull: true,
        planned_dive: true,
        strafe: true,
    };
}

/// A pass (spec, "Pass choice").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Straight,
    PopUp,
    FastHigh,
    Dive,
    Level,
    /// The gunship's left-hand orbit about the target (slice ST-F).
    Orbit,
    /// A helicopter's stand-off attack: low to the weapon's range, unmask,
    /// fire, mask (slice ST-F).
    StandOff,
    /// A low level pass for drag (retarded) bombs (slice ST-T2).
    Laydown,
    /// A gun run: a shallow dive with bursts (slice ST-T2).
    Strafe,
}

impl Pass {
    pub const ALL: [Self; 9] = [
        Self::Straight,
        Self::PopUp,
        Self::FastHigh,
        Self::Dive,
        Self::Level,
        Self::Orbit,
        Self::StandOff,
        Self::Laydown,
        Self::Strafe,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Straight => "straight",
            Self::PopUp => "popup",
            Self::FastHigh => "fasthigh",
            Self::Dive => "dive",
            Self::Level => "level",
            Self::Orbit => "orbit",
            Self::StandOff => "standoff",
            Self::Laydown => "laydown",
            Self::Strafe => "strafe",
        }
    }

    /// A pass whose bomb cue lies far ahead (10,000 ft or more) and steers
    /// by pointing it at the aim ([`CUE_POINT_GAIN`]).
    pub fn points_cue(self) -> bool {
        matches!(self, Self::Level | Self::Laydown | Self::FastHigh)
    }
}

/// What a store is to the strike (spec, "Weapons and releases").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    /// An unguided bomb or a pack.
    Bomb,
    /// A rocket pod.
    Rocket,
    /// A laser bomb or missile.
    Laser,
    /// A JDAM.
    Coordinate,
    /// The AT-2: the helicopters' (slice ST-F), never chosen here.
    Command,
    /// A missile with its own seeker: Mavericks, AS-14, AS-30.
    Seeker,
    /// A radar air-to-ground missile (AS-7, AS-15, AGM-84A, AM-39, AS-16).
    RadarAgm,
    /// HARM and the other anti-radiation missiles.
    AntiRadiation,
    /// A gun: one of the gunship's linked guns (slice ST-F; flown by its
    /// family, never chosen by the fixed-wing table) or a fixed-wing
    /// striker's gun for strafing (slice ST-T2).
    Gun,
}

impl StoreKind {
    /// Unguided: the skill circle applies (John, 2026-10-11; to each gun
    /// burst, John, 2026-10-11, later).
    pub fn unguided(self) -> bool {
        matches!(self, Self::Bomb | Self::Rocket | Self::Gun)
    }
    /// Released on a seeker's lock through the AI's own missile release.
    pub fn seeker(self) -> bool {
        matches!(self, Self::Seeker | Self::RadarAgm | Self::AntiRadiation)
    }
    /// Flown to on a level run (spec, "Other weapons").
    pub fn level_run(self) -> bool {
        matches!(self, Self::Laser | Self::Coordinate)
    }
}

/// One store as the striker sees it: the host describes its kind, the
/// mission counts its rounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Store {
    pub station: StationId,
    pub kind: StoreKind,
    pub rounds: u32,
    /// Ticks between two rounds of a burst (rockets), at least one quarter
    /// second.
    pub cadence_ticks: u64,
    /// The farthest the AI fires it (a seeker: retail's guided rule), feet.
    pub fire_range_ft: f64,
    /// Its zone-1 minimum and maximum ranges, feet: an unguided round is
    /// released only with its cue inside them (retail zone 1).
    pub minimum_range_ft: f64,
    pub maximum_range_ft: f64,
    /// A rocket's speed for its cue's drop, ft/s; `None` for a bomb, which
    /// leaves at the aircraft's speed.
    pub round_speed_fps: Option<f64>,
    /// The record's launch-retard share, retained to classify laydown stores.
    /// Bomb release velocity is no longer scaled by this value.
    pub release_speed_share: f64,
    pub bomb_body: crate::combat::bomb_flight::Body,
    /// The fuze's arming time, seconds, and the fragments' reach, feet (the
    /// sight's LOW and FRAG rules).
    pub fuze_arm_s: f64,
    pub frag_radius_ft: f64,
    /// A laser store that falls on the bomb equation when it is let go with
    /// no spot (a laser glide bomb, not a laser missile): it can be dropped
    /// by hand as an unguided bomb (slice ST-G).
    pub falls_free: bool,
    /// A guided store's time of flight to a ground target, from the host's
    /// estimate (a helicopter's stand-off advance, John 2026-10-29); `None`
    /// when the host gave none.
    pub flight: Option<FlightTimes>,
}

impl Store {
    /// A drag (retarded) bomb: its retarder slows it after release.
    pub fn retarded(&self) -> bool {
        self.kind == StoreKind::Bomb && self.release_speed_share < 0.95
    }
}

/// A guided store's time of flight to a ground target at a few ranges, from
/// the host's energy-model estimate (or its record's speed under the other
/// rules): `points` are (range feet, seconds), ranges rising.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlightTimes {
    pub points: [(f64, f64); 4],
}

impl FlightTimes {
    /// The time of flight to `range_ft`, seconds: along the straight lines
    /// between the points (from no time at no range), and past the last at
    /// the last stretch's pace.
    pub fn seconds_to(&self, range_ft: f64) -> f64 {
        let range = range_ft.max(0.);
        let mut previous = (0., 0.);
        for point in self.points {
            if range <= point.0 {
                return lerp(previous, point, range);
            }
            previous = point;
        }
        let [.., before, last] = self.points;
        lerp(before, last, range)
    }
}

/// The straight line through `a` and `b` (range, seconds) at `range`.
fn lerp(a: (f64, f64), b: (f64, f64), range: f64) -> f64 {
    let span = b.0 - a.0;
    if span <= 0. {
        return b.1;
    }
    a.1 + (b.1 - a.1) * (range - a.0) / span
}

/// What the host worked out for the striker's selection this tick, from its
/// strike cockpit: the same solutions a human's HUD shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solution {
    pub station: StationId,
    pub target: u32,
    /// Where the target stands, and whether it still stands.
    pub target_position: Vector,
    pub target_velocity: Vector,
    pub target_alive: bool,
    /// The cue's predicted impact (bomb CCIP, which the LVL cue is timed on,
    /// or the rocket's), for unguided stores.
    pub impact: Option<Vector>,
    /// Seconds from release to that impact.
    pub impact_seconds: Option<f64>,
    /// The shared release test passes now: the cue within the gate of the
    /// aim point (unguided), the guided reach, or a seeker's lock.
    pub due: bool,
    /// The sight warns LOW or FRAG: the AI does not pickle (fitted, agent).
    pub warning: bool,
    /// One of this aircraft's guided rounds is flying at the target (retail
    /// fires one guided round at a time).
    pub guided_in_flight: bool,
    /// The guided weapon's glide can reach a point ahead from here at all
    /// (the sight has a reach); true for every other store. A glide bomb
    /// released from too high or too slow runs out of life before it lands.
    pub reach_ok: bool,
    /// Seconds to the release where the host's sight times one (a guided
    /// bomb: its distance to the level T's bar over the ground speed),
    /// negative once past it; `None` for every other store, whose time the
    /// strike works out itself (the committed-to-release window).
    pub release_seconds: Option<f64>,
    /// The store would reach the target from here, whatever the AI's own
    /// release range: a seeker's lock without the fire range, a guided
    /// weapon's IN RNG with no advisory (under the energy rules, its
    /// exact-point answer). A stand-off release planned outside known rings
    /// leaves on it (John, 2026-10-29).
    pub reaches: bool,
    /// The ground range of the guided sight's release bar, feet: where a
    /// guided bomb is due (none for other stores, or with no reach).
    pub bar_ft: Option<f64>,
}

/// The host's per-step description (see the module notes).
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub class: StrikeClass,
    pub stores: Vec<Store>,
    /// A laser designator of its own: a spot is assured, so laser stores
    /// may be chosen (spec, decision 9).
    pub designator: bool,
    /// The mission's airborne height above the ground, for a heavy's floor
    /// (Quick Mission's altitude setting); `None` when unknown.
    pub mission_height_ft: Option<f64>,
    pub solution: Option<Solution>,
    /// The laser and the spots around it (slice ST-L).
    pub lase: LaseInputs,
    /// The host's answers on a guided store's shots from farther out (a
    /// stand-off release outside known rings, John 2026-10-29); none when
    /// it gave none.
    pub shots: Shots,
}

/// Where the striker flies this tick: a heading and a flight-path pitch,
/// degrees, and a speed (the ordinary adapter flies it with its controls).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steer {
    pub heading_deg: f64,
    pub pitch_deg: f64,
    pub speed: SpeedRequest,
    pub afterburner: bool,
    /// A bank to hold, degrees (right positive), instead of the bank the
    /// adapter picks for the heading: the gunship's orbit flies its bank
    /// (slice ST-F). `None` everywhere else.
    pub bank_deg: Option<f64>,
    pub handling: Handling,
}

impl Steer {
    /// A steer the ordinary adapter flies.
    pub fn plain(heading_deg: f64, pitch_deg: f64, speed: SpeedRequest, afterburner: bool) -> Self {
        Self {
            heading_deg,
            pitch_deg,
            speed,
            afterburner,
            handling: Handling::PLAIN,
            bank_deg: None,
        }
    }
}

/// A store to release now, at the striker's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Release {
    pub station: StationId,
    pub target: u32,
}

/// One step's answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Order {
    /// `None` hands the flying to the rest of the controller (defend, home).
    pub steer: Option<Steer>,
    pub release: Option<Release>,
    pub activity: Option<Activity>,
}

/// What the strike cockpit is to hold for the host: the target, the store,
/// the bomb sight's mode, the aim and whether the laser is wanted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    pub target: u32,
    pub station: StationId,
    pub kind: StoreKind,
    pub mode: DeliveryMode,
    /// The aim offset from the target, feet along x and z, and the release
    /// gate, feet (zero offset for a guided store).
    pub aim_ft: [f64; 2],
    pub gate_ft: f64,
    /// The laser is wanted on now, with a designator of its own (slice
    /// ST-L): the pass of a laser store, or its hold.
    pub laser: bool,
}

/// The step's world, as the mission hands it over.
pub struct Context<'a> {
    pub tick: u64,
    pub aircraft: u32,
    pub side: Side,
    pub own: &'a OwnState,
    pub velocity: Vector,
    pub picture: &'a GroundPicture,
    pub inputs: &'a Inputs,
    pub experience: Experience,
    pub warnings: RecentWarnings,
    /// The controller flies a B47 defence this step (a missile or gunfire).
    pub defending: bool,
    /// Fuel, damage or the mission sends it home.
    pub recovering: bool,
    /// The strike targets of the other members of its wing.
    pub mates: &'a [u32],
    /// What its wing's designators and laser carriers are doing (slice ST-L).
    pub wing: WingFacts,
    /// The aircraft's ceiling, feet above sea level.
    pub ceiling_ft: f64,
    /// The terrain height, for a helicopter's masked hold-off (slice H4);
    /// `None` masks nothing.
    pub terrain: Option<&'a dyn Fn(f64, f64) -> f64>,
    /// The compass bearing of the threat a helicopter is breaking from, when
    /// known (slice H4, the ambush ring).
    pub threat_bearing_deg: Option<f64>,
}

/// The strike's phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Toward the target until the choice point.
    Ingress,
    /// Too slow under threat: level at maximum speed until energy is Good.
    Regain,
    /// Flying a pass.
    Attack,
    /// Away from the target after a pass.
    Exit,
    /// Back toward the target to choose again.
    TurnBack,
    /// Every briefed target is fully attacked: wait for one.
    Wait,
    /// The air logic flies a defence.
    Defend,
    /// Done: no target or no store left, or sent home.
    Home,
    /// Holding a laser spot on a target in an offset turn ([`lase`]).
    Lase,
    /// A helicopter holds off outside a threat's ring (slice H4,
    /// [`super::helicopter_threat`]).
    HoldOff,
}

/// A pass's stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Toward the pass's approach point.
    Approach,
    /// The pop-up's pull.
    Pull,
    /// The cue flown onto the aim point; the release.
    Final,
    /// The dive's pull-out climb.
    Climb,
    /// A helicopter holds the line to its command-guided round until it
    /// has hit or is gone (slice ST-F).
    Guide,
}

/// What an AI with laser stores and no laser support does (John,
/// 2026-10-12): drop a laser bomb by hand as an unguided bomb, or, with a
/// laser store that cannot fall unguided (a laser missile), go home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoLaze {
    /// "No laze support, dropping manual".
    Manual,
    /// "No laze support, RTB".
    Rtb,
}

impl NoLaze {
    /// The reason the replay's `ai.strike` event records.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Manual => "no_laze_manual",
            Self::Rtb => "no_laze_rtb",
        }
    }
    /// The words on the radio.
    pub fn words(self) -> &'static str {
        match self {
            Self::Manual => "No laze support, dropping manual",
            Self::Rtb => "No laze support, RTB",
        }
    }
}

/// A no-laze decision (slice ST-G): what was decided, for which laser store
/// and target, and whether the radio has said so.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoLazeCall {
    pub kind: NoLaze,
    pub station: StationId,
    pub target: Option<u32>,
    pub told: bool,
}

/// One striker's strike state (see the module notes).
#[derive(Clone, Debug, PartialEq)]
pub struct Strike {
    /// The aircraft's decision seed, which the mission sets from its roster
    /// place: the skill draw's "mission seed".
    seed: u64,
    phase: Phase,
    segment: Segment,
    /// The tick the phase or segment began.
    since: u64,
    pass: Option<Pass>,
    /// The driver's forced pass and store (coverage only).
    forced: Option<Pass>,
    forced_station: Option<StationId>,
    target: Option<u32>,
    station: Option<StationId>,
    kind: Option<StoreKind>,
    /// Passes flown at the current target, from 1.
    pass_number: u32,
    aim_ft: [f64; 2],
    /// The lead on a moving target: its ground velocity times the cue's
    /// time of fall, feet along x and z (fitted, agent).
    lead_ft: [f64; 2],
    gate_ft: f64,
    /// The drawn offset lies inside the circle.
    inside: bool,
    /// The final holds this flight-path pitch, degrees, while the cue
    /// creeps across the aim; `None` while it is still flying the cue.
    holding: Option<f64>,
    /// The altitude a level run or a hold flies at, feet above sea level.
    hold_ft: Option<f64>,
    /// Rounds released on this pass and the tick the next may go.
    released: u32,
    next_release: u64,
    /// The heading the exit holds and the jink's side (+1 right, -1 left).
    exit_heading_deg: f64,
    jink_side: f64,
    /// The altitude the strike was tasked at: the exit climbs no higher
    /// than [`EXIT_CLIMB_ABOVE_FT`] over it.
    cruise_ft: Option<f64>,
    /// The last tick a warning sounded.
    warned: Option<u64>,
    /// The threat level when the pass was chosen.
    threat: ThreatLevel,
    /// It left after a defence because it was not safe: at the exit
    /// distance it resumes only if safe, else goes home.
    withdrawn: bool,
    /// The human's attack this strike continues (slice ST-K), if any.
    takeover: Option<Takeover>,
    /// The lase in progress: the hold after its own laser bomb, a duty, or
    /// the wing's (slice ST-L).
    lase: Option<Lase>,
    /// It has a laser designator of its own (last step's inputs).
    designator: bool,
    /// It has a laser store with rounds left (last step's inputs).
    laser_rounds: bool,
    /// The AI turned the laser on by hand ([`Strike::laser_set`]).
    laser_set: bool,
    /// It has laser stores and no laser support: it drops them by hand or
    /// goes home (slice ST-G).
    no_laze: Option<NoLazeCall>,
    /// A gun run's bursts (slice ST-T2): the burst being aimed or fired
    /// (from 1 each pass), the tick it opened, and the end of the recovery
    /// after the last one.
    burst: u32,
    burst_open: Option<u64>,
    recovery_until: u64,
    /// The trim across the aim's line, feet (see [`CUE_TRACK_INTEGRAL`]).
    cue_across_sum: f64,
    /// Last tick's cue against the drawn aim: where the cue lay from it,
    /// feet along x and z, and whether the cue was still closing on it then
    /// (see [`Strike::cue_passing`]).
    cue_last: Option<([f64; 2], bool)>,
    /// The striker's height above the ground when it was first tasked: a
    /// heavy's "mission airborne height" when the host gives none.
    tasked_agl_ft: Option<f64>,
    /// A helicopter's threat awareness (slice H4).
    heli: super::helicopter_threat::HeliThreat,
    /// The unguided cue against the release point at the last attack step:
    /// the tick, how far it lay short of the aim along the track (feet) and
    /// its filtered closing speed (ft/s).
    release_cue: Option<(u64, f64, f64)>,
    /// Seconds to the release point at the last attack step, negative once
    /// past it; `None` with no estimate.
    release_in_s: Option<f64>,
    /// The tick the committed-to-release window opened on this pass.
    committed: Option<u64>,
    /// The exit or turn back follows an early retarget: the exit turns back
    /// at the set-up distance ([`RETARGET_SETUP_SHARE`]), not the full exit
    /// distance, and the turn back chooses only once at its height
    /// ([`RETARGET_HEIGHT_SLACK_FT`]) or inside the set-up distance.
    retargeted: bool,
    /// The pass's stand-off release outside known rings (John, 2026-10-29;
    /// [`standoff`]), kept after the release for the exit.
    release_plan: Option<ReleasePlan>,
    /// The pass closed past its plan without releasing: it is flown as
    /// before.
    plan_dropped: bool,
    /// The tick its last stand-off round is taken to land by.
    standoff_until: Option<u64>,
    /// The host's inputs for this step, consumed by it.
    inputs: Option<Inputs>,
}

impl Strike {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            phase: Phase::Ingress,
            segment: Segment::Approach,
            since: 0,
            pass: None,
            forced: None,
            forced_station: None,
            target: None,
            station: None,
            kind: None,
            pass_number: 0,
            aim_ft: [0.; 2],
            lead_ft: [0.; 2],
            gate_ft: 0.,
            inside: true,
            holding: None,
            hold_ft: None,
            released: 0,
            next_release: 0,
            exit_heading_deg: 0.,
            jink_side: 1.,
            cruise_ft: None,
            warned: None,
            threat: ThreatLevel::Clear,
            withdrawn: false,
            takeover: None,
            lase: None,
            designator: false,
            laser_rounds: false,
            laser_set: false,
            no_laze: None,
            burst: 0,
            burst_open: None,
            recovery_until: 0,
            cue_across_sum: 0.,
            cue_last: None,
            tasked_agl_ft: None,
            heli: Default::default(),
            release_cue: None,
            release_in_s: None,
            committed: None,
            retargeted: false,
            release_plan: None,
            plan_dropped: false,
            standoff_until: None,
            inputs: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn segment(&self) -> Segment {
        self.segment
    }
    /// The pass being flown (or last flown), for the driver's report.
    pub fn pass(&self) -> Option<Pass> {
        self.pass
    }
    pub fn target(&self) -> Option<u32> {
        self.target
    }
    pub fn pass_number(&self) -> u32 {
        self.pass_number
    }
    /// The drawn aim offset, feet, and whether it lies inside the circle.
    pub fn aim(&self) -> ([f64; 2], bool) {
        (self.aim_ft, self.inside)
    }
    /// The lead on a moving target for the current pass: its ground
    /// velocity times the cue's time of fall, feet along x and z.
    pub fn lead(&self) -> [f64; 2] {
        self.lead_ft
    }
    /// The gun burst's aim offset now, feet along x and z: the drawn start
    /// offset while the burst is unopened, walking to nothing over the
    /// skill's walk-in time once it has opened (slice ST-G).
    pub fn burst_aim(&self, tick: u64, level: Experience) -> [f64; 2] {
        let left = match (self.kind, self.burst_open) {
            (Some(StoreKind::Gun), Some(start)) => walk_in_left(level, tick.saturating_sub(start)),
            _ => 1.,
        };
        [self.aim_ft[0] * left, self.aim_ft[1] * left]
    }
    /// A gun run's burst being aimed or fired (from 1 each pass), 0 for any
    /// other store.
    pub fn burst(&self) -> u32 {
        self.burst
    }
    /// The no-laze decision in force, if any (slice ST-G).
    pub fn no_laze(&self) -> Option<&NoLazeCall> {
        self.no_laze.as_ref()
    }
    /// The radio has said the no-laze decision; true if there was one to say.
    pub fn told_no_laze(&mut self) -> bool {
        match self.no_laze.as_mut() {
            Some(call) if !call.told => {
                call.told = true;
                true
            }
            _ => false,
        }
    }
    /// A helicopter's last threat decision (slice H4), if any.
    pub fn threat_call(&self) -> Option<&super::helicopter_threat::Call> {
        self.heli.call.as_ref()
    }
    /// The radio has said the threat decision; the decision if there was
    /// one still to say.
    pub fn told_threat_call(&mut self) -> Option<super::helicopter_threat::Decision> {
        match self.heli.call.as_mut() {
            Some(call) if !call.told => {
                call.told = true;
                Some(call.decision)
            }
            _ => None,
        }
    }
    /// Where a helicopter holding off circles, and since when.
    pub fn hold_off(&self) -> Option<(Vector, u64)> {
        self.heli.hold
    }
    /// A helicopter's move-ahead ambush in hand (John, 2026-10-29).
    pub fn move_ahead(&self) -> Option<&super::helicopter_ahead::Ahead> {
        self.heli.ahead.as_ref()
    }
    /// Done: the mission sends it home.
    pub fn home(&self) -> bool {
        self.phase == Phase::Home
    }
    /// The run is inside its committed-to-release window at `tick` (T.O.R.E,
    /// John, 2026-10-28): a new defence trigger waits for the release, so
    /// the mission holds the controller's defence motion back.
    pub fn holds_run(&self, tick: u64) -> bool {
        self.phase == Phase::Attack
            && self
                .committed
                .is_some_and(|since| tick.saturating_sub(since) <= COMMIT_HOLD_TICKS)
    }
    /// Seconds to the release point at the last attack step, negative once
    /// past it (the driver's trace).
    pub fn release_in_s(&self) -> Option<f64> {
        self.release_in_s
    }
    /// Forces the pass and the store (the headless driver's coverage
    /// runs); the real choice is [`choose_pass`].
    pub fn force(&mut self, pass: Option<Pass>, station: Option<StationId>) {
        self.forced = pass;
        self.forced_station = station;
    }
    /// Weapons are held: any pass in hand is dropped and the strike starts
    /// again from its ingress (a striker sent home stays home).
    pub fn stand_down(&mut self, tick: u64) {
        if !matches!(self.phase, Phase::Home | Phase::Ingress) {
            self.enter(Phase::Ingress, tick);
            self.released = 0;
            self.holding = None;
            self.burst_open = None;
        }
    }

    /// The host's description for the next step.
    pub fn set_inputs(&mut self, inputs: Inputs) {
        self.inputs = Some(inputs);
    }
    pub fn inputs(&self) -> Option<&Inputs> {
        self.inputs.as_ref()
    }
    pub(super) fn take_inputs(&mut self) -> Option<Inputs> {
        self.inputs.take()
    }

    /// What the strike cockpit must hold for the host to solve the next
    /// step.
    pub fn selection(&self) -> Option<Selection> {
        if matches!(self.phase, Phase::Home | Phase::Wait | Phase::HoldOff) {
            return None;
        }
        let (target, station, kind) = (self.target?, self.station?, self.kind?);
        let level = self.pass == Some(Pass::Level) && kind == StoreKind::Bomb;
        Some(Selection {
            target,
            station,
            kind,
            mode: if level {
                DeliveryMode::Level
            } else {
                DeliveryMode::Ccip
            },
            aim_ft: if kind.unguided() {
                [
                    self.aim_ft[0] + self.lead_ft[0],
                    self.aim_ft[1] + self.lead_ft[1],
                ]
            } else {
                [0.; 2]
            },
            gate_ft: self.gate_ft,
            laser: kind == StoreKind::Laser && self.laser_target().is_some(),
        })
    }

    fn enter(&mut self, phase: Phase, tick: u64) {
        self.phase = phase;
        self.segment = Segment::Approach;
        self.since = tick;
        // The release estimate and its window belong to one pass, the
        // retarget's set-up to the exit and turn back that follow it.
        if !matches!(phase, Phase::Exit | Phase::TurnBack) {
            self.retargeted = false;
        }
        self.release_cue = None;
        self.release_in_s = None;
        self.committed = None;
    }

    fn segment_to(&mut self, segment: Segment, tick: u64) {
        self.segment = segment;
        self.since = tick;
    }

    /// One mission step (see the module notes).
    pub fn step(&mut self, cx: &Context<'_>) -> Order {
        let own = cx.own;
        let mut order = Order {
            steer: None,
            release: None,
            activity: None,
        };
        if cx.warnings.air_to_air || cx.warnings.surface_to_air {
            self.warned = Some(cx.tick);
        }
        if self.cruise_ft.is_none() {
            self.cruise_ft = Some(own.position[1]);
        }
        self.designator = cx.inputs.designator;
        self.laser_rounds = cx
            .inputs
            .stores
            .iter()
            .any(|s| s.kind == StoreKind::Laser && s.rounds > 0);
        if self.tasked_agl_ft.is_none() {
            self.tasked_agl_ft = Some(own.agl_ft);
        }
        if cx.recovering {
            self.phase = Phase::Home;
            self.lase = None;
        }
        if self.phase == Phase::Home {
            return order;
        }
        // Helicopters and the gunship fly their own families (slice ST-F).
        match cx.inputs.class {
            StrikeClass::Helicopter => return helicopter::step(self, cx),
            StrikeClass::Gunship => return gunship::step(self, cx),
            _ => {}
        }
        // Defend: the existing reactions fly; resume at ingress once the
        // threat has gone (no warning in 10 s, no fighter threatening).
        // Inside the committed-to-release window a new trigger waits for the
        // release (T.O.R.E, John, 2026-10-28).
        let committed = self.holds_run(cx.tick);
        if (cx.defending || self.threatened(cx)) && !committed {
            if self.phase != Phase::Defend {
                self.enter(Phase::Defend, cx.tick);
                self.released = 0;
                // A threat ends a lase (spec, "Lasing"): the weapon on the
                // spot goes blind, as for a player who stops lasing.
                self.lase = None;
            }
            return order;
        }
        if self.phase == Phase::Defend {
            if !self.quiet(cx) {
                return order;
            }
            // The threat has gone: resume when safe, else egress and look
            // again from the exit distance (spec, "Defend" and "Safe").
            if self.safe(cx) {
                self.enter(Phase::Ingress, cx.tick);
            } else {
                self.withdrawn = true;
                self.exit(cx);
            }
        }
        // A takeover goes on only while safe (slice ST-K); a committed run
        // releases first.
        if !committed {
            self.recheck_safe(cx);
        }
        if let Some(order) = self.step_lase(cx) {
            return order;
        }
        let solution = self.solution(cx);
        let target_alive = match (self.target, solution) {
            (Some(_), Some(s)) => s.target_alive,
            (Some(id), None) => standing(cx.picture, id),
            (None, _) => false,
        };
        // A pass in flight ends when its target falls: before its release
        // the striker takes the next target at once (T.O.R.E, John,
        // 2026-10-28), after it, or with none left, it exits.
        if self.phase == Phase::Attack && !target_alive && !self.retarget(cx) {
            self.exit(cx);
        }
        let solution = self.solution(cx);
        let target_at = solution
            .map(|s| s.target_position)
            .or_else(|| self.focus_position(cx));
        match self.phase {
            Phase::Ingress | Phase::Wait => {
                let point = target_at.or_else(|| objective_point(cx.picture));
                let near = point.is_none_or(|p| horizontal(own.position, p) <= CHOICE_DISTANCE_FT);
                let waited =
                    self.phase != Phase::Wait || cx.tick.saturating_sub(self.since) >= WAIT_TICKS;
                if near && waited {
                    self.choose(cx);
                }
            }
            Phase::Regain => {
                if energy_good(own) {
                    self.choose(cx);
                }
            }
            Phase::TurnBack => {
                if let Some(point) = target_at {
                    let off =
                        angle_difference(own.heading_deg, bearing_deg(own.position, point)).abs();
                    if off <= TURN_BACK_DEG && (!self.retargeted || self.set_up(cx, point)) {
                        self.choose(cx);
                    }
                } else {
                    self.choose(cx);
                }
            }
            Phase::Exit => {
                if let Some(point) = target_at {
                    let away = horizontal(own.position, point);
                    let turn_back = if self.retargeted {
                        RETARGET_SETUP_SHARE * self.exit_distance(cx)
                    } else {
                        self.exit_distance(cx)
                    };
                    if away >= turn_back {
                        if !self.withdrawn {
                            self.enter(Phase::TurnBack, cx.tick);
                        } else if self.safe(cx) {
                            self.withdrawn = false;
                            self.end_stand_off();
                            self.enter(Phase::TurnBack, cx.tick);
                        } else if !self.begin_stand_off(cx, point) {
                            // A takeover stands off instead (slice ST-K).
                            self.enter(Phase::Home, cx.tick);
                        }
                    }
                }
            }
            Phase::Attack | Phase::Defend | Phase::Home | Phase::Lase | Phase::HoldOff => {}
        }
        // The choice may have changed the target or ended the strike.
        let solution = self.solution(cx);
        let target_at = solution
            .map(|s| s.target_position)
            .or_else(|| self.focus_position(cx));
        match self.phase {
            Phase::Home | Phase::Defend | Phase::Lase | Phase::HoldOff => order,
            Phase::Ingress | Phase::Wait => {
                let point = target_at
                    .or_else(|| objective_point(cx.picture))
                    .or_else(|| {
                        self.spot_wait_point(cx)
                            .filter(|_| self.phase == Phase::Wait)
                    });
                order.steer = point.map(|p| self.cruise_steer(cx, p, SpeedRequest::Corner));
                order.activity = Some(Activity::Pursuing);
                order
            }
            Phase::Regain => {
                order.steer = Some(Steer::plain(
                    own.heading_deg,
                    0.,
                    SpeedRequest::Maximum,
                    true,
                ));
                order.activity = Some(Activity::Pursuing);
                order
            }
            Phase::TurnBack => {
                order.steer = target_at.map(|p| self.cruise_steer(cx, p, SpeedRequest::Corner));
                order.activity = Some(Activity::Pursuing);
                order
            }
            Phase::Exit => {
                // A takeover's stand-off and guiding hold (slice ST-K).
                let steer = self
                    .stand_off_steer(cx, target_at)
                    .or_else(|| self.guide_steer(cx))
                    .unwrap_or_else(|| self.exit_steer(cx, target_at));
                order.steer = Some(steer);
                order.activity = Some(Activity::Attacking);
                order
            }
            Phase::Attack => match target_at {
                Some(point) => self.attack(cx, point, solution),
                None => {
                    self.exit(cx);
                    order.steer = Some(self.exit_steer(cx, None));
                    order
                }
            },
        }
    }

    /// Where the picture knows the striker's target stands, or the target of
    /// its lase duty when it has no other.
    fn focus_position(&self, cx: &Context<'_>) -> Option<Vector> {
        self.target
            .or_else(|| self.lase.map(|l| l.target))
            .and_then(|id| known_position(cx.picture, id))
    }

    /// The host's solution, when it is for the current selection.
    fn solution(&self, cx: &Context<'_>) -> Option<Solution> {
        cx.inputs
            .solution
            .filter(|s| Some(s.target) == self.target && Some(s.station) == self.station)
    }

    /// Whether something the striker must answer threatens it now: an
    /// aircraft inside 5 nm facing it (spec, "Defend"). Missile and gun
    /// defences arrive as `defending`.
    fn threatened(&self, cx: &Context<'_>) -> bool {
        cx.picture
            .threats()
            .fighters
            .iter()
            .any(|f| f.distance_ft <= DEFEND_RANGE_FT && f.off_nose_deg <= DEFEND_FACING_DEG)
    }

    /// No warning in the last 10 s.
    fn quiet(&self, cx: &Context<'_>) -> bool {
        self.warned
            .is_none_or(|t| cx.tick.saturating_sub(t) >= WARNING_TICKS)
    }

    /// The spec's safe test (confirmed by John, 2026-10-11): no warning in
    /// 10 s, no known hostile SAM covering the target, and no hostile fighter
    /// inside 15 nm.
    pub fn safe(&self, cx: &Context<'_>) -> bool {
        let target = self
            .target
            .and_then(|id| known_position(cx.picture, id))
            .or_else(|| cx.inputs.solution.map(|s| s.target_position));
        self.quiet(cx)
            && !cx
                .picture
                .threats()
                .fighters
                .iter()
                .any(|f| f.distance_ft <= SAFE_FIGHTER_FT)
            && target.is_none_or(|t| {
                !cx.picture
                    .threats()
                    .hostile_defenses(cx.side)
                    .any(|d| d.covers_with_missiles(t))
            })
    }

    /// The choice: target, store, pass and aim; or wait, or home.
    fn choose(&mut self, cx: &Context<'_>) {
        self.choose_without(cx, None);
    }

    /// [`Self::choose`], never taking `dead` (a target that has just fallen
    /// and that the picture may not yet know has).
    fn choose_without(&mut self, cx: &Context<'_>, dead: Option<u32>) {
        // A takeover attacks only when safe (slice ST-K).
        if self.holds_off(cx) {
            return;
        }
        let inputs = cx.inputs;
        let own = cx.own;
        // A spot has appeared: the sight guides the laser bombs again.
        if matches!(
            self.no_laze,
            Some(NoLazeCall {
                kind: NoLaze::Manual,
                ..
            })
        ) && self.spot_somewhere(cx)
        {
            self.no_laze = None;
        }
        let picked = match dead {
            Some(dead) => self
                .pick_target_where(cx, &|id| id != dead)
                .filter(|id| *id != dead),
            None => self.pick_target(cx),
        };
        let Some(target) = picked else {
            // A designator with nothing to strike with waits for its wing's
            // laser carriers: they choose, it lases (slice ST-L).
            if inputs.designator
                && cx.wing.carriers
                && !inputs
                    .stores
                    .iter()
                    .any(|s| s.rounds > 0 && s.kind != StoreKind::Command)
            {
                self.enter(Phase::Wait, cx.tick);
                self.target = None;
                return;
            }
            // A laser carrier that wants only a buddy's spot looks for one
            // while it closes on the target, and gives up inside its fire
            // range (slice ST-L).
            if let Some(point) = self.spot_wait_point(cx) {
                if horizontal(own.position, point) > SPOT_WAIT_RANGES * self.laser_fire_range(cx) {
                    self.enter(Phase::Wait, cx.tick);
                    self.target = None;
                    return;
                }
                // No spot by then: drop by hand, or go home saying so
                // (slice ST-G).
                if self.decide_no_laze(cx) {
                    return;
                }
            }
            let waiting = cx.picture.briefed().iter().any(|b| {
                b.standing
                    && b.side != cx.side
                    && b.side != Side(0)
                    && b.kind != BriefedKind::Ordered
                    && cx.mates.iter().filter(|id| **id == b.id).count()
                        >= targeting::STRIKE_ATTACKER_ALLOWANCE as usize
                    && can_attack_target(
                        &inputs.stores,
                        inputs.class,
                        target_facts(cx.picture, b.id),
                        spot_assured(cx, b.id),
                    )
            });
            if waiting && cx.picture.ordered().is_none() {
                self.enter(Phase::Wait, cx.tick);
            } else {
                self.enter(Phase::Home, cx.tick);
            }
            self.target = None;
            return;
        };
        let target_at = known_position(cx.picture, target)
            .or_else(|| inputs.solution.map(|s| s.target_position))
            .unwrap_or(own.position);
        let release_at = target_at;
        let threat = cx
            .picture
            .threats()
            .level(cx.side, &[target_at, release_at], cx.warnings);
        let facts = target_facts(cx.picture, target);
        let store = match self.forced_station {
            Some(station) => inputs
                .stores
                .iter()
                .find(|s| s.station == station && s.rounds > 0)
                .copied(),
            None => self.kept_store(inputs).or_else(|| {
                let manual = self.manual_on(cx);
                let view;
                let stores: &[Store] = if manual {
                    view = self.manual_view(cx);
                    &view
                } else {
                    &inputs.stores
                };
                // A wingman sent at a specific object uses a carried seeker
                // before an unguided bomb. Strike-role wings keep their existing
                // load priorities and threat-dependent tactics.
                let precision =
                    (cx.picture.role() == super::engagement::StrikeRole::Cap).then(|| {
                        stores
                            .iter()
                            .copied()
                            .filter(|s| s.kind.seeker())
                            .collect::<Vec<_>>()
                    });
                precision
                    .as_ref()
                    .and_then(|stores| {
                        choose_store(
                            stores,
                            inputs.class,
                            threat,
                            facts,
                            spot_assured(cx, target) || manual,
                        )
                    })
                    .or_else(|| {
                        choose_store(
                            stores,
                            inputs.class,
                            threat,
                            facts,
                            spot_assured(cx, target) || manual,
                        )
                    })
            }),
        };
        // A laser bomb dropped by hand is an unguided bomb to the pass: a
        // slick one (its launch retard is the glide bomb's, not a drag
        // chute's, so it flies no laydown).
        let store = store.map(|s| {
            if s.kind == StoreKind::Laser && self.manual_on(cx) && s.falls_free {
                Store {
                    kind: StoreKind::Bomb,
                    release_speed_share: 1.,
                    ..s
                }
            } else {
                s
            }
        });
        let Some(store) = store else {
            self.enter(Phase::Home, cx.tick);
            self.target = None;
            return;
        };
        if self.target != Some(target) {
            self.pass_number = 0;
        }
        self.target = Some(target);
        let same_store = self.station == Some(store.station);
        self.station = Some(store.station);
        self.kind = Some(store.kind);
        self.threat = threat;
        let distance = horizontal(own.position, target_at);
        let agl = own.agl_ft;
        let radius = turn_radius_ft(own.speed.0, own.g_limit);
        let choice = match self.forced.or_else(|| self.kept_pass(store.kind)) {
            Some(pass) => PassChoice::Pass(pass),
            None => choose_pass(PassInputs {
                class: inputs.class,
                kind: store.kind,
                retarded: store.retarded(),
                threat,
                energy_good: energy_good(own),
                dive_gate: dive_gate(agl, distance, radius),
            }),
        };
        let pass = match choice {
            PassChoice::Regain => {
                self.enter(Phase::Regain, cx.tick);
                return;
            }
            PassChoice::Pass(pass) => pass,
        };
        let pass = if cx.picture.role() == super::engagement::StrikeRole::Cap
            && self.forced.is_none()
            && self.takeover.is_none()
            && pass == Pass::Straight
            && store.kind == StoreKind::Bomb
            && !store.retarded()
        {
            Pass::Level
        } else {
            pass
        };
        self.pass = Some(pass);
        self.pass_number += 1;
        let radius_ft = release_circle_ft(cx.experience);
        let draw = aim_draw(self.seed, cx.aircraft, self.pass_number, radius_ft);
        self.aim_ft = draw.offset_ft;
        self.inside = draw.inside;
        self.jink_side = draw.side;
        self.gate_ft = (GATE_SHARE * radius_ft).max(GATE_FLOOR_FT);
        self.holding = None;
        // The trim belongs to the store's mount and the wind: it carries over
        // to the next pass with the same store.
        if !same_store {
            self.cue_across_sum = 0.;
        }
        self.cue_last = None;
        self.released = 0;
        self.next_release = 0;
        self.burst_open = None;
        self.recovery_until = 0;
        self.burst = 0;
        self.release_plan = None;
        self.plan_dropped = false;
        if store.kind == StoreKind::Gun {
            // Each burst has its own draw (John, 2026-10-11, later).
            self.next_burst(cx);
        }
        let floor = self.floor(cx, threat);
        self.hold_ft = Some(match pass {
            Pass::Level => target_at[1] + agl.max(floor),
            Pass::FastHigh => target_at[1] + FAST_HIGH_OFFSET_FT.max(floor),
            Pass::Dive if store.kind == StoreKind::Rocket => target_at[1] + ROCKET_DIVE_HEIGHT_FT,
            Pass::Dive => target_at[1] + agl.max(2. * radius).max(DIVE_GATE_MIN_FT),
            Pass::PopUp => target_at[1] + POPUP_OFFSET_FT,
            // The families' own passes are never a fighter's.
            Pass::Straight | Pass::Orbit | Pass::StandOff => target_at[1],
            Pass::Laydown => target_at[1] + laydown_height_at(&store, own.speed.0, target_at[1]),
            Pass::Strafe => target_at[1] + STRAFE_HEIGHT_FT,
        });
        // A target behind is not attacked from where it stands: the exit
        // opens the distance and turns back (fitted).
        let off = angle_difference(own.heading_deg, bearing_deg(own.position, target_at)).abs();
        if off > 90. && distance < self.exit_distance(cx) {
            self.exit(cx);
            return;
        }
        self.enter(Phase::Attack, cx.tick);
    }

    /// A laser carrier with no designator in its wing and nothing else to
    /// drop that found no target with a buddy's spot on it (the caller has
    /// asked [`Self::pick_target`]), and the target it would wait for.
    fn spot_wait_point(&self, cx: &Context<'_>) -> Option<Vector> {
        let inputs = cx.inputs;
        let laser = self.carries_only_laser(cx);
        (laser && !inputs.designator && !cx.wing.designator && !self.manual_on(cx))
            .then(|| {
                cx.picture
                    .ordered()
                    .and_then(|id| known_position(cx.picture, id))
                    .or_else(|| objective_point(cx.picture))
            })
            .flatten()
    }

    /// The striker has a laser store with rounds and no other store it could
    /// drop on a target (an anti-radiation or other missile counts as one; a
    /// gun only where [`has_usable_store`] lets it strafe, so a fighter's gun
    /// does not, found at the integration).
    fn carries_only_laser(&self, cx: &Context<'_>) -> bool {
        let stores = &cx.inputs.stores;
        stores
            .iter()
            .any(|s| s.kind == StoreKind::Laser && s.rounds > 0)
            && !stores.iter().any(|s| {
                s.kind != StoreKind::Laser
                    && has_usable_store(std::slice::from_ref(s), false, cx.inputs.class)
            })
    }

    /// The laser store's fire range, feet (zero with none).
    fn laser_fire_range(&self, cx: &Context<'_>) -> f64 {
        cx.inputs
            .stores
            .iter()
            .filter(|s| s.kind == StoreKind::Laser && s.rounds > 0)
            .map(|s| s.fire_range_ft)
            .fold(0., f64::max)
    }

    /// The target the striker attacks next: the ordered one, an
    /// anti-radiation carrier's radars (SAM, then AAA), else the selector's.
    fn pick_target(&self, cx: &Context<'_>) -> Option<u32> {
        self.pick_target_as(cx, false)
    }

    /// [`Self::pick_target`], or with `unbound` the target it would pick if
    /// its laser stores needed no spot (the no-laze decision names it).
    fn pick_target_as(&self, cx: &Context<'_>, unbound: bool) -> Option<u32> {
        self.pick_target_with(cx, unbound, None)
    }

    /// [`Self::pick_target`] among the candidates `allow` accepts (a
    /// helicopter's targets outside every ring, slice H4).
    fn pick_target_where(&self, cx: &Context<'_>, allow: &dyn Fn(u32) -> bool) -> Option<u32> {
        self.pick_target_with(cx, false, Some(allow))
    }

    fn pick_target_with(
        &self,
        cx: &Context<'_>,
        unbound: bool,
        allow: Option<&dyn Fn(u32) -> bool>,
    ) -> Option<u32> {
        let own = cx.own.position;
        let manual = self.manual_on(cx);
        let view;
        let stores: &[Store] = if manual {
            view = self.manual_view(cx);
            &view
        } else {
            &cx.inputs.stores
        };
        let anti_radiation = stores
            .iter()
            .any(|s| s.kind == StoreKind::AntiRadiation && s.rounds > 0);
        // A laser store is usable with a spot assured: its own
        // designator, a designator in the wing, or a same-side spot on the
        // target (slice ST-L); or dropped by hand (slice ST-G).
        let spot_any = cx.inputs.designator
            || cx.wing.designator
            || !cx.inputs.lase.spot_targets.is_empty()
            || manual
            || unbound;
        let usable_store = has_usable_store(stores, spot_any, cx.inputs.class);
        // A laser carrier with no designator, no designator in its wing and
        // nothing else to drop attacks only what a buddy's spot lies on.
        let attackers = |id: u32| cx.mates.iter().filter(|m| **m == id).count() as u32;
        let usable = |_: u16| usable_store;
        let mut candidates = cx
            .picture
            .strike_candidates(own, cx.side, &attackers, &usable);
        // A surface store is not necessarily usable on this particular object.
        // In particular, an anti-radiation missile cannot attack a truck.
        for candidate in &mut candidates {
            candidate.usable &= candidate.side != Side(0);
            candidate.usable &= can_attack_target(
                stores,
                cx.inputs.class,
                target_facts(cx.picture, candidate.id.0),
                spot_assured(cx, candidate.id.0) || manual || unbound,
            );
        }
        if !manual
            && !unbound
            && self.carries_only_laser(cx)
            && !cx.inputs.designator
            && !cx.wing.designator
        {
            for c in &mut candidates {
                c.usable &= cx.inputs.lase.spot_targets.contains(&c.id.0);
            }
        }
        if let Some(allow) = allow {
            for c in &mut candidates {
                c.usable &= allow(c.id.0);
            }
        }
        let ordered = candidates
            .iter()
            .any(|c| c.source == StrikeSource::Ordered && c.valid && c.side != cx.side);
        if anti_radiation && !ordered {
            let radar = sead_order(cx.picture, cx.side, own, &attackers);
            if let Some(id) = radar.first() {
                return Some(*id);
            }
        }
        targeting::select_strike_target(ObjectId(cx.aircraft), cx.side, &candidates).map(|c| c.id.0)
    }

    /// The striker drops its laser bombs by hand (slice ST-G): it decided
    /// to, no spot has appeared since, and a laser store that falls free
    /// still has rounds.
    fn manual_on(&self, cx: &Context<'_>) -> bool {
        matches!(
            self.no_laze,
            Some(NoLazeCall {
                kind: NoLaze::Manual,
                ..
            })
        ) && cx
            .inputs
            .stores
            .iter()
            .any(|s| s.kind == StoreKind::Laser && s.falls_free && s.rounds > 0)
    }

    /// A spot is, or will be, on a target: the aircraft's own designator, a
    /// designator in its wing, or any same-side spot.
    fn spot_somewhere(&self, cx: &Context<'_>) -> bool {
        cx.inputs.designator || cx.wing.designator || !cx.inputs.lase.spot_targets.is_empty()
    }

    /// The stores while it drops laser bombs by hand: the laser stores that
    /// cannot fall free (laser missiles) are left out.
    fn manual_view(&self, cx: &Context<'_>) -> Vec<Store> {
        cx.inputs
            .stores
            .iter()
            .map(|s| Store {
                rounds: if s.kind == StoreKind::Laser && !s.falls_free {
                    0
                } else {
                    s.rounds
                },
                ..*s
            })
            .collect()
    }

    /// The carrier of laser stores only has closed on the target without a
    /// spot (slice ST-G; John, 2026-10-12): a laser bomb is dropped by hand
    /// as an unguided bomb, and with no laser store that can fall free it
    /// goes home, saying "No laze support" either way. Returns whether it
    /// decided (it needs a target to bomb).
    fn decide_no_laze(&mut self, cx: &Context<'_>) -> bool {
        let Some(target) = self.pick_target_as(cx, true) else {
            return false;
        };
        let lasers = || {
            cx.inputs
                .stores
                .iter()
                .filter(|s| s.kind == StoreKind::Laser && s.rounds > 0)
        };
        let Some(store) = lasers().find(|s| s.falls_free).or_else(|| lasers().next()) else {
            return false;
        };
        let kind = if store.falls_free {
            NoLaze::Manual
        } else {
            NoLaze::Rtb
        };
        self.no_laze = Some(NoLazeCall {
            kind,
            station: store.station,
            target: Some(target),
            told: false,
        });
        match kind {
            NoLaze::Manual => self.choose(cx),
            NoLaze::Rtb => {
                self.enter(Phase::Home, cx.tick);
                self.target = None;
            }
        }
        true
    }

    /// The release floor of this pass (spec): 3,000 ft or 5,000 ft by
    /// threat; a heavy's own.
    fn floor(&self, cx: &Context<'_>, threat: ThreatLevel) -> f64 {
        match cx.inputs.class {
            StrikeClass::Heavy => heavy_floor_ft(
                cx.inputs.mission_height_ft.or(self.tasked_agl_ft),
                threat,
                cx.ceiling_ft - cx.own.position[1] + cx.own.agl_ft,
            ),
            _ => release_floor_ft(threat),
        }
    }

    fn exit_distance(&self, cx: &Context<'_>) -> f64 {
        let radius = turn_radius_ft(cx.own.speed.0, cx.own.g_limit);
        let base = if cx.inputs.class == StrikeClass::Heavy {
            HEAVY_EXIT_FT
        } else {
            EXIT_TURN_MIN_FT
        };
        let standoff = self.standoff_exit_ft(cx.tick).unwrap_or(0.);
        base.max(2. * radius).max(standoff)
    }

    /// Draws the next gun burst's aim (slice ST-T2): John's circle applies
    /// to each burst, by a draw keyed on the pass and the burst number.
    fn next_burst(&mut self, cx: &Context<'_>) {
        self.burst += 1;
        let draw = burst_draw(
            self.seed,
            cx.aircraft,
            self.pass_number,
            self.burst,
            release_circle_ft(cx.experience),
        );
        // The rounds walk in from short of the target, as a burst closing on
        // it does: an offset beyond it is mirrored back along the track.
        let mut offset = draw.offset_ft;
        if let Some(u) = track_unit(cx.velocity) {
            let along = offset[0] * u[0] + offset[1] * u[1];
            if along > 0. {
                offset = [offset[0] - 2. * along * u[0], offset[1] - 2. * along * u[1]];
            }
        }
        self.aim_ft = offset;
        self.inside = draw.inside;
    }

    /// Leave the pass: the exit run, straight on; after a stand-off
    /// release (John, 2026-10-29), away from the target.
    fn exit(&mut self, cx: &Context<'_>) {
        self.exit_heading_deg = match self.target.and_then(|id| known_position(cx.picture, id)) {
            Some(target) if self.standing_off(cx.tick) => bearing_deg(target, cx.own.position),
            _ => cx.own.heading_deg,
        };
        self.holding = None;
        self.retargeted = false;
        self.enter(Phase::Exit, cx.tick);
    }

    /// The striker's target fell before its pass released (T.O.R.E, John,
    /// 2026-10-28): it takes the next target at once, by the same choice and
    /// attacker allowance as at the exit distance, instead of flying the
    /// exit first. At least the set-up distance ([`RETARGET_SETUP_SHARE`] of
    /// the exit distance) away it turns back toward it at once, climbing to
    /// its cruise height, and chooses its pass once pointing at it (as at
    /// the end of an exit) and back at that height or inside the set-up
    /// distance: at once if it already is. Nearer, it flies the exit on its
    /// heading and turns back at the set-up distance. Returns false,
    /// changing nothing, for a striker that
    /// released on its pass or is past its release point, or that has no
    /// other target: it exits as before.
    fn retarget(&mut self, cx: &Context<'_>) -> bool {
        let Some(dead) = self.target else {
            return false;
        };
        if self.released > 0 || self.segment == Segment::Climb || self.past_release(cx, dead) {
            return false;
        }
        let Some(next) = self
            .pick_target_where(cx, &|id| id != dead)
            .filter(|id| *id != dead)
        else {
            return false;
        };
        let Some(at) = known_position(cx.picture, next) else {
            return false;
        };
        let own = cx.own;
        let distance = horizontal(own.position, at);
        let off = angle_difference(own.heading_deg, bearing_deg(own.position, at)).abs();
        let setup = RETARGET_SETUP_SHARE * self.exit_distance(cx);
        if distance >= setup && off <= TURN_BACK_DEG && self.set_up(cx, at) {
            self.choose_without(cx, Some(dead));
            return true;
        }
        self.target = Some(next);
        self.pass_number = 0;
        if distance >= setup {
            self.enter(Phase::TurnBack, cx.tick);
        } else {
            self.exit(cx);
        }
        self.retargeted = true;
        true
    }

    /// After an early retarget, the striker can choose its pass on the
    /// target at `point`: back at the height it cruises at (within
    /// [`RETARGET_HEIGHT_SLACK_FT`]), or already inside the set-up distance.
    fn set_up(&self, cx: &Context<'_>, point: Vector) -> bool {
        cx.own.position[1] >= self.cruise_altitude(cx) - RETARGET_HEIGHT_SLACK_FT
            || horizontal(cx.own.position, point) <= RETARGET_SETUP_SHARE * self.exit_distance(cx)
    }

    /// The pass is past the point where it would have released: the cue
    /// beyond the aim at the last attack step, or with no estimate the
    /// target `id` behind the aircraft.
    fn past_release(&self, cx: &Context<'_>, id: u32) -> bool {
        match self.release_in_s {
            Some(seconds) => seconds < 0.,
            None => known_position(cx.picture, id)
                .or_else(|| cx.inputs.solution.map(|s| s.target_position))
                .is_none_or(|p| {
                    angle_difference(cx.own.heading_deg, bearing_deg(cx.own.position, p)).abs()
                        >= 90.
                }),
        }
    }

    /// Toward `point` at the cruise altitude (at least the release floor),
    /// as a heavy, at its floor.
    fn cruise_steer(&self, cx: &Context<'_>, point: Vector, speed: SpeedRequest) -> Steer {
        let own = cx.own;
        Steer::plain(
            bearing_deg(own.position, point),
            hold_pitch_deg(own.position[1], self.cruise_altitude(cx)),
            speed,
            false,
        )
    }

    /// The height [`Self::cruise_steer`] flies at, feet above sea level.
    fn cruise_altitude(&self, cx: &Context<'_>) -> f64 {
        let own = cx.own;
        let floor = self.floor(cx, self.threat);
        let ground = own.position[1] - own.agl_ft;
        let mut altitude = self
            .cruise_ft
            .unwrap_or(own.position[1])
            .max(ground + floor);
        if cx.inputs.class != StrikeClass::Heavy {
            // Fighters keep what they have above the floor.
            altitude = altitude.max(own.position[1].min(ground + floor + 20_000.));
        }
        altitude
    }

    fn exit_steer(&self, cx: &Context<'_>, target_at: Option<Vector>) -> Steer {
        let own = cx.own;
        let away = target_at.map_or(f64::INFINITY, |p| horizontal(own.position, p));
        // A dive's exit climbs back to the dive's height for the next pass.
        let dive = (self.pass == Some(Pass::Dive))
            .then_some(self.hold_ft)
            .flatten()
            .unwrap_or(f64::MIN);
        let cap = (self.cruise_ft.unwrap_or(own.position[1]) + EXIT_CLIMB_ABOVE_FT).max(dive);
        let climbing = own.position[1] < cap;
        if away < JINK_INSIDE_FT {
            let leg = cx.tick.saturating_sub(self.since) / JINK_LEG_TICKS % JINK_LEGS;
            let step = if leg.is_multiple_of(2) {
                JINK_STEP_DEG
            } else {
                0.
            };
            Steer::plain(
                wrap_deg(self.exit_heading_deg + self.jink_side * step),
                if climbing { JINK_CLIMB_DEG } else { 0. },
                SpeedRequest::Maximum,
                true,
            )
        } else {
            Steer::plain(
                self.exit_heading_deg,
                if climbing {
                    EXIT_CLIMB_DEG
                } else {
                    hold_pitch_deg(own.position[1], cap)
                },
                SpeedRequest::Maximum,
                true,
            )
        }
    }

    /// The pass in flight.
    fn attack(&mut self, cx: &Context<'_>, target: Vector, solution: Option<Solution>) -> Order {
        let own = cx.own;
        let pass = self.pass.unwrap_or(Pass::Straight);
        let kind = self.kind.unwrap_or(StoreKind::Bomb);
        // A moving target is led by the cue's time of fall (fitted, agent).
        self.lead_ft = match solution {
            Some(s) if kind.unguided() => {
                let t = s.impact_seconds.unwrap_or(0.).clamp(0., 60.);
                [s.target_velocity[0] * t, s.target_velocity[2] * t]
            }
            _ => [0.; 2],
        };
        // A gun run is flown onto the target itself (and its lead): the
        // burst's start offset and its walk-in are the gunner's pointing,
        // laid on the rounds as they leave ([`Self::burst_aim`]).
        let offset = if kind == StoreKind::Gun {
            self.lead_ft
        } else if kind.unguided() {
            [
                self.aim_ft[0] + self.lead_ft[0],
                self.aim_ft[1] + self.lead_ft[1],
            ]
        } else {
            [0.; 2]
        };
        let aim = [target[0] + offset[0], target[1], target[2] + offset[1]];
        let distance = horizontal(own.position, target);
        let floor = self.floor(cx, self.threat);
        let mut order = Order {
            steer: None,
            release: None,
            activity: Some(Activity::Attacking),
        };
        let mut hold = self.hold_ft.unwrap_or(own.position[1]);
        // A glide bomb the sight says cannot reach from here (too high, or
        // too slow for its life) is approached in a descent to a height it
        // can (fitted, agent, slice ST-L); never below the release floor.
        if kind.level_run() && solution.is_some_and(|s| !s.reach_ok) {
            let lowest = own.position[1] - own.agl_ft + floor;
            hold = (hold - GLIDE_DESCENT_FPS / 120.).max(lowest.min(hold));
            self.hold_ft = Some(hold);
        }
        let speed = if pass == Pass::FastHigh {
            SpeedRequest::Maximum
        } else {
            SpeedRequest::Corner
        };
        let afterburner = pass == Pass::FastHigh;
        let ahead =
            angle_difference(own.heading_deg, bearing_deg(own.position, target)).abs() < 90.;
        let reach = horizontal(own.position, aim);

        // The approach and the pass's own end.
        let level_like = matches!(pass, Pass::Level | Pass::Laydown)
            || (pass == Pass::FastHigh && !kind.seeker())
            || kind.level_run();
        if self.segment == Segment::Approach {
            match pass {
                Pass::PopUp if distance < popup_pull_at(kind) && ahead && !kind.seeker() => {
                    self.segment_to(Segment::Pull, cx.tick)
                }
                Pass::Dive if depression_deg(own.position, aim) >= dive_rollin_deg(kind) => {
                    self.segment_to(Segment::Final, cx.tick)
                }
                Pass::Strafe if depression_deg(own.position, aim) >= STRAFE_DIVE_DEG => {
                    self.segment_to(Segment::Final, cx.tick)
                }
                Pass::Laydown if reach <= LAYDOWN_LEAD_FT => {
                    self.segment_to(Segment::Final, cx.tick)
                }
                Pass::Laydown | Pass::Strafe => {}
                Pass::Straight if kind.unguided() => self.segment_to(Segment::Final, cx.tick),
                _ if level_like || kind.seeker() => self.segment_to(Segment::Final, cx.tick),
                _ => {}
            }
        }
        if self.segment == Segment::Pull
            && cx.tick.saturating_sub(self.since) >= POPUP_PULL_TICKS
            && (depression_deg(own.position, target) >= popup_dive_deg(kind)
                || distance < 2. * POPUP_EXIT_FT)
        {
            self.segment_to(Segment::Final, cx.tick);
        }
        if self.segment == Segment::Climb {
            if cx.tick.saturating_sub(self.since) >= DIVE_CLIMB_TICKS {
                self.exit(cx);
                return Order {
                    steer: Some(self.exit_steer(cx, Some(target))),
                    ..order
                };
            }
            order.steer = Some(self.climb_steer(cx));
            return order;
        }
        // A stand-off release outside known rings (John, 2026-10-29): planned
        // again each step before the release; a pass at its plan while its
        // last stand-off round flies ends there.
        if self.update_release_plan(cx, target, solution) {
            self.exit(cx);
            return Order {
                steer: Some(self.exit_steer(cx, Some(target))),
                ..order
            };
        }
        let pull_out = dive_pull_out_ft(own);
        let ended = match pass {
            _ if kind == StoreKind::Rocket
                && self.released > 0
                && !self.burst_left(cx, solution, aim) =>
            {
                true
            }
            Pass::Dive => (self.segment == Segment::Final && own.agl_ft < pull_out) || !ahead,
            Pass::PopUp => self.segment == Segment::Final && (distance < POPUP_EXIT_FT || !ahead),
            Pass::Straight | Pass::Orbit | Pass::StandOff => distance < STRAIGHT_EXIT_FT,
            Pass::FastHigh => distance < FAST_HIGH_EXIT_FT && !level_like,
            Pass::Level | Pass::Laydown => false,
            Pass::Strafe => {
                !ahead
                    || (self.segment == Segment::Final
                        && (slant(own.position, target) < STRAFE_CEASE_FT
                            || own.agl_ft < STRAFE_BREAK_AGL_FT))
                    || (kind == StoreKind::Gun && self.rounds_left(cx) == 0)
            }
        } || (level_like && self.overflown(cx, target, aim, solution))
            || (kind.seeker() && distance < STRAIGHT_EXIT_FT)
            || (kind.level_run() && distance < GUIDED_EXIT_FT);
        if ended {
            if pass == Pass::Dive && self.segment == Segment::Final {
                self.segment_to(Segment::Climb, cx.tick);
                order.steer = Some(self.climb_steer(cx));
                return order;
            }
            self.exit(cx);
            order.steer = Some(self.exit_steer(cx, Some(target)));
            return order;
        }

        // Steering.
        let mut steer = match self.segment {
            Segment::Approach => {
                // Every approach is flown at the aim, so the cue starts in
                // line with it (the aim is the target for a guided store).
                let along = [aim[0] - own.position[0], aim[2] - own.position[2]];
                let length = along[0].hypot(along[1]).max(1.);
                let u = [along[0] / length, along[1] / length];
                let short_of = |by: f64, height: f64| -> Vector {
                    [aim[0] - u[0] * by, height, aim[2] - u[1] * by]
                };
                // The roll-in point of a dive: the dive's height, where the
                // aim will be `deg` down (fitted, agent); over the target
                // once inside it (retail).
                let rollin = |deg: f64| -> Vector {
                    let at = (hold - target[1]) / deg.to_radians().tan();
                    if reach > at + DIVE_ROLLIN_LEAD_FT {
                        short_of(at, hold)
                    } else {
                        short_of(0., hold)
                    }
                };
                let point = match pass {
                    // A low approach under the radar: down to 500 ft over
                    // the target by the pull point (fitted, agent), then on
                    // at that height (retail).
                    Pass::PopUp if reach > popup_pull_at(kind) + POPUP_LEAD_FT => {
                        short_of(popup_pull_at(kind), target[1] + POPUP_OFFSET_FT)
                    }
                    Pass::PopUp => short_of(0., target[1] + POPUP_OFFSET_FT),
                    Pass::FastHigh => short_of(0., hold),
                    Pass::Dive => rollin(dive_rollin_deg(kind)),
                    Pass::Strafe => rollin(STRAFE_DIVE_DEG),
                    // Down to the laydown height by its lead point.
                    Pass::Laydown => short_of(LAYDOWN_LEAD_FT, hold),
                    _ => short_of(0., target[1]),
                };
                let (heading, pitch) = steering_frame(own.position, point);
                Steer::plain(heading, pitch, speed, afterburner)
            }
            Segment::Pull => Steer::plain(
                bearing_deg(own.position, aim),
                (-depression_deg(own.position, target) + POPUP_PULL_DEG).min(MAX_DIVE_DEG),
                SpeedRequest::Corner,
                false,
            ),
            // A gun run tracks: the flight path turns the gun's ground cue
            // onto the burst's aim (fitted, agent).
            Segment::Final if kind == StoreKind::Gun => {
                // Rolled in onto the line to the aim first, as a dive.
                let line = steering_frame(own.position, aim).1;
                let rolling_in = own.flight_path_pitch_deg > line + STRAFE_ROLLIN_DONE_DEG;
                let (heading, pitch) = match solution.and_then(|s| s.impact) {
                    Some(cue) if !rolling_in => {
                        let turn =
                            elevation_deg(own.position, aim) - elevation_deg(own.position, cue);
                        // Across, the track on the aim's line, as a dive.
                        (
                            self.line_heading(cx, aim, cue, true),
                            own.flight_path_pitch_deg + GUN_PITCH_GAIN * turn,
                        )
                    }
                    _ => (bearing_deg(own.position, aim), line),
                };
                Steer {
                    heading_deg: heading,
                    pitch_deg: pitch.clamp(-2. * STRAFE_DIVE_DEG, STRAFE_DIVE_DEG),
                    speed: SpeedRequest::Corner,
                    afterburner: false,
                    handling: Handling::STRAFE,
                    bank_deg: None,
                }
            }
            Segment::Final if level_like || kind.seeker() => {
                let impact = solution.and_then(|s| s.impact).filter(|_| kind.unguided());
                let heading = match impact {
                    Some(impact) => self.line_heading(cx, aim, impact, false),
                    None => bearing_deg(own.position, aim),
                };
                let pitch = if kind.seeker() && pass != Pass::Level && pass != Pass::FastHigh {
                    steering_frame(own.position, target).1
                } else if pass == Pass::Laydown {
                    // Below the release floor by design: the laydown's own
                    // height clears the fuze and the fragments.
                    hold_pitch_deg(own.position[1], hold)
                } else {
                    hold_pitch_deg(
                        own.position[1],
                        hold.max(own.position[1] - own.agl_ft + floor),
                    )
                };
                Steer::plain(heading, pitch, speed, afterburner)
            }
            Segment::Final => {
                let (heading, pitch, rolling_in) = match solution.and_then(|s| s.impact) {
                    Some(impact) => {
                        let line = steering_frame(own.position, target).1;
                        let rollin_line = line
                            + if kind == StoreKind::Bomb && pass == Pass::Dive {
                                BOMB_ROLLIN_LEAD_DEG
                            } else {
                                0.
                            };
                        // A dive or a pop-up first rolls in onto its line
                        // (slice ST-T2): until the flight path is within
                        // [`ROLLIN_DONE_DEG`] of it, the cue is not flown.
                        let rolling_in = matches!(pass, Pass::Dive | Pass::PopUp)
                            && self.holding.is_none()
                            && own.flight_path_pitch_deg > rollin_line + ROLLIN_DONE_DEG;
                        // While rolling in the cue swings with the attitude,
                        // so the heading is the bearing to the aim.
                        if rolling_in {
                            (bearing_deg(own.position, aim), rollin_line, true)
                        } else {
                            // The roll and pull tracks the cue onto the aim
                            // and holds it there, as a gun run does (slice
                            // ST-T2); a straight run creeps it across.
                            let pitch = self.fly_cue(cx, aim, impact, line, pass);
                            // A dive or pop-up is rolled and pulled, so
                            // steered on its track; a straight run on its
                            // body heading.
                            let on_track = matches!(pass, Pass::Dive | Pass::PopUp);
                            (self.line_heading(cx, aim, impact, on_track), pitch, false)
                        }
                    }
                    None => {
                        let (heading, pitch) = steering_frame(own.position, aim);
                        (heading, pitch, matches!(pass, Pass::Dive | Pass::PopUp))
                    }
                };
                // A dive or a pop-up rolls and pulls (slice ST-T2); a straight
                // run flies a push over wings level (the adapter pushes
                // negative G for a pitch far below the flight path, where a
                // banked push turns the wrong way; fitted, agent).
                let handling = Handling {
                    track_heading: false,
                    pull: rolling_in || matches!(pass, Pass::Dive | Pass::PopUp),
                    planned_dive: pass == Pass::Dive,
                    strafe: false,
                };
                let heading = if !handling.pull && pushes(own, pitch) {
                    own.heading_deg
                } else {
                    heading
                };
                Steer {
                    heading_deg: heading,
                    pitch_deg: pitch.max(-MAX_DIVE_DEG),
                    speed: SpeedRequest::Corner,
                    afterburner: false,
                    bank_deg: None,
                    handling,
                }
            }
            Segment::Climb | Segment::Guide => unreachable!("handled above"),
        };
        steer.handling.track_heading = kind == StoreKind::Bomb;
        order.steer = Some(steer);

        // The release.
        if kind == StoreKind::Gun {
            if self.segment == Segment::Final {
                order.release = self.fire_gun(cx, aim, solution);
            }
            return order;
        }
        if matches!(self.segment, Segment::Final | Segment::Pull)
            && let Some(s) = solution
            && cx.tick >= self.next_release
        {
            // The release floor is a bomb's (rockets dive below it and pull
            // out on the plan, slice ST-T2).
            let floor_ok = match pass {
                Pass::Dive => own.agl_ft >= floor,
                // A level run holds its height over the target; the ground
                // under it may rise a little (fitted, agent).
                Pass::Level => own.agl_ft >= floor - LEVEL_FLOOR_SLACK_FT,
                _ => true,
            } || kind != StoreKind::Bomb;
            let first = self.released == 0;
            let in_zone = !kind.unguided()
                || s.impact.is_some_and(|i| {
                    let range = slant(own.position, i);
                    self.store(cx).is_none_or(|st| {
                        range >= st.minimum_range_ft
                            && (st.maximum_range_ft <= 0. || range <= st.maximum_range_ft)
                    })
                });
            // The release test (slice ST-T2): the shared gate around the
            // drawn aim, or the cue's closest pass by the aim inside John's
            // circle around the (led) target.
            let passing = self.cue_passing(cx, s.impact, aim);
            let due = s.due || passing;
            // A stand-off release planned outside known rings leaves at its
            // range once the store reaches (John, 2026-10-29).
            let planned = self.planned_due(cx, target, &s);
            let fire = in_zone
                && match kind {
                    StoreKind::Rocket => {
                        (first && due)
                            || (!first
                                && self.released < ROCKETS_PER_PASS
                                && s.impact.is_some_and(|i| {
                                    horizontal(i, aim) <= release_circle_ft(cx.experience)
                                }))
                    }
                    k if k.seeker() => first && (s.due || planned) && !s.guided_in_flight,
                    // A takeover drops no second laser weapon while one
                    // guides (slice ST-K).
                    StoreKind::Laser if self.takeover.is_some() => {
                        first && s.due && !s.guided_in_flight
                    }
                    k if k.unguided() => first && due,
                    _ => first && (s.due || planned),
                };
            if fire && floor_ok && !s.warning && self.rounds_left(cx) > 0 {
                order.release = Some(Release {
                    station: s.station,
                    target: s.target,
                });
                self.released += 1;
                self.next_release = cx.tick
                    + self
                        .store(cx)
                        .map_or(super::QUARTER_SECOND_TICKS, |st| st.cadence_ticks);
                self.released_on_plan(cx.tick);
                // A bomb, a guided weapon or a seeker leaves once a pass;
                // the run then exits (a dive climbs out first).
                if kind != StoreKind::Rocket && pass != Pass::Dive {
                    // A laser weapon on the aircraft's own spot is held on
                    // the target until it lands (slice ST-L).
                    if !(kind == StoreKind::Laser && self.hold_after_release(cx)) {
                        self.exit(cx);
                    }
                } else if kind != StoreKind::Rocket {
                    self.segment_to(Segment::Climb, cx.tick);
                }
            }
        }
        self.update_commit(cx, target, aim, solution);
        order
    }

    /// Times the pass's release and opens or closes its committed-to-release
    /// window (T.O.R.E, John, 2026-10-28; see [`COMMIT_WINDOW_S`]). The
    /// window opens in the final (or a pop-up's pull), before anything has
    /// left, once the release is [`COMMIT_WINDOW_S`] or less away; it holds
    /// through a rocket burst, and closes at the release, when the pass
    /// ends, when the cue is [`COMMIT_PAST_S`] past the aim, or
    /// [`COMMIT_HOLD_TICKS`] after it opened.
    fn update_commit(
        &mut self,
        cx: &Context<'_>,
        target: Vector,
        aim: Vector,
        solution: Option<Solution>,
    ) {
        let flying =
            self.phase == Phase::Attack && matches!(self.segment, Segment::Final | Segment::Pull);
        let estimate = if flying {
            self.release_estimate(cx, target, aim, solution)
        } else {
            self.release_cue = None;
            None
        };
        self.release_in_s = estimate;
        let bursting = self.kind == Some(StoreKind::Rocket)
            && self.released > 0
            && self.burst_left(cx, solution, aim);
        let live = flying && (self.released == 0 || bursting);
        self.committed = match (live, self.committed) {
            (false, _) => None,
            (true, None) => estimate
                .filter(|t| (0.0..=COMMIT_WINDOW_S).contains(t))
                .map(|_| cx.tick),
            (true, Some(since)) => (cx.tick.saturating_sub(since) <= COMMIT_HOLD_TICKS
                && (bursting || estimate.is_none_or(|t| t >= -COMMIT_PAST_S)))
            .then_some(since),
        };
    }

    /// Seconds to the pass's release point, negative once past it: for a
    /// bomb or rocket the cue's distance short of the aim along the track
    /// over its closing speed, measured and filtered over
    /// [`CUE_RATE_TICKS`] (the ground speed at first, as the level T has
    /// it); a guided bomb's from the host's sight; a seeker's (or a guided
    /// bomb's without one) its range less its fire range over the closing
    /// speed. `None` for a gun run, which has no single release point.
    fn release_estimate(
        &mut self,
        cx: &Context<'_>,
        target: Vector,
        aim: Vector,
        solution: Option<Solution>,
    ) -> Option<f64> {
        let kind = self.kind?;
        let own = cx.own;
        let over = |along: f64, closing: f64| {
            if closing > 1. {
                along / closing
            } else if along >= 0. {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            }
        };
        let fire_range = |this: &Self| -> Option<f64> {
            let store = this.store(cx)?;
            let to = [target[0] - own.position[0], target[2] - own.position[2]];
            let length = to[0].hypot(to[1]).max(1.);
            let closing = (cx.velocity[0] * to[0] + cx.velocity[2] * to[1]) / length;
            Some(over(
                slant(own.position, target) - store.fire_range_ft,
                closing,
            ))
        };
        // A planned stand-off release: the distance to its range over the
        // closing speed (John, 2026-10-29).
        if let Some(plan) = self.release_plan.filter(|_| self.released == 0)
            && (kind.seeker() || kind.level_run())
        {
            let to = [target[0] - own.position[0], target[2] - own.position[2]];
            let length = to[0].hypot(to[1]).max(1.);
            let closing = (cx.velocity[0] * to[0] + cx.velocity[2] * to[1]) / length;
            return Some(over(
                horizontal(own.position, target) - plan.range_ft,
                closing,
            ));
        }
        match kind {
            StoreKind::Bomb | StoreKind::Rocket => {
                let Some(impact) = solution.and_then(|s| s.impact) else {
                    self.release_cue = None;
                    return None;
                };
                let u = track_unit(cx.velocity)?;
                let along = (aim[0] - impact[0]) * u[0] + (aim[2] - impact[2]) * u[1];
                let closing = match self.release_cue {
                    Some((tick, before, rate)) if cx.tick > tick => {
                        let ticks = (cx.tick - tick) as f64;
                        let measured = (before - along) * 120. / ticks;
                        rate + (measured - rate) * (ticks / CUE_RATE_TICKS).min(1.)
                    }
                    Some((_, _, rate)) => rate,
                    None => cx.velocity[0].hypot(cx.velocity[2]),
                };
                self.release_cue = Some((cx.tick, along, closing));
                Some(over(along, closing))
            }
            k if k.seeker() => fire_range(self),
            StoreKind::Laser | StoreKind::Coordinate => solution
                .and_then(|s| s.release_seconds)
                .or_else(|| fire_range(self)),
            // A gun run's bursts, and the helicopters' command round.
            _ => None,
        }
    }

    /// The heading that lays the track on the aim's bearing (slice ST-T2,
    /// fitted, agent): on that track, wings level, the cue lies on the line
    /// to the aim, so the sideways miss closes on the first pass from any
    /// reasonable offset. What the cue still lies off that line (a pod's or
    /// gun's offset, a crab in the wind) is trimmed out slowly by moving the
    /// line across ([`CUE_TRACK_INTEGRAL`]). In a dive a heading change turns the path
    /// across by only the dive angle's cosine, so the correction grows by
    /// it. `on_track` lays the correction on the track (a rolled and pulled
    /// steer), else on the body heading (the plain adapter steers that).
    fn line_heading(
        &mut self,
        cx: &Context<'_>,
        aim: Vector,
        impact: Vector,
        on_track: bool,
    ) -> f64 {
        let own = cx.own;
        let Some(u) = track_unit(cx.velocity) else {
            return bearing_deg(own.position, aim);
        };
        let track = wrap_deg(u[0].atan2(u[1]).to_degrees());
        // The cue's miss across the track, feet (positive: the aim lies right).
        let right = [u[1], -u[0]];
        let cross = (aim[0] - impact[0]) * right[0] + (aim[2] - impact[2]) * right[1];
        // Bombs inherit ground velocity, so their cue follows the track.
        // Point that cue directly; attitude-based integral trim adds a lag
        // and can swing it past the unchanged skill gate before release.
        let velocity_bomb = self.kind == Some(StoreKind::Bomb);
        if velocity_bomb || self.pass.is_some_and(Pass::points_cue) {
            let range = horizontal(own.position, impact).max(CUE_POINT_MIN_RANGE_FT);
            let gain = if velocity_bomb && on_track {
                BOMB_CUE_TRACK_GAIN / own.flight_path_pitch_deg.to_radians().cos().max(0.25)
            } else {
                CUE_POINT_GAIN
            };
            let rate = if velocity_bomb {
                self.cue_last.map_or(0., |(previous, _)| {
                    let previous_cross = -previous[0] * right[0] - previous[1] * right[1];
                    ((cross - previous_cross) * 120. / range).to_degrees()
                })
            } else {
                0.
            };
            let turn = gain * (cross.atan2(range).to_degrees() + BOMB_CUE_DAMPING_S * rate);
            let base = if velocity_bomb || on_track {
                track
            } else {
                own.heading_deg
            };
            return wrap_deg(base + turn.clamp(-30., 30.));
        }
        // The track is laid on a point the trim has moved across from the
        // aim; the trim learns only once the track is on that line, so it
        // does not chase the cue's swing while the heading still settles.
        let point = [
            aim[0] + right[0] * self.cue_across_sum,
            aim[1],
            aim[2] + right[1] * self.cue_across_sum,
        ];
        let off_line = angle_difference(track, bearing_deg(own.position, point));
        if off_line.abs() < CUE_TRIM_ON_LINE_DEG {
            self.cue_across_sum = (self.cue_across_sum + CUE_TRACK_INTEGRAL * cross / 120.)
                .clamp(-CUE_TRIM_LIMIT_FT, CUE_TRIM_LIMIT_FT);
        }
        let correction = off_line / own.flight_path_pitch_deg.to_radians().cos().max(0.25);
        let base = if on_track { track } else { own.heading_deg };
        wrap_deg(base + correction.clamp(-30., 30.))
    }

    /// Whether the cue passed the drawn aim point at its closest during this
    /// tick (slice nr-cue, John's answer 12 of 2026-10-13; fitted, agent):
    /// the release the shared gate misses when the cue sweeps by the aim a
    /// few feet outside it. The cue's position against the aim is a straight
    /// line between two ticks, so the time of its closest approach inside
    /// the tick is solved exactly ([`closest_approach`]) and the two tests
    /// are made there, not on the tick's samples: the closest point lies
    /// inside John's circle around the aim, and the cue there lies inside
    /// John's circle around the led target (the ring's outer edge, 2R, for a
    /// ring draw), give or take the release gate. The answer is the same
    /// whatever the tick rate and wherever the ticks fall on the cue's
    /// path; the old test read the one sample before the closest point and
    /// flipped on one tick (the cue 125.2 ft off on one sample, 128.3 ft
    /// on the next, with a 126 ft circle). The first sample counts as a
    /// possible closest point (a cue that appears inside the circle and
    /// leaves is released as before).
    fn cue_passing(&mut self, cx: &Context<'_>, impact: Option<Vector>, aim: Vector) -> bool {
        let Some(impact) = impact else {
            self.cue_last = None;
            return false;
        };
        let now = [impact[0] - aim[0], impact[2] - aim[2]];
        let Some((before, closing)) = self.cue_last else {
            self.cue_last = Some((now, true));
            return false;
        };
        let (found, closing_now) = closest_approach(before, now, closing);
        self.cue_last = Some((now, closing_now));
        let Some(closest) = found else {
            return false;
        };
        // The led target lies the drawn offset behind the aim (the lead on a
        // moving target cancels): the cue's offset from it there.
        let from_led = [
            closest.at[0] + self.aim_ft[0],
            closest.at[1] + self.aim_ft[1],
        ];
        let circle = release_circle_ft(cx.experience);
        // The circle the draw allows, with the release gate's own tolerance.
        let radius = circle * if self.inside { 1. } else { 2. } + self.gate_ft;
        closest.distance <= circle && from_led[0].hypot(from_led[1]) <= radius
    }

    /// The pull-out after a dive: wings level, nose up 25 degrees (retail),
    /// pulled with the roll and pull's full authority.
    fn climb_steer(&self, cx: &Context<'_>) -> Steer {
        // On the track the aircraft flies, so the roll and pull asks for no
        // turn (the body heading drifts from it in the pull).
        let track = track_unit(cx.velocity).map_or(cx.own.heading_deg, |u| {
            wrap_deg(u[0].atan2(u[1]).to_degrees())
        });
        Steer {
            heading_deg: track,
            pitch_deg: DIVE_CLIMB_DEG,
            speed: SpeedRequest::Corner,
            afterburner: false,
            handling: Handling::PULL,
            bank_deg: None,
        }
    }

    /// A gun run's fire (slice ST-T2; John, 2026-10-11, later: the circle
    /// applies to each burst; slice ST-G, John, 2026-10-12: the rounds walk
    /// onto the target). The run is flown onto the target and its lead. A
    /// burst opens once the gun's ground cue lies within the pilot's
    /// fine-tracking reach of it ([`gun_track_ft`]) and inside the gun's
    /// range, and fires at the gun's cadence for the AI's 0.5 s burst while
    /// the cue stays within that reach, then recovers 0.5 s (the AI gun
    /// employment's policy, docs/spec/ai-gun-employment.md) and draws the
    /// next burst's start. The rounds leave from the burst's drawn offset
    /// and the aim walks in a straight line onto the target over the
    /// skill's walk-in time ([`gun_walk_in_s`], [`Self::burst_aim`]): the
    /// bridge lays each round on that aim, within the same reach of the cue.
    fn fire_gun(
        &mut self,
        cx: &Context<'_>,
        aim: Vector,
        solution: Option<Solution>,
    ) -> Option<Release> {
        let s = solution?;
        let cue = s.impact?;
        let store = self.store(cx)?;
        if store.rounds == 0 {
            return None;
        }
        let own = cx.own;
        let tick = cx.tick;
        let range = slant(own.position, cue);
        let in_zone = range >= store.minimum_range_ft
            && (store.maximum_range_ft <= 0. || range <= store.maximum_range_ft);
        // The cue must lie within the pilot's fine-tracking reach of the
        // target to fire; the rounds are then laid exactly on the burst's
        // walked aim (the bridge applies the correction).
        let near = horizontal(cue, aim) <= gun_track_ft(range);
        match self.burst_open {
            Some(start) => {
                let on = in_zone && tick < start + super::gunnery::BURST_TICKS && near;
                if !on {
                    self.burst_open = None;
                    self.recovery_until = tick + super::gunnery::RECOVERY_TICKS;
                    self.next_burst(cx);
                    return None;
                }
            }
            None => {
                if tick < self.recovery_until || !in_zone || !near {
                    return None;
                }
                self.burst_open = Some(tick);
                self.next_release = tick;
            }
        }
        if tick < self.next_release {
            return None;
        }
        self.released += 1;
        self.next_release = tick + store.cadence_ticks.max(1);
        Some(Release {
            station: s.station,
            target: s.target,
        })
    }

    fn store(&self, cx: &Context<'_>) -> Option<Store> {
        let station = self.station?;
        cx.inputs
            .stores
            .iter()
            .find(|s| s.station == station)
            .copied()
    }

    fn rounds_left(&self, cx: &Context<'_>) -> u32 {
        self.store(cx).map_or(0, |s| s.rounds)
    }

    /// A rocket burst goes on while rounds and the circle last.
    fn burst_left(&self, cx: &Context<'_>, solution: Option<Solution>, aim: Vector) -> bool {
        self.kind == Some(StoreKind::Rocket)
            && self.released < ROCKETS_PER_PASS
            && self.rounds_left(cx) > 0
            && solution
                .and_then(|s| s.impact)
                .is_some_and(|i| horizontal(i, aim) <= release_circle_ft(cx.experience))
    }

    /// The pitch that flies the cue onto the aim in a diving pass (fitted,
    /// agent): while the cue is far short, the pass's own line (`line`,
    /// degrees); else the flight path is turned by [`CUE_PITCH_GAIN`] times
    /// the angle between the cue and a point [`CUE_MARGIN_FT`] short of the
    /// aim, never more than 8 degrees above the line, until the cue is
    /// within that margin short of the aim; then the flight path is held and
    /// the cue creeps across the aim, as it does in any steady dive or level
    /// run. A cue that overshoots by the circle is flown again.
    fn fly_cue(
        &mut self,
        cx: &Context<'_>,
        aim: Vector,
        impact: Vector,
        line: f64,
        pass: Pass,
    ) -> f64 {
        let own = cx.own;
        // A steep dive holds its cue farther short, so the cue's sideways
        // error is gone before it creeps onto the aim (slice ST-T2, fitted).
        let margin = if pass == Pass::Dive {
            DIVE_CUE_MARGIN_FT
        } else {
            CUE_MARGIN_FT
        };
        let Some(u) = track_unit(cx.velocity) else {
            return line;
        };
        let short = (aim[0] - impact[0]) * u[0] + (aim[2] - impact[2]) * u[1];
        if let Some(hold) = self.holding {
            if short >= -release_circle_ft(cx.experience) {
                return hold;
            }
            self.holding = None;
        }
        if short > CUE_ACQUIRE_FT {
            return line;
        }
        if (0.0..=margin + CUE_HOLD_TOLERANCE_FT).contains(&short) {
            self.holding = Some(own.flight_path_pitch_deg);
            return own.flight_path_pitch_deg;
        }
        let q = [aim[0] - u[0] * margin, aim[1], aim[2] - u[1] * margin];
        let turn = elevation_deg(own.position, q) - elevation_deg(own.position, impact);
        (own.flight_path_pitch_deg + CUE_PITCH_GAIN * turn).min(line + 8.)
    }

    /// A level-like run has passed its chance: the cue (or, with none, the
    /// aircraft) is beyond the aim point by more than the circle, or the
    /// target is behind.
    fn overflown(
        &self,
        cx: &Context<'_>,
        target: Vector,
        aim: Vector,
        solution: Option<Solution>,
    ) -> bool {
        let own = cx.own;
        let track = [cx.velocity[0], cx.velocity[2]];
        let norm = track[0].hypot(track[1]);
        if norm < 1. {
            return false;
        }
        let u = [track[0] / norm, track[1] / norm];
        let along = |from: Vector| (aim[0] - from[0]) * u[0] + (aim[2] - from[2]) * u[1];
        match solution.and_then(|s| s.impact) {
            Some(impact) if self.kind.is_some_and(StoreKind::unguided) => {
                along(impact) < -release_circle_ft(cx.experience)
            }
            _ => along(own.position) < 0. && horizontal(own.position, target) > 0.,
        }
    }
}

/// Retail's pass choice outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassChoice {
    Pass(Pass),
    /// Too slow under threat: regain speed first.
    Regain,
}

/// What the pass choice reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassInputs {
    pub class: StrikeClass,
    pub kind: StoreKind,
    /// A drag (retarded) bomb.
    pub retarded: bool,
    pub threat: ThreatLevel,
    pub energy_good: bool,
    pub dive_gate: bool,
}

/// The pass, a fixed function of the bombing class, the store, the
/// perceived threat and the available speed (spec, "Pass choice"; John,
/// 2026-10-11, decision 3: no dice).
pub fn choose_pass(i: PassInputs) -> PassChoice {
    use PassChoice::Pass as P;
    if i.class == StrikeClass::Heavy {
        return P(Pass::Level);
    }
    match i.kind {
        k if k.seeker() => P(if i.energy_good {
            Pass::FastHigh
        } else {
            Pass::Straight
        }),
        StoreKind::Laser | StoreKind::Coordinate => P(Pass::Level),
        kind => {
            if !i.energy_good && i.threat >= ThreatLevel::Guns {
                PassChoice::Regain
            } else if kind == StoreKind::Gun {
                // A gun run (slice ST-T2).
                P(Pass::Strafe)
            } else if i.retarded {
                // A drag bomb is made for the low level pass (slice ST-T2).
                P(Pass::Laydown)
            } else if i.threat == ThreatLevel::Missiles {
                P(Pass::PopUp)
            } else if i.dive_gate {
                P(Pass::Dive)
            } else {
                P(Pass::Straight)
            }
        }
    }
}

/// A spot is assured on `target` (spec, "Lasing"): the aircraft's own
/// designator, a designator in its wing, or a same-side spot already on the
/// target.
pub fn spot_assured(cx: &Context<'_>, target: u32) -> bool {
    cx.inputs.designator || cx.wing.designator || cx.inputs.lase.spot_targets.contains(&target)
}

/// What the striker knows of a target for the store choice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TargetFacts {
    /// It has a radar (a briefed or perceived defence's radar).
    pub radar: bool,
    /// It emits now.
    pub emitting: bool,
}

/// The store for a target (spec, "Weapons and releases", "Store choice";
/// fitted order, agent): an anti-radiation missile at a radar; under threat
/// Missiles stand-off stores first; else bombs, then guided bombs, seekers
/// and rockets. A laser store only with a spot assured (its own
/// designator, decision 9). A heavy carries no rockets into a pass. The
/// AT-2 and guns are not chosen here.
pub fn choose_store(
    stores: &[Store],
    class: StrikeClass,
    threat: ThreatLevel,
    target: TargetFacts,
    designator: bool,
) -> Option<Store> {
    use StoreKind as K;
    let order: &[K] = if threat == ThreatLevel::Missiles {
        &[
            K::AntiRadiation,
            K::Seeker,
            K::RadarAgm,
            K::Coordinate,
            K::Laser,
            K::Bomb,
            K::Rocket,
            K::Gun,
        ]
    } else {
        &[
            K::AntiRadiation,
            K::Bomb,
            K::Coordinate,
            K::Laser,
            K::Seeker,
            K::RadarAgm,
            K::Rocket,
            K::Gun,
        ]
    };
    order.iter().find_map(|kind| {
        let allowed = match kind {
            K::AntiRadiation => target.radar || target.emitting,
            K::Laser => designator,
            K::Rocket => class != StrikeClass::Heavy,
            // Attack jets strafe once their bombs and rockets are gone
            // (spec, "Bombing class"); the driver may force any gun.
            K::Gun => class == StrikeClass::Attack,
            K::Command => false,
            _ => true,
        };
        allowed
            .then(|| {
                stores
                    .iter()
                    .find(|s| s.kind == *kind && s.rounds > 0)
                    .copied()
            })
            .flatten()
    })
}

/// Whether the carried stores can attack this object, before assigning a pass.
/// Helicopters and gunships retain their family-specific employment rules.
pub fn can_attack_target(
    stores: &[Store],
    class: StrikeClass,
    target: TargetFacts,
    designator: bool,
) -> bool {
    if matches!(class, StrikeClass::Helicopter | StrikeClass::Gunship) {
        has_usable_store(stores, designator, class)
    } else {
        choose_store(stores, class, ThreatLevel::Clear, target, designator).is_some()
    }
}

/// The SEAD order of an anti-radiation carrier's targets (spec, "SEAD";
/// John, decision 15): the objective's SAM radars, then AAA with radar,
/// then any other radar, each nearest first, standing and hostile, with
/// fewer than two other attackers.
pub fn sead_order(
    picture: &GroundPicture,
    side: Side,
    own: Vector,
    attackers: &dyn Fn(u32) -> u32,
) -> Vec<u32> {
    let mut ranked: Vec<(u8, f64, u32)> = picture
        .briefed()
        .iter()
        .filter(|b| b.standing && b.side != side && b.side != Side(0))
        .filter(|b| attackers(b.id) < targeting::STRIKE_ATTACKER_ALLOWANCE)
        .filter_map(|b| {
            let facts = b.defense?;
            if !facts.radar {
                return None;
            }
            let rank = if facts.missile_reach_ft > 0. || b.class & CLASS_SAM != 0 {
                0
            } else if facts.gun_reach_ft > 0. || b.class & CLASS_AAA != 0 {
                1
            } else {
                2
            };
            Some((rank, horizontal(own, b.position), b.id))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(_, _, id)| id).collect()
}

/// Whether any of the stores can attack a ground target now: a store with
/// rounds, the command-guided AT-2 only for a helicopter (slice ST-F flies
/// it), a gun only for an attack jet or the gunship, and a laser weapon only
/// when a spot is assured (`designator`). The target choice and a flight lead's order (which a wingman
/// refuses without one) both ask it.
pub fn has_usable_store(stores: &[Store], designator: bool, class: StrikeClass) -> bool {
    stores.iter().any(|s| {
        s.rounds > 0
            && (s.kind != StoreKind::Command || class == StrikeClass::Helicopter)
            && (s.kind != StoreKind::Laser || designator)
            // A gun strafes for an attack jet (slice ST-T2) and is the
            // gunship's weapon (slice ST-F); a fighter's gun is no reason
            // to take a ground order.
            && (s.kind != StoreKind::Gun
                || matches!(class, StrikeClass::Attack | StrikeClass::Gunship))
    })
}

/// The radius of John's release circle by skill (2026-10-11): Ace 10 m,
/// 25 m more for each level down.
pub fn release_circle_ft(level: Experience) -> f64 {
    let metres = match level {
        Experience::Ace => 10.,
        Experience::Experienced => 35.,
        Experience::Average => 60.,
        Experience::Novice => 85.,
    };
    metres * FEET_PER_METRE
}

/// The distance within which the gun's cue is near enough the target to
/// fire and to be laid on the walked aim, at `range_ft` slant:
/// [`GUN_TRACK_DEG`] of arc, at least [`GUN_GATE_MIN_FT`].
pub fn gun_track_ft(range_ft: f64) -> f64 {
    (range_ft * GUN_TRACK_DEG.to_radians().tan()).max(GUN_GATE_MIN_FT)
}

/// How long a gun burst takes to walk its rounds onto the target, by skill
/// (John, 2026-10-12; the times are the agent's, opinionated): a burst opens
/// on its drawn offset point and the aim walks in a straight line onto the
/// target, so the rounds that follow land on it. An Ace is on in 0.1 s, a
/// Novice in 0.4 s of a 0.5 s burst.
pub fn gun_walk_in_s(level: Experience) -> f64 {
    match level {
        Experience::Ace => 0.1,
        Experience::Experienced => 0.2,
        Experience::Average => 0.3,
        Experience::Novice => 0.4,
    }
}

/// How much of a burst's drawn offset is left `elapsed_ticks` after it
/// opened: 1 at the start, 0 from the end of the walk-in on.
pub fn walk_in_left(level: Experience, elapsed_ticks: u64) -> f64 {
    (1. - elapsed_ticks as f64 / 120. / gun_walk_in_s(level)).clamp(0., 1.)
}

/// One pass's seeded draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimDraw {
    /// The aim point's ground offset from the target, feet along x and z.
    pub offset_ft: [f64; 2],
    /// It lies inside the circle (90% of passes).
    pub inside: bool,
    /// The exit jink's side, +1 right or -1 left.
    pub side: f64,
}

/// The pass's aim offset (spec, "Skill and release precision"): a
/// deterministic function of the mission's seed, the aircraft and the pass
/// number, the same on every host and in a replay. 90% of passes inside the
/// circle of radius `radius_ft`, uniform by area (distance `R sqrt(u)`); 10%
/// in the ring from R to 2R (distance `R sqrt(1 + 3u)`); a uniform bearing.
/// It owns no generator state: the same three inputs give the same draw.
pub fn aim_draw(seed: u64, aircraft: u32, pass: u32, radius_ft: f64) -> AimDraw {
    let mut key = seed
        ^ u64::from(aircraft).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(pass).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ 0x5354_5249_4B45_0001;
    let mut unit = || {
        key = mix(key);
        (key >> 11) as f64 / (1u64 << 53) as f64
    };
    let band = unit();
    let spread = unit();
    let turn = unit();
    let side = unit();
    let inside = band < IN_CIRCLE_SHARE;
    let distance = if inside {
        radius_ft * spread.sqrt()
    } else {
        radius_ft * (1. + 3. * spread).sqrt()
    };
    let angle = turn * std::f64::consts::TAU;
    AimDraw {
        offset_ft: [distance * angle.sin(), distance * angle.cos()],
        inside,
        side: if side < 0.5 { -1. } else { 1. },
    }
}

/// SplitMix64's finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The release floor of a fighter's or attack jet's dive or level release
/// (spec): 3,000 ft under Clear or Air, 5,000 ft under Guns or Missiles.
pub fn release_floor_ft(threat: ThreatLevel) -> f64 {
    match threat {
        ThreatLevel::Clear | ThreatLevel::Air => FLOOR_CLEAR_FT,
        ThreatLevel::Guns | ThreatLevel::Missiles => FLOOR_DEFENDED_FT,
    }
}

/// A heavy's minimum release height above the ground (spec, "Heavy
/// bombers"; John, 2026-10-11): max(5,000 ft, the mission's airborne
/// height), plus 5,000 ft under Guns and 10,000 ft under Missiles, never
/// above 90% of its ceiling (`ceiling_agl_ft`, above the same ground).
pub fn heavy_floor_ft(
    mission_height_ft: Option<f64>,
    threat: ThreatLevel,
    ceiling_agl_ft: f64,
) -> f64 {
    let base = HEAVY_MIN_FT.max(mission_height_ft.unwrap_or(0.));
    let raise = match threat {
        ThreatLevel::Clear | ThreatLevel::Air => 0.,
        ThreatLevel::Guns => HEAVY_RAISE_GUNS_FT,
        ThreatLevel::Missiles => HEAVY_RAISE_MISSILES_FT,
    };
    let cap = HEAVY_CEILING_SHARE * ceiling_agl_ft;
    if cap.is_finite() && cap > 0. {
        (base + raise).min(cap)
    } else {
        base + raise
    }
}

/// The retail turn radius, V² over 43.6 G, feet (V in ft/s).
pub fn turn_radius_ft(speed_fps: f64, g: f64) -> f64 {
    speed_fps * speed_fps / (43.6 * g.max(1.))
}

/// Retail's dive gate without its dice: height above the ground and
/// horizontal distance both at least max(10,000 ft, 2R).
pub fn dive_gate(agl_ft: f64, distance_ft: f64, radius_ft: f64) -> bool {
    let min = DIVE_GATE_MIN_FT.max(2. * radius_ft);
    agl_ft >= min && distance_ft >= min
}

/// Energy is Good at or above the corner speed (spec).
pub fn energy_good(own: &OwnState) -> bool {
    own.speed.0 >= own.limits.corner.0
}

/// Where a pop-up pulls up, feet from the target.
fn popup_pull_at(kind: StoreKind) -> f64 {
    match kind {
        StoreKind::Rocket => ROCKET_POPUP_PULL_AT_FT,
        StoreKind::Bomb => POPUP_PULL_AT_FT + BOMB_POPUP_PULL_LEAD_FT,
        _ => POPUP_PULL_AT_FT,
    }
}

/// How far down the target is when a pop-up's pull ends, degrees.
fn popup_dive_deg(kind: StoreKind) -> f64 {
    if kind == StoreKind::Rocket {
        ROCKET_POPUP_DIVE_DEG
    } else {
        POPUP_DIVE_DEG
    }
}

/// The depression a dive rolls in at (slice ST-T2): a bomb's steep dive,
/// a rocket's moderate one.
pub fn dive_rollin_deg(kind: StoreKind) -> f64 {
    if kind == StoreKind::Rocket {
        ROCKET_DIVE_ROLLIN_DEG
    } else {
        DIVE_ROLLIN_DEG
    }
}

/// The height a pull-out from a dive of `pitch_deg` (negative down) at
/// `speed_fps` and `g` loses (slice ST-T2, fitted, agent): the physical
/// pull-out arc, radius V² / (g (G - 1)), plus [`PULL_OUT_REACTION_S`] of
/// the sink before it starts. Retail's turn radius is far tighter than the
/// aircraft flies, so the plan uses the physical one.
pub fn pull_out_loss_ft(speed_fps: f64, g: f64, pitch_deg: f64) -> f64 {
    let dive = (-pitch_deg).clamp(0., 90.).to_radians();
    let radius = speed_fps * speed_fps / (32.174 * (g - 1.).max(0.5));
    radius * (1. - dive.cos()) + speed_fps * dive.sin() * PULL_OUT_REACTION_S
}

/// Where a dive pulls out, feet above the ground (slice ST-T2): high enough
/// to bottom out [`PULL_OUT_CLEARANCE_FT`] over the ground from the present
/// dive at the present speed, and never below retail's 2,000 ft. Retail
/// pulls out at max(2R, 2,000 ft) with its own turn radius; the plan
/// replaces 2R (opinionated, agent).
pub fn dive_pull_out_ft(own: &OwnState) -> f64 {
    DIVE_PULL_OUT_MIN_FT.max(
        pull_out_loss_ft(own.speed.0, own.g_limit, own.flight_path_pitch_deg)
            + PULL_OUT_CLEARANCE_FT,
    )
}

/// The planned dive's terrain floor (slice ST-T2, the exception John
/// allowed): the steepest flight path, in the B44 floor's 5 degree steps,
/// from which the pull-out still bottoms out [`PULL_OUT_CLEARANCE_FT`]
/// over the highest ground ahead, and climbs when rising ground ahead asks
/// for a climb. It replaces the general B44 floor, whose dive
/// test assumes retail's tight turn radius, only while a striker flies a
/// planned dive.
pub fn planned_pitch_floor_deg(own: &OwnState) -> f64 {
    planned_pitch_floor_with(own, PULL_OUT_CLEARANCE_FT)
}

/// [`planned_pitch_floor_deg`] for a pull-out that bottoms out `clearance_ft`
/// over the highest ground ahead (a gun run's is [`STRAFE_CLEARANCE_FT`]).
pub fn planned_pitch_floor_with(own: &OwnState, clearance_ft: f64) -> f64 {
    let margin = own.altitude_msl_ft - own.terrain_ahead_ft - clearance_ft;
    let floor = (0..=16)
        .map(|i| -MAX_DIVE_DEG + 5. * f64::from(i))
        .find(|pitch| pull_out_loss_ft(own.speed.0, own.g_limit, *pitch) <= margin)
        .unwrap_or(10.);
    // The general floor's glide-slope term (no descent through the ground
    // at the end of the look-ahead) is what holds a dive shallow; a planned
    // dive keeps only a climb that rising ground ahead asks for.
    if own.terrain_climb_deg > 0. {
        floor.max(own.terrain_climb_deg.min(90.))
    } else {
        floor
    }
}

/// A drag bomb's laydown height above the target (slice ST-T2, fitted,
/// agent): the lowest whole 50 ft from [`LAYDOWN_MIN_FT`] at which a level
/// release at `speed_fps` clears the sight's LOW rule (the fall lasts the
/// fuze's arming time, with [`LOW_MARGIN`]) and its FRAG rule (the aircraft,
/// flying on level, is beyond the fragments' reach when the bomb bursts,
/// with [`FRAG_MARGIN`]), using the shared bomb flight model.
pub fn laydown_height_ft(store: &Store, speed_fps: f64) -> f64 {
    laydown_height_at(store, speed_fps, 0.)
}

fn laydown_height_at(store: &Store, speed_fps: f64, ground: f64) -> f64 {
    let safe = |height: f64| {
        let release = crate::combat::ballistics::ReleaseState {
            position: [0., ground + height, 0.],
            velocity: [0., 0., speed_fps],
            body: store.bomb_body,
        };
        let Some(impact) = crate::combat::ballistics::impact_at(release, ground) else {
            return false;
        };
        let fall = impact.seconds;
        let behind = speed_fps * fall - impact.position[2];
        fall >= LOW_MARGIN * store.fuze_arm_s
            && behind.hypot(height) >= FRAG_MARGIN * store.frag_radius_ft
    };
    (0..200)
        .map(|i| LAYDOWN_MIN_FT + 50. * f64::from(i))
        .find(|h| safe(*h))
        .unwrap_or(LAYDOWN_MIN_FT + 10_000.)
}

/// A gun burst's aim (slice ST-T2): the pass's draw rule ([`aim_draw`])
/// keyed also on the burst number, so each burst has its own point in the
/// circle (90%) or the ring (10%), the same on every host.
pub fn burst_draw(seed: u64, aircraft: u32, pass: u32, burst: u32, radius_ft: f64) -> AimDraw {
    let key = seed ^ mix(u64::from(burst) ^ 0x4755_4E00_0000_0000);
    aim_draw(key, aircraft, pass, radius_ft)
}

/// The straight-line distance between two points, feet.
fn slant(a: Vector, b: Vector) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// The retail steering frame (spec, "The steering frame"): straight at the
/// point; a pitch within 3 degrees snaps level while the point is 3,000 ft
/// or more away; clamped to the 80 degree climb and dive limits. Returns
/// (heading, pitch) in degrees.
pub fn steering_frame(own: Vector, point: Vector) -> (f64, f64) {
    let heading = bearing_deg(own, point);
    let run = horizontal(own, point);
    let rise = point[1] - own[1];
    let mut pitch = rise.atan2(run).to_degrees();
    if pitch.abs() < 3. && (run * run + rise * rise).sqrt() >= 3_000. {
        pitch = 0.;
    }
    (heading, pitch.clamp(-MAX_DIVE_DEG, MAX_DIVE_DEG))
}

/// The heading that moves the cue onto the aim across the track (fitted,
/// agent): the body heading (which the adapter steers, and which a bomb
/// leaves along) turned by [`CUE_HEADING_GAIN`] times the angle, seen from
/// the aircraft, between the aim and the line the cue sweeps along the
/// track.
pub fn cue_heading_deg(own: &OwnState, velocity: Vector, aim: Vector, impact: Vector) -> f64 {
    let Some(u) = track_unit(velocity) else {
        return bearing_deg(own.position, aim);
    };
    let right = [u[1], -u[0]];
    let cross = (aim[0] - impact[0]) * right[0] + (aim[2] - impact[2]) * right[1];
    // The slant, not the ground distance: in a steep dive a small heading
    // change already moves the cue far across (slice ST-T2).
    let reach = slant(own.position, aim).max(500.);
    // `right` points to the right of the track (headings turn clockwise,
    // from +z toward +x), so an aim to the right turns the heading up.
    let correction = (CUE_HEADING_GAIN * cross.atan2(reach).to_degrees()).clamp(-30., 30.);
    wrap_deg(own.heading_deg + correction)
}

/// Whether flying `pitch` (degrees) asks the adapter for less than
/// [`PUSH_G`]: a push, in which a banked aircraft turns the wrong way.
pub fn pushes(own: &OwnState, pitch: f64) -> bool {
    let error = (pitch - own.flight_path_pitch_deg).to_radians();
    let g = own.flight_path_pitch_deg.to_radians().cos()
        + own.speed.0.max(100.) * error / ADAPTER_PITCH_S / 32.174;
    g < PUSH_G
}

/// The horizontal direction of travel, when the aircraft moves.
fn track_unit(velocity: Vector) -> Option<[f64; 2]> {
    let norm = velocity[0].hypot(velocity[2]);
    (norm > 1.).then(|| [velocity[0] / norm, velocity[2] / norm])
}

/// How far above the horizon `point` is, degrees (negative below).
pub fn elevation_deg(own: Vector, point: Vector) -> f64 {
    -depression_deg(own, point)
}

/// The pitch that holds `altitude`: toward a point 6,000 ft ahead at that
/// altitude, within 10 degrees, with the steering frame's 3 degree snap.
pub fn hold_pitch_deg(current_ft: f64, altitude_ft: f64) -> f64 {
    let pitch = (altitude_ft - current_ft).atan2(6_000.).to_degrees();
    if pitch.abs() < 0.5 {
        0.
    } else {
        pitch.clamp(-10., 10.)
    }
}

/// How far below the horizon `point` is, degrees (negative above).
pub fn depression_deg(own: Vector, point: Vector) -> f64 {
    (own[1] - point[1])
        .atan2(horizontal(own, point).max(1e-6))
        .to_degrees()
}

/// The closest point of a cue's path to the aim, found inside one tick
/// (slice nr-cue).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Closest {
    /// Where the cue lies from the aim at that moment, feet along x and z.
    pub at: [f64; 2],
    /// Its distance from the aim, feet.
    pub distance: f64,
    /// When, as a share of the tick that ended with this sample: 0 is the
    /// previous sample (the cue turned back there), 1 the new one.
    pub fraction: f64,
}

/// Whether the cue's closest approach to the aim fell inside the tick that
/// took it from `before` to `now` (both: where the cue lies from the aim,
/// feet along x and z), and where. Between two ticks the cue's position
/// against the aim is taken as a straight line, so the closest point is
/// exact whatever the tick rate; an approach is counted in exactly one tick:
///
/// - inside the tick, when the cue was closing at its start and is not at
///   its end (`fraction` in (0, 1]);
/// - at the tick's start, when the cue was still closing at the end of the
///   tick before and is opening from the first moment of this one (a cue
///   path that bends at a sample; `fraction` 0).
///
/// `closing` says the cue was still closing on the aim at the end of the
/// tick before; the second answer is the same flag for the end of this tick.
/// A cue that has not moved has no closest point.
pub fn closest_approach(before: [f64; 2], now: [f64; 2], closing: bool) -> (Option<Closest>, bool) {
    let step = [now[0] - before[0], now[1] - before[1]];
    let span = step[0] * step[0] + step[1] * step[1];
    if span <= 0. {
        return (None, false);
    }
    let start = before[0] * step[0] + before[1] * step[1];
    let end = now[0] * step[0] + now[1] * step[1];
    let fraction = if start >= 0. {
        // Opening from the first moment: the closest point is the sample
        // before, if the cue was closing when it got there.
        closing.then_some(0.)
    } else if end >= 0. {
        Some(-start / span)
    } else {
        None
    };
    let found = fraction.map(|f| {
        let at = [before[0] + step[0] * f, before[1] + step[1] * f];
        Closest {
            at,
            distance: at[0].hypot(at[1]),
            fraction: f,
        }
    });
    (found, end < 0.)
}

pub fn horizontal(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// Heading from `from` to `to`, degrees, 0 along +z, clockwise to +x.
pub fn bearing_deg(from: Vector, to: Vector) -> f64 {
    wrap_deg((to[0] - from[0]).atan2(to[2] - from[2]).to_degrees())
}

fn wrap_deg(deg: f64) -> f64 {
    deg.rem_euclid(360.)
}

/// `to - from`, the short way, -180..180.
pub fn angle_difference(from: f64, to: f64) -> f64 {
    (to - from + 180.).rem_euclid(360.) - 180.
}

/// Where the picture knows `id` stands.
fn known_position(picture: &GroundPicture, id: u32) -> Option<Vector> {
    picture
        .briefed()
        .iter()
        .find(|b| b.id == id)
        .map(|b| b.position)
        .or_else(|| {
            picture
                .seen()
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.position)
        })
        .or_else(|| picture.contact(id).map(|c| c.position))
}

fn standing(picture: &GroundPicture, id: u32) -> bool {
    match picture.briefed().iter().find(|b| b.id == id) {
        Some(b) => b.standing,
        None => picture.seen().iter().any(|s| s.id == id),
    }
}

fn objective_point(picture: &GroundPicture) -> Option<Vector> {
    picture.objective().and_then(|o| o.point)
}

fn target_facts(picture: &GroundPicture, id: u32) -> TargetFacts {
    let briefed = picture.briefed().iter().find(|b| b.id == id);
    let seen = picture.seen().iter().find(|s| s.id == id);
    let radar = briefed
        .and_then(|b| b.defense)
        .or_else(|| seen.and_then(|s| s.defense))
        .is_some_and(|d| d.radar);
    let emitting =
        seen.is_some_and(|s| s.emitting) || picture.contact(id).is_some_and(|c| c.emitting);
    TargetFacts { radar, emitting }
}

#[path = "strike_checkpoint.rs"]
mod checkpoint;

#[path = "strike_takeover.rs"]
mod takeover;
pub use takeover::{STAND_OFF_MARGIN_FT, Takeover};
#[path = "family_gunship.rs"]
pub mod gunship;
#[path = "family_helicopter.rs"]
pub mod helicopter;

#[cfg(test)]
#[path = "strike_tests.rs"]
mod tests;
