//! Per-actor controller (AI-3): one aircraft's decision loop.
//!
//! [`Controller::step`] is the per-actor entry point proposed in
//! [`docs/spec/ai.md`](../../../../docs/spec/ai.md), "Proposed host API". It
//! takes one immutable snapshot per advancing 120 Hz tick and returns an
//! [`IntentBatch`]. It never moves a body, applies damage, creates a
//! projectile or reads the renderer; the host applies the intents.
//!
//! The controller owns no rules of its own. It sequences the isolated
//! components:
//!
//! | Concern | Component |
//! | --- | --- |
//! | Reason ranking, launch warnings, countermeasures | [`threat`](super::threat) |
//! | Target retention, eligibility and ranking (B41) | [`targeting`](super::targeting) |
//! | Ahead/facing/off-beam and distances (B01 to B03) | [`geometry`](super::geometry) |
//! | Tactical choice (B10 to B14) | [`tactics`](super::tactics) |
//! | Request limits and the deadline clock (B13) | [`motion`](super::motion) |
//! | Pursuit frame, speed regulation and lead (B15, B44) | [`pursuit`](super::pursuit) |
//! | Steering rates and the terrain floor (B44) | [`steering`](super::steering) |
//! | Weapon cadence and ammunition (B42, B45) | [`weapon_service`](super::weapon_service) |
//! | Wing commands and formation (B43, B46) | [`wing`](super::wing) |
//! | Waypoints and fuel (B48) | [`route`](super::route) |
//! | Per-level numbers | [`experience`](super::experience) |
//!
//! Where a component reports [`AiError::UnspecifiedRule`] the controller must
//! not stall: it applies the matching named rule from [`fitted`](super::fitted)
//! and records the [`Fallback`] in the returned batch and in its own
//! [`FallbackLog`]. Every such rule is fitted, not recovered retail behavior.
//!
//! Determinism: the controller holds one seeded [`DecisionRandom`]. It draws
//! only at documented decision points, never once per tick. A tactic is chosen
//! on the fitted cadence in
//! [`fitted::tactical_cadence_quarters`](super::fitted::tactical_cadence_quarters)
//! or when the active maneuver ends, whichever comes first, so calling `step`
//! more often does not reroll anything. A tick that does not advance is a
//! no-op that repeats the previous batch.

use tore_formats::aircraft::AircraftId;

use super::experience::{self, ResolvedExperience};
use super::fitted::{self, Fallback};
use super::geometry::{self, KnownTarget, OwnPose, RelativeAngles, TargetPose};
use super::motion::{
    self, Bank, CommandClock, Deadline, Duration, ManeuverFrame, MotionRequest, PitchRequest,
    SpeedRequest,
};
use super::pursuit::{self, PursuitOffset};
use super::route::{self, FuelState};
use super::steering::{self, AxisRates, CommandMode, SteeringRequest, TerrainInputs};
use super::tactics::{
    self, BehaviorChoice, MissileLaunch, MissileReaction, MissileReactionInputs, PursuitCondition,
    QuadrantThresholds, TacticalSituation, TacticalThresholds, TargetClass,
};
use super::targeting::{self, CandidateTarget, ObjectId, SelectionRoute, Selector, SelectorKind};
use super::thought::{
    self, ActiveManeuverView, ControllerTrace, FormationTrace, FuelTrace, LockTrace, ManeuverPath,
    ManeuverTrace, MissionTargetRefusal, MotionBranch, PitchSource, PursuitTrace, ResolveTrace,
    StationTrace, StepPath, TargetPath, WarningStep, WarningTrace, WeaponTrace,
};
use super::threat::{
    self, AttackState, FlightState, ScriptReason, ScriptStart, SeekerClass, TimeOfDay,
    WarningDelay, WarningInputs, WarningReaction, WarningTarget,
};
use super::weapon_service::{
    self, ActorId, Delay, LockStatus, ProjectilePacing, ServiceInputs, ServiceOutcome, StationId,
    StoreCandidate, StoreCapability, TargetId as WeaponTargetId, TimingProfile,
};
use super::wing::{
    self, AppliedSetting, Formation, FormationVariation, MotionDuration, MotionSummary,
    ReceiverOutcome, RecipientState, SendOutcome, SenderState, TargetId as WingTargetId,
    WingControl, WingRequest,
};
use super::{AiError, DecisionRandom, Result, ScalarSpeed, SpeedLimits};

const SEARCH_ORBIT_ENTRY_FT: f64 = 6_076.0;
const SEARCH_ORBIT_RADIUS_FT: f64 = 4_557.0;
const SEARCH_ORBIT_CORRECTION_BAND_FT: f64 = 1_519.0;
const SEARCH_ORBIT_MAX_CORRECTION_DEG: f64 = 45.0;
const SEARCH_COMMAND_SECONDS: u8 = 3;
const ACE_SEARCH_LIMIT_TICKS: u64 = 120 * 120;

/// The behavior family an actor belongs to.
///
/// All twelve ported aircraft bind to the fighter/strike family
/// (AI experience spec, "Currently ported aircraft"). The other retail
/// families are recorded there and are not implemented, so constructing one is
/// an explicit error rather than a silent substitution of fighter behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehaviorFamily {
    FighterStrike,
    F117,
    Helicopter,
    Bomber,
    AC130,
    LargeAircraft,
    Airliner,
    Moth,
}

/// The actor's mission role, kept separate from its family and its capability.
///
/// A Su-25 uses the fighter/strike family in a ground-attack role; the role
/// never changes which family supplies its decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionRole {
    AirToAir,
    AirToGround,
    Escort,
}

/// Stable per-actor identity. Family, capability, role and experience stay
/// independent inputs, as M1e requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorIdentity {
    pub actor: ActorId,
    pub side: targeting::Side,
    /// Wing index on the setup screen, 0..=2 per side.
    pub wing: u8,
    /// Member index inside the wing, from 0. It is the aircraft's place in the
    /// roster and never changes; who leads is [`Self::leads`].
    pub member: u8,
    /// This aircraft currently leads its wing. At the start it is the wing's
    /// first member (member 0); lead succession moves it.
    pub leads: bool,
    /// The exact aircraft record. `F18` is the F/A-18D, `RafaleC` the Rafale C.
    pub aircraft: AircraftId,
    /// Human control. No AI path may set this true; it gates the experience G
    /// adjustment and the B45 launch-context G check.
    pub human_controlled: bool,
}

impl ActorIdentity {
    pub fn is_leader(&self) -> bool {
        self.leads
    }
}

/// Behavior binding for an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BehaviorProfile {
    pub family: BehaviorFamily,
    pub role: MissionRole,
}

/// The actor's own observed state for one tick. No renderer state and no
/// shared player globals; every quantity is the actor's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnState {
    pub position: [f64; 3],
    pub heading_deg: f64,
    /// Flight-path pitch. Body pitch is this plus `body_pitch_offset_deg`.
    pub flight_path_pitch_deg: f64,
    pub body_pitch_offset_deg: f64,
    pub bank_deg: f64,
    pub speed: ScalarSpeed,
    /// The loaded envelope limits at the current altitude (B04, B15).
    pub limits: SpeedLimits,
    pub altitude_msl_ft: f64,
    pub agl_ft: f64,
    /// Highest terrain along the speed-scaled look-ahead, for the B44 floor.
    pub terrain_ahead_ft: f64,
    /// Flight-path climb that clears the look-ahead's terrain by the minimum
    /// altitude, degrees; -90 when none is needed or not measured.
    pub terrain_climb_deg: f64,
    /// The record's minimum-altitude value; 300 in every inspected record.
    pub minimum_altitude_ft: f64,
    pub at_ceiling: bool,
    pub on_ground: bool,
    /// Loaded G limit after damage, hit-point and load reductions, already
    /// carrying the experience adjustment for an AI actor.
    pub g_limit: f64,
    pub roll_limit_deg_per_s: f64,
    pub maximum_bank_deg: f64,
    pub alive: bool,
    /// Endurance at cruise, seconds. B48.
    pub fuel_endurance_s: f64,
    /// Time to reach the home airport at cruise, seconds; `None` with no home.
    pub time_home_s: Option<f64>,
    pub internal_fuel_lbs: f64,
    pub radar_emitting: bool,
    /// The aircraft is a helicopter or the V-22 flown through the rotor
    /// adapter (slice H3): its flight home is planned and flown at
    /// [`route::rotorcraft_cruise_speed`], and its fuel states use
    /// [`route::rotorcraft_fuel_state`], not B48's fighter rules.
    pub rotorcraft: bool,
}

impl OwnState {
    pub fn body_pitch_deg(&self) -> f64 {
        self.flight_path_pitch_deg + self.body_pitch_offset_deg
    }
}

/// One permitted target view. The host builds these from the actor's own
/// sensors; `Observable` is service input, never automatic AI knowledge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetView {
    /// Observed IR signature relative to 100, multiplied by engine heat.
    pub infrared_strength: f64,
    pub id: u32,
    pub side: targeting::Side,
    pub position: [f64; 3],
    pub heading_deg: f64,
    pub pitch_deg: f64,
    pub speed: ScalarSpeed,
    pub maximum_speed: ScalarSpeed,
    pub is_aircraft: bool,
    pub is_fighter: bool,
    pub human_controlled: bool,
    pub valid: bool,
    pub type_allowed: bool,
    pub seeker_eligible: bool,
    /// Members of this actor's wing already attacking this target (B41).
    pub wing_attackers: u32,
    /// Terrain blocks the firing path.
    pub terrain_blocked: bool,
    pub sensor_supported: bool,
    /// The view is built from a flightmate's track in the flight data link,
    /// not from this actor's own sensors (slice G3b). The actor may fly toward
    /// it and keep it as its target, but never fires on it: a launch needs a
    /// target the actor holds itself.
    pub link_track: bool,
}

/// A frozen aircraft observation that may be investigated but never attacked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchContact {
    pub id: u32,
    pub position: [f64; 3],
    pub observed_tick: u64,
}

/// Perceived missile defense resolved by the shared defensive policy. It
/// contains no hidden target or launcher information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefenseSource {
    Missile,
    IncomingFire,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenseMotion {
    pub source: DefenseSource,
    pub heading_deg: f64,
    pub pitch_deg: f64,
    /// A helicopter's defence (slice ST-H, docs/spec/ai-strike.md,
    /// "Helicopter defence"): flown at maximum speed, not corner speed, which
    /// sinks a helicopter.
    pub rotorcraft: bool,
    /// Turn cold at maximum dry speed instead of corner speed.
    pub extend: bool,
}

/// A launch warning delivered to this actor and nobody else (B47).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThreatReport {
    pub missile_id: u32,
    pub seeker: SeekerClass,
    pub launcher_id: u32,
    pub launcher_same_side: bool,
    /// Missile-to-target separation at launch, feet.
    pub distance_at_launch_ft: f64,
    pub launch_tick: u64,
}

/// Frame events, as proposed in the spec's "Records and required inputs".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameEvent {
    ThreatReported(ThreatReport),
    Hit,
    TargetUnavailable(u32),
    ActorRemoved(u32),
}

/// The actor's wing context for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WingView {
    pub control: WingControl,
    pub formation: Formation,
    pub horizontal_spacing_ft: i32,
    pub vertical_spacing_ft: i32,
    /// Formation slot, 1..=9 for a wingman.
    pub slot: u8,
    /// Leader pose and speed, for the formation point and mode 9 speed.
    pub leader: Option<LeaderView>,
    /// Wingmen currently in formation, for B43 target sharing.
    pub wingmen_in_formation: u32,
    /// This actor's wing is in combat, and in approach (B11, B12 inputs).
    pub wing_combat: bool,
    pub wing_approach: bool,
    /// The unresolved B12 wing-approach value. `None` leaves the wing-split
    /// branch untried rather than substituting ordinary target distance.
    pub wing_approach_value_ft: Option<f64>,
}

/// A leader's pose as seen by its wingman.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeaderView {
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub heading_deg: f64,
    pub speed: ScalarSpeed,
    pub target: Option<u32>,
    /// The leader is landing (B48 join-landing reads this).
    pub recovering: bool,
    /// The leader is on the ground. Wingmen do not formate on it.
    pub on_ground: bool,
}

/// One carried store, as the weapon service sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationView {
    pub station: StationId,
    pub guided: bool,
    pub capability: StoreCapability,
    pub inhibited: bool,
    pub rounds: weapon_service::Rounds,
    /// Pointing error from this store's mount to the target, degrees.
    pub pointing_error_deg: f64,
    /// The employment envelope's horizontal angular limit, if it has one.
    pub employment_limit_deg: Option<f64>,
    /// The store passes its employment envelope against the current target.
    pub employment_fit: Option<f64>,
    pub minimum_range_ft: f64,
    pub maximum_range_ft: Option<f64>,
    /// Energy-model permission for this target; Some(None) is pending or unreachable.
    pub energy_target: Option<Option<u32>>,
    /// All-aspect flag and nominal IR seeker reach, absent for other stores.
    pub infrared_envelope: Option<(bool, f64)>,
    pub requires_radar: bool,
    pub requires_sensor: bool,
    pub employment_zone: Option<tore_formats::weapons::Zone>,
    pub mount: [f64; 3],
    pub damage_vs_category: f64,
    /// Nominal store speed, for the fitted lead prediction.
    pub store_speed: ScalarSpeed,
    /// The weapon's own tracking delay once locked (B42).
    pub tracking_delay: Delay,
    pub pacing: ProjectilePacing,
}

/// Mount-relative geometry of one store against one target.
struct MountGeometry {
    basis: crate::attitude::Basis,
    origin: [f64; 3],
    range: f64,
    error: f64,
}

impl StationView {
    fn mount_geometry(&self, own: &OwnState, target: &TargetView) -> MountGeometry {
        let basis = crate::attitude::Basis::new(
            own.heading_deg.to_radians(),
            own.body_pitch_deg().to_radians(),
            own.bank_deg.to_radians(),
        );
        let origin = std::array::from_fn(|i| {
            own.position[i]
                + basis.right[i] * self.mount[0]
                + basis.up[i] * self.mount[1]
                + basis.forward[i] * self.mount[2]
        });
        let delta = crate::combat::missiles::sub(target.position, origin);
        let range = crate::combat::missiles::length(delta);
        let forward = crate::attitude::dot(delta, basis.forward);
        let right = crate::attitude::dot(delta, basis.right);
        let up = crate::attitude::dot(delta, basis.up);
        let error = right
            .atan2(forward)
            .to_degrees()
            .abs()
            .max(up.atan2(forward.hypot(right)).to_degrees().abs());
        MountGeometry {
            basis,
            origin,
            range,
            error,
        }
    }

    fn infrared_fit(&self, origin: [f64; 3], target: &TargetView) -> bool {
        self.infrared_envelope.is_none_or(|(all_aspect, nominal)| {
            let direction =
                crate::attitude::unit(crate::combat::missiles::sub(origin, target.position));
            let facing = crate::attitude::dot(
                crate::attitude::Basis::new(
                    target.heading_deg.to_radians(),
                    target.pitch_deg.to_radians(),
                    0.,
                )
                .forward,
                direction,
            );
            (if self.energy_target.is_some() {
                crate::combat::missiles::seeker::energy_infrared_quality
            } else {
                crate::combat::missiles::seeker::infrared_quality
            })(
                all_aspect,
                nominal,
                target.infrared_strength,
                facing,
                crate::combat::missiles::length(crate::combat::missiles::sub(
                    origin,
                    target.position,
                )),
            ) >= 0.25
        })
    }

    /// Current mount-relative pointing error after a successful employment
    /// check. This is shared by diagnostics and release, avoiding stale target
    /// geometry when selection changes during the decision. It stops at the
    /// first failing reason; [`Self::employment_check`] reports them all.
    pub fn employment_error(&self, own: &OwnState, target: &TargetView) -> Option<f64> {
        let MountGeometry {
            basis,
            origin,
            range,
            error,
        } = self.mount_geometry(own, target);
        let permitted = self.energy_target.is_none_or(|id| id == Some(target.id))
            && self.infrared_fit(origin, target)
            && range >= self.minimum_range_ft
            && self.maximum_range_ft.is_none_or(|max| range <= max)
            && self.employment_limit_deg.is_none_or(|limit| error <= limit)
            && self.employment_zone.is_none_or(|zone| {
                crate::combat::missiles::geometry(&zone, origin, basis, target.position, None)
            });
        permitted.then_some(error)
    }

    /// Every B45 employment reason against `target`, with the same
    /// arithmetic as [`Self::employment_error`]: it permits exactly when this
    /// check has no failure, and its error is this check's `error_deg`. For
    /// explanation only; no decision reads it.
    // Each failure is the exact negation of `employment_error`'s test, so a
    // NaN range or error fails here exactly when it fails there.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn employment_check(&self, own: &OwnState, target: &TargetView) -> EmploymentCheck {
        let MountGeometry {
            basis,
            origin,
            range,
            error,
        } = self.mount_geometry(own, target);
        EmploymentCheck {
            range_ft: range,
            error_deg: error,
            below_minimum_range: !(range >= self.minimum_range_ft),
            beyond_maximum_range: self.energy_target.is_some_and(|id| id != Some(target.id))
                || !self.maximum_range_ft.is_none_or(|max| range <= max),
            beyond_angle_limit: !self.employment_limit_deg.is_none_or(|limit| error <= limit),
            outside_zone: !self.infrared_fit(origin, target)
                || !self.employment_zone.is_none_or(|zone| {
                    crate::combat::missiles::geometry(&zone, origin, basis, target.position, None)
                }),
        }
    }

    /// The B45 store filters for this station against `target`, in the order
    /// the store choice applies them. The first failing filter is the verdict;
    /// a store that passes them all is usable with its pointing error. Pure:
    /// the store choice itself uses exactly this.
    pub fn verdict(
        &self,
        own: &OwnState,
        target: &TargetView,
        class: weapon_service::TargetClass,
    ) -> StationVerdict {
        if self.inhibited {
            return StationVerdict::Inhibited;
        }
        if self.requires_radar && !own.radar_emitting {
            return StationVerdict::RadarOff;
        }
        if self.requires_sensor && !target.sensor_supported {
            return StationVerdict::NoSensorTrack;
        }
        if !weapon_service::store_eligible(self.capability, class) {
            return StationVerdict::WrongTargetClass;
        }
        if matches!(self.rounds, weapon_service::Rounds::Finite(0)) {
            return StationVerdict::Empty;
        }
        match self.employment_error(own, target) {
            Some(error_deg) => StationVerdict::Usable { error_deg },
            None => StationVerdict::OutsideEnvelope,
        }
    }
}

/// Why one store can or cannot be employed against the current target (the
/// B45 employment envelope), every reason evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmploymentCheck {
    /// Distance from the store's mount to the target, feet.
    pub range_ft: f64,
    /// The larger of the horizontal and vertical pointing errors from the
    /// mount, degrees.
    pub error_deg: f64,
    pub below_minimum_range: bool,
    pub beyond_maximum_range: bool,
    pub beyond_angle_limit: bool,
    /// Outside the store's employment zone; never true without a zone.
    pub outside_zone: bool,
}

/// One failing employment reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmploymentFailure {
    BelowMinimumRange,
    BeyondMaximumRange,
    BeyondAngleLimit,
    OutsideZone,
}

impl EmploymentCheck {
    /// Whether the store may be employed.
    pub fn permitted(&self) -> bool {
        self.first_failure().is_none()
    }

    /// The reason [`StationView::employment_error`] stops at: the first
    /// failure in the order it tests them.
    pub fn first_failure(&self) -> Option<EmploymentFailure> {
        self.failures().next()
    }

    /// Every failing reason, in that order.
    pub fn failures(&self) -> impl Iterator<Item = EmploymentFailure> {
        [
            (
                self.below_minimum_range,
                EmploymentFailure::BelowMinimumRange,
            ),
            (
                self.beyond_maximum_range,
                EmploymentFailure::BeyondMaximumRange,
            ),
            (self.beyond_angle_limit, EmploymentFailure::BeyondAngleLimit),
            (self.outside_zone, EmploymentFailure::OutsideZone),
        ]
        .into_iter()
        .filter_map(|(failed, reason)| failed.then_some(reason))
    }
}

/// The first B45 store filter one station fails, or its pointing error when
/// it passes them all ([`StationView::verdict`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StationVerdict {
    Inhibited,
    /// It needs the aircraft's own radar, which is not emitting.
    RadarOff,
    /// It needs a sensor track the target does not have.
    NoSensorTrack,
    /// It cannot be used against this class of target (air or surface).
    WrongTargetClass,
    Empty,
    /// Outside its employment envelope; [`StationView::employment_check`]
    /// says why.
    OutsideEnvelope,
    /// It may be employed; its mount-relative pointing error, degrees.
    Usable {
        error_deg: f64,
    },
}

/// Route and recovery context (B48).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteView {
    pub home_airport: Option<route::Position>,
    pub leader_is_ai: bool,
}

/// One immutable snapshot for one advancing tick.
pub struct DecisionFrame<'a> {
    pub tick: u64,
    pub own: OwnState,
    pub targets: &'a [TargetView],
    pub events: &'a [FrameEvent],
    pub stations: &'a [StationView],
    pub dispensers: &'a [threat::DispenserStore],
    pub wing: WingView,
    pub route: RouteView,
    pub now: TimeOfDay,
    /// The actor's flight state for the B47 gates.
    pub flight_state: FlightState,
}

/// How a motion intent finishes (B13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// Expires when the quarter-second clock reaches this deadline.
    Deadline(Deadline),
    /// Zero-duration geometric completion on the named axis.
    Axis(motion::CompletionAxis),
}

/// A requested maneuver, already resolved into concrete numbers for a steering
/// adapter. The request it came from is kept so the recovered bounds stay
/// visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionIntent {
    /// Identity of this maneuver, so feedback cannot be mistaken between two.
    pub id: u64,
    pub request: MotionRequest,
    pub heading_deg: f64,
    pub flight_path_pitch_deg: f64,
    pub speed: ScalarSpeed,
    pub bank: Bank,
    pub completion: Completion,
    /// The B15 steering point, when the maneuver pursues one.
    pub steering_point: Option<[f64; 3]>,
    pub mode: CommandMode,
    /// Use the fitted close-formation bank request limit. Movement remains input-only.
    pub formation_flight: bool,
    /// Request afterburner through the physical aircraft switch.
    pub afterburner: bool,
}

/// A sensor/target request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorIntent {
    pub designate: Option<u32>,
    pub clear_designation: bool,
}

/// A weapon release request. Naming the actor, station, target and request
/// identity is what keeps a retry from firing twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponIntent {
    pub request: weapon_service::FireRequest,
}

/// A countermeasure release request (B47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceIntent {
    pub class: SeekerClass,
    pub count: u8,
}

/// What the actor is doing, for a player-readable display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Formation,
    Searching,
    Acquiring,
    Pursuing,
    Attacking,
    Defending,
    Evading,
    Breaking,
    Rejoining,
    ReturningToBase,
    /// On the ground waiting for its turn to depart.
    Waiting,
    Taxiing,
    TakingOff,
    /// Holding at the marshal point while another aircraft lands first.
    HoldingMarshal,
    Landing,
    /// Stopped on the ground after landing.
    Landed,
    OutOfFuel,
    Destroyed,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Formation => "In formation",
            Self::Searching => "Searching",
            Self::Acquiring => "Acquiring",
            Self::Pursuing => "Pursuing",
            Self::Attacking => "Attacking",
            Self::Defending => "Defending",
            Self::Evading => "Evading",
            Self::Breaking => "Breaking",
            Self::Rejoining => "Rejoining",
            Self::ReturningToBase => "Returning to base",
            Self::Waiting => "Waiting to take off",
            Self::Taxiing => "Taxiing",
            Self::TakingOff => "Taking off",
            Self::HoldingMarshal => "Holding at marshal",
            Self::Landing => "Landing",
            Self::Landed => "Landed",
            Self::OutOfFuel => "Out of fuel",
            Self::Destroyed => "Destroyed",
        }
    }
}

/// Everything one tick asks the host to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntentBatch {
    pub motion: Option<MotionIntent>,
    pub gun_aim: Option<super::gunnery::Aim>,
    pub sensor: SensorIntent,
    pub weapons: Vec<WeaponIntent>,
    pub devices: Option<DeviceIntent>,
    pub wing: Vec<WingRequest>,
    pub activity: Option<Activity>,
    /// Fitted fallbacks applied on this tick, in the order they were applied.
    pub fallbacks: Vec<Fallback>,
    /// The reason the script is running under (B47 ranking).
    pub reason: Option<ScriptReason>,
    pub fuel_state: Option<FuelState>,
    /// Launchers whose B47 warning this actor accepted this tick from the
    /// other side while carrying a dispenser: its "SAM launch" or "AAM
    /// launch" radio call (docs/spec/radio-chatter.md). Read-only output.
    pub launch_calls: Vec<u32>,
}

/// A count of every fitted fallback this actor has applied since it was built.
///
/// This exists so a report can state, per actor, which unresolved branches were
/// actually reached in a run rather than which ones exist in the code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FallbackLog {
    counts: [u64; Fallback::ALL.len()],
}

impl FallbackLog {
    pub fn record(&mut self, fallback: Fallback) {
        let index = Fallback::ALL
            .iter()
            .position(|f| *f == fallback)
            .expect("every fallback is in Fallback::ALL");
        self.counts[index] = self.counts[index].saturating_add(1);
    }

    pub fn count(&self, fallback: Fallback) -> u64 {
        Fallback::ALL
            .iter()
            .position(|f| *f == fallback)
            .map(|i| self.counts[i])
            .unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.counts.iter().copied().sum()
    }

    /// Every fallback that has fired at least once, with its count.
    pub fn applied(&self) -> Vec<(Fallback, u64)> {
        Fallback::ALL
            .into_iter()
            .zip(self.counts)
            .filter(|(_, n)| *n > 0)
            .collect()
    }
}

/// The active maneuver the controller is flying.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ActiveManeuver {
    intent: MotionIntent,
    submitted_at: CommandClock,
    formation: bool,
    search: bool,
    ordered: bool,
}

/// One aircraft's persistent decision state.
///
/// Held by the host across ticks and, per the spec's persistence requirement,
/// included in any future replay or save snapshot. A removed actor's state is
/// dropped; a restart builds a fresh controller from the recorded initial
/// configuration, which is why [`Controller::new`] is fully deterministic in
/// its seed.
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    identity: ActorIdentity,
    profile: BehaviorProfile,
    experience: ResolvedExperience,
    random: DecisionRandom,
    service: weapon_service::WeaponService,
    gun_views: Vec<super::gunnery::View>,
    gun_cycles: std::collections::BTreeMap<u8, super::gunnery::Cycle>,
    gun_phase: Option<weapon_service::Phase>,
    damage_recovery: bool,
    /// Opinionated (requested by John, 2026-09-29): the mission is over for
    /// this aircraft and it has no runway to land on, so it flies the B48
    /// return-to-base path home and holds there.
    mission_complete: bool,
    gun_tracking: Option<super::gunnery::View>,
    /// B42: after no store resolved, the tick before which a missile is not
    /// chosen again for the motion (the nominal no-station retry).
    missile_retry_until: Option<u64>,
    /// B13: the tick the running 1-second gun-tracking motion started.
    gun_tracking_since: Option<u64>,
    variation: FormationVariation,
    smooth_variation: [f64; 3],
    variation_tick: Option<u64>,
    formation_configuration: Option<(Formation, i32, i32)>,
    active: Option<ActiveManeuver>,
    target: Option<u32>,
    reason: Option<ScriptReason>,
    /// B46 receiver state, refreshed from the frame each advancing tick.
    recipient: RecipientState,
    /// Quarter-second count at which a new tactical choice becomes due.
    next_choice_quarters: u64,
    last_tick: Option<u64>,
    last_batch: IntentBatch,
    next_motion_id: u64,
    next_request_id: u64,
    fallbacks: FallbackLog,
    pending_warnings: Vec<(u64, ThreatReport)>,
    pursuit: Option<(u32, PursuitOffset)>,
    formation_guidance: super::formation::Guidance,
    formation_traffic: std::sync::Arc<[super::formation::Traffic]>,
    ordered_approach: Option<(u32, f64, f64, bool)>,
    search_contact: Option<SearchContact>,
    search_started_tick: Option<u64>,
    search_orbit_altitude_ft: Option<f64>,
    completed_search: Option<SearchContact>,
    /// Opinionated (John, 2026-09-30): the wing's mission of opportunity
    /// search point, a hostile's last known position, flown like a lost
    /// contact when this aircraft has none of its own
    /// ([`super::opportunity`]).
    wing_search: Option<SearchContact>,
    /// Opinionated (John, 2026-09-30): the wing's next waypoint once its
    /// mission of opportunity search is over ([`super::opportunity`]).
    wing_route: Option<[f64; 3]>,
    defense_motion: Option<DefenseMotion>,
    defense_motion_id: Option<u64>,
    mission_target: Option<Option<u32>>,
    mission_rejoin: Option<[f64; 3]>,
    mission_search_bearing: Option<f64>,
    /// A striker's order for this step (slice ST-T), set by the mission just
    /// before `step` and taken by it.
    strike: Option<super::strike::Order>,
    /// How the adapter is to fly the strike steer this step flew (slice
    /// ST-T2); the mission takes it right after the step.
    strike_handling: super::strike::Handling,
    /// Write-only record of the latest advancing tick. No decision reads it.
    trace: thought::Record<ControllerTrace>,
}

impl Controller {
    /// Cancel offensive motion immediately while leaving missile survival
    /// decisions available. Mission permission determines subsequent targeting.
    pub fn return_to_formation(&mut self) {
        self.target = None;
        self.recipient.target = None;
        self.recipient.target_order = None;
        self.recipient.target_deadline = None;
        self.recipient.active_command = false;
        self.reason = None;
        self.active = None;
        self.pursuit = None;
        self.ordered_approach = None;
        self.search_contact = None;
        self.search_started_tick = None;
        self.search_orbit_altitude_ft = None;
        self.mission_target = Some(None);
        self.mission_rejoin = None;
        self.mission_search_bearing = None;
        self.next_choice_quarters = 0;
    }

    pub fn set_mission_hold_fire(&mut self, hold: bool) {
        if hold {
            self.recipient.target_order = Some(wing::TargetOrder::HoldFire);
        } else if self.recipient.target_order == Some(wing::TargetOrder::HoldFire) {
            self.recipient.target_order = None;
        }
    }

    /// Weapons are held (the hold preset or a hold fire order).
    pub fn weapons_held(&self) -> bool {
        self.recipient.target_order == Some(wing::TargetOrder::HoldFire)
    }

    /// A shared bearing can orient a search but never supplies a target ID,
    /// range, remembered aircraft or weapon solution.
    pub fn set_mission_search_bearing(&mut self, bearing: Option<f64>) {
        if self.mission_search_bearing.is_some() != bearing.is_some() {
            self.active = None;
        }
        self.mission_search_bearing = bearing;
    }

    /// Apply an already-permissioned mission choice. Isolated component
    /// callers retain B41 selection until the mission supplies this override.
    pub fn set_mission_target(&mut self, target: Option<u32>) {
        if self.mission_target != Some(target) {
            if self.pursuit.is_some() {
                self.active = None;
                self.pursuit = None;
            }
            self.next_choice_quarters = 0;
        }
        self.mission_target = Some(target);
    }

    pub fn set_mission_rejoin(&mut self, point: Option<[f64; 3]>) {
        if self.mission_rejoin.is_some() != point.is_some() {
            self.active = None;
            self.pursuit = None;
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
        }
        self.mission_rejoin = point;
    }

    /// Survival maneuvers are driven by perceived missile or incoming-fire evidence. Entering
    /// or leaving defense invalidates a previous tactic, without losing the
    /// independently observed offensive target or the aircraft's memory.
    pub fn set_defense_motion(&mut self, defense: Option<DefenseMotion>) {
        if self.defense_motion.is_some() != defense.is_some() {
            self.active = None;
            self.pursuit = None;
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
            self.defense_motion_id = None;
            self.next_choice_quarters = 0;
        }
        self.defense_motion = defense;
    }

    /// Supply the remembered contact to investigate. This record is frozen:
    /// it is never admitted to target selection or weapon employment.
    pub fn set_search_contact(&mut self, contact: Option<SearchContact>) {
        if self.search_contact != contact {
            if self.active.is_some_and(|active| active.search) {
                self.active = None;
            }
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
            self.completed_search = None;
        }
        self.search_contact = contact;
    }

    /// Supply the wing's mission of opportunity search point, or `None`. It is
    /// searched as a lost contact is, without the Ace's two-minute limit, and
    /// only while this aircraft has no lost contact of its own to
    /// investigate. Like that contact it never becomes a target.
    pub fn set_wing_search(&mut self, contact: Option<SearchContact>) {
        if self.wing_search.map(|c| c.id) != contact.map(|c| c.id)
            && self
                .search_contact
                .is_none_or(|own| self.completed_search == Some(own))
        {
            if self.active.is_some_and(|active| active.search) {
                self.active = None;
            }
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
        }
        self.wing_search = contact;
    }

    /// The wing's mission of opportunity search point, if any.
    pub fn wing_search(&self) -> Option<SearchContact> {
        self.wing_search
    }

    /// Supply the waypoint the wing flies once its mission of opportunity
    /// search is over, or `None`. It is flown only while this aircraft has
    /// no target, no lost contact and no search point.
    pub fn set_wing_route(&mut self, waypoint: Option<[f64; 3]>) {
        self.wing_route = waypoint;
    }

    /// The waypoint of the wing's mission of opportunity, if any.
    pub fn wing_route(&self) -> Option<[f64; 3]> {
        self.wing_route
    }

    /// Supply one immutable mission snapshot before advancing this controller.
    pub fn set_formation_observation(
        &mut self,
        traffic: impl Into<std::sync::Arc<[super::formation::Traffic]>>,
    ) {
        self.formation_traffic = traffic.into();
    }

    /// Diagnostics/extension hook, with no normal-flight display or side effects.
    pub fn formation_trace(&self) -> Option<super::formation::Trace> {
        self.formation_guidance.trace
    }

    /// Build a controller for one actor.
    ///
    /// Only the fighter/strike family is implemented. Any other family is
    /// rejected rather than served fighter behavior, because the spec records
    /// the others as separate contracts (B20). A human-controlled actor is
    /// rejected too: no AI path may drive one, and the human-control bit is
    /// what exempts an aircraft from the experience G adjustment.
    pub fn new(
        identity: ActorIdentity,
        profile: BehaviorProfile,
        experience: ResolvedExperience,
        seed: u64,
    ) -> Result<Self> {
        if identity.human_controlled {
            return Err(AiError::InvalidInput(
                "a human-controlled aircraft is never driven by an AI controller",
            ));
        }
        if profile.family != BehaviorFamily::FighterStrike {
            return Err(AiError::UnspecifiedRule(
                "B20 behavior families other than fighter/strike",
            ));
        }
        Ok(Self {
            gun_views: Vec::new(),
            gun_cycles: Default::default(),
            gun_phase: None,
            damage_recovery: false,
            mission_complete: false,
            gun_tracking: None,
            missile_retry_until: None,
            gun_tracking_since: None,
            identity,
            profile,
            experience,
            random: DecisionRandom::seeded(seed),
            service: weapon_service::WeaponService::new(
                identity.actor,
                TimingProfile::for_aircraft(identity.aircraft),
            ),
            variation: FormationVariation::new(0),
            smooth_variation: [0.; 3],
            variation_tick: None,
            formation_configuration: None,
            active: None,
            target: None,
            reason: None,
            recipient: RecipientState {
                human_controlled: false,
                // B46 accepts states 19 and 20 only; the host never produces
                // a rejected number. See `fitted::MANEUVER_STATE_FREE`.
                maneuver_state: fitted::MANEUVER_STATE_FREE,
                target: None,
                body_heading_deg: 0,
                speed_limits: SpeedLimits {
                    minimum: ScalarSpeed(0.0),
                    maximum: ScalarSpeed(0.0),
                    corner: ScalarSpeed(0.0),
                },
                active_command: false,
                formation: None,
                wing_control: None,
                horizontal_spacing_ft: None,
                vertical_spacing_ft: None,
                target_order: None,
                target_deadline: None,
            },
            next_choice_quarters: 0,
            last_tick: None,
            last_batch: IntentBatch::default(),
            next_motion_id: 1,
            next_request_id: 1,
            fallbacks: FallbackLog::default(),
            pending_warnings: Vec::new(),
            pursuit: None,
            formation_guidance: super::formation::Guidance::default(),
            formation_traffic: std::sync::Arc::default(),
            ordered_approach: None,
            search_contact: None,
            search_started_tick: None,
            search_orbit_altitude_ft: None,
            completed_search: None,
            wing_search: None,
            wing_route: None,
            defense_motion: None,
            defense_motion_id: None,
            mission_target: None,
            mission_rejoin: None,
            mission_search_bearing: None,
            strike: None,
            strike_handling: super::strike::Handling::PLAIN,
            trace: thought::Record::default(),
        })
    }

    pub fn identity(&self) -> &ActorIdentity {
        &self.identity
    }

    /// Mission return to base without a runway (see `AiMission`).
    /// A striker's order for the next step (docs/spec/ai-strike.md): its
    /// steering flies below a B47 defence and above every other motion, and
    /// its release becomes a weapon request. The step takes it.
    pub fn set_strike(&mut self, order: Option<super::strike::Order>) {
        self.strike = order;
    }

    /// How the adapter is to fly this step's motion: a striker's attack
    /// handling when the step flew its steer, else plain. Taken, so it never
    /// outlives the step (slice ST-T2).
    pub fn take_strike_handling(&mut self) -> super::strike::Handling {
        std::mem::take(&mut self.strike_handling)
    }

    /// The controller flies a B47 defence (a missile or gunfire) now.
    pub fn defending(&self) -> bool {
        self.defense_motion.is_some()
    }

    pub fn set_mission_complete(&mut self, complete: bool) {
        self.mission_complete = complete;
    }

    pub fn mission_complete(&self) -> bool {
        self.mission_complete
    }

    /// Lead succession: this aircraft now leads its wing, or no longer does.
    pub fn set_leads(&mut self, leads: bool) {
        self.identity.leads = leads;
    }

    pub fn profile(&self) -> &BehaviorProfile {
        &self.profile
    }

    pub fn experience(&self) -> ResolvedExperience {
        self.experience
    }
    /// The Enemy AI cheat changes skill mid-flight. Every use reads the level
    /// live, so only timers already running keep their old values.
    pub fn set_experience(&mut self, experience: ResolvedExperience) {
        self.experience = experience;
    }

    pub fn target(&self) -> Option<u32> {
        self.target
    }

    /// The aircraft a lead's order told this actor to attack, while that order
    /// stands (the concrete target of its latest target order). The flight
    /// data link's pursuit applies only to this target (slice G3b).
    pub fn ordered_target(&self) -> Option<u32> {
        match self.recipient.target_order {
            Some(wing::TargetOrder::ConcreteTarget(id)) => Some(id.0),
            _ => None,
        }
    }

    pub fn reason(&self) -> Option<ScriptReason> {
        self.reason
    }

    /// Every fitted fallback this actor has applied.
    pub fn fallbacks(&self) -> &FallbackLog {
        &self.fallbacks
    }

    /// What this controller considered on its latest advancing tick. A
    /// write-only record: no decision reads it.
    pub fn trace(&self) -> &ControllerTrace {
        &self.trace.0
    }

    /// The random draws of the latest advancing tick, with what each decided.
    pub fn draws(&self) -> &super::DrawLog {
        self.random.log()
    }

    /// The intents of the latest advancing tick; a repeated tick returns
    /// these again.
    pub fn last_batch(&self) -> &IntentBatch {
        &self.last_batch
    }

    /// The weapon service's current phase (B42).
    pub fn weapon_phase(&self) -> weapon_service::Phase {
        self.gun_phase.unwrap_or(self.service.phase())
    }

    pub fn set_damage_recovery(&mut self, recovery: bool) {
        self.damage_recovery = recovery;
    }

    pub fn set_gun_views(&mut self, views: Vec<super::gunnery::View>) {
        self.gun_views = views;
    }

    /// The weapon service's pending deadline, in quarter-second counts.
    pub fn weapon_deadline(&self) -> Option<u64> {
        if self.gun_phase.is_some() {
            let gun = self.gun_tracking?;
            self.gun_cycles
                .get(&gun.station.0)?
                .deadline(self.last_tick?)
                .map(|tick| tick / 30)
        } else {
            self.service.deadline()
        }
    }

    /// The maneuver being flown, if any.
    pub fn active_maneuver(&self) -> Option<ActiveManeuverView> {
        self.active.map(|active| ActiveManeuverView {
            intent: active.intent,
            submitted_tick: active.submitted_at.tick(),
            formation: active.formation,
            search: active.search,
            ordered: active.ordered,
        })
    }

    /// The frozen contact supplied for investigation, if any.
    pub fn search_contact(&self) -> Option<SearchContact> {
        self.search_contact
    }

    /// Advance one simulation tick and return this tick's intents.
    ///
    /// A tick that does not advance repeats the previous batch without drawing
    /// anything, so a paused host, a re-render or a duplicated call cannot
    /// reroll a tactic. A tick that goes backwards is invalid input.
    pub fn step(&mut self, frame: &DecisionFrame<'_>) -> Result<IntentBatch> {
        let strike = self.strike.take();
        self.strike_handling = super::strike::Handling::PLAIN;
        match self.last_tick {
            Some(last) if frame.tick < last => {
                return Err(AiError::InvalidInput("controller tick went backwards"));
            }
            Some(last) if frame.tick == last => return Ok(self.last_batch.clone()),
            _ => {}
        }
        self.last_tick = Some(frame.tick);
        // Write-only records start afresh on each advancing tick.
        self.trace.0 = ControllerTrace::begin(frame.tick);
        self.random.clear_log();

        let mut batch = IntentBatch::default();
        if !frame.own.alive {
            self.formation_guidance = super::formation::Guidance::default();
            batch.activity = Some(Activity::Destroyed);
            self.last_batch = batch.clone();
            self.trace.0.path = StepPath::Destroyed;
            return Ok(batch);
        }
        self.trace.0.path = StepPath::Decided;

        let clock = CommandClock::at_tick(frame.tick);

        // Keep the B46 receiver state current so an order arriving between
        // ticks is judged against this actor's real heading, limits and state.
        self.recipient.human_controlled = self.identity.human_controlled;
        self.recipient.body_heading_deg = frame.own.heading_deg.round() as i32;
        self.recipient.speed_limits = frame.own.limits;
        self.recipient.active_command = self.active.is_some();
        self.recipient.maneuver_state = if self.target.is_some() {
            fitted::MANEUVER_STATE_ENGAGED
        } else {
            fitted::MANEUVER_STATE_FREE
        };

        // 1. Events set this tick's script reason (B47 ranking) and can
        //    release countermeasures.
        let reason = self.process_events(frame, &mut batch)?;

        // 2. Fuel and recovery (B48) outrank ordinary combat decisions.
        let fuel = self.fuel(frame, &mut batch);
        let recovering = matches!(fuel, Some(FuelState::Bingo) | Some(FuelState::Critical))
            || self.mission_complete;
        if let Some(fuel_trace) = self.trace.0.fuel.as_mut() {
            fuel_trace.recovering = recovering;
        }

        if self
            .ordered_approach
            .is_some_and(|(id, ..)| !frame.targets.iter().any(|t| t.id == id && t.valid))
        {
            self.ordered_approach = None;
            self.active = None;
            self.target = None;
            self.recipient.target = None;
            self.recipient.target_order = Some(wing::TargetOrder::HoldFire);
            self.trace.0.ordered_approach_lost = true;
        }

        // 3. Target selection and geometry (B41, B01 to B03).
        let selected = self.select_target(frame)?;
        self.trace.0.target.chosen = selected;
        batch.sensor.designate = selected;
        if selected.is_none()
            && (self.target.is_some()
                || self.recipient.target_order == Some(wing::TargetOrder::HoldFire))
        {
            batch.sensor.clear_designation = true;
        }
        self.target = selected;
        let view = selected.and_then(|id| frame.targets.iter().find(|t| t.id == id).copied());
        if view.is_some() && self.search_started_tick.is_some() {
            if self.active.is_some_and(|active| active.search) {
                self.active = None;
            }
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
            self.trace.0.target.search_ended = true;
        }
        let geometry = view.map(|t| self.geometry(frame, &t)).transpose()?;
        self.trace.0.geometry = geometry;

        // 4. Weapons, on the service's own clock (B42, B45).
        // A target known only from the flight data link is flown toward and
        // never fired on: the weapons see no target until this actor's own
        // sensors hold it (slice G3b).
        let own_view = view.filter(|t| !t.link_track);
        let has_firing_solution = self.weapons(
            frame,
            own_view,
            geometry.as_ref().filter(|_| own_view.is_some()),
            &mut batch,
        )?;

        // A striker's release (slice ST-T), unless weapons are held.
        if let Some(release) = strike.and_then(|o| o.release)
            && self.recipient.target_order != Some(wing::TargetOrder::HoldFire)
            && !self.damage_recovery
        {
            batch.weapons.push(WeaponIntent {
                request: weapon_service::FireRequest {
                    actor: self.identity.actor,
                    station: release.station,
                    target: weapon_service::TargetId(release.target),
                    request_id: weapon_service::RequestId(self.next_request_id),
                },
            });
            self.next_request_id += 1;
        }

        // 5. Wing requests (B43).
        self.wing_requests(frame, &mut batch);

        // 6. Motion. An active maneuver runs to its completion rule before a
        //    new tactic is chosen (B13), except when a higher reason restarts
        //    the script.
        if let Some(defense) = self.defense_motion {
            let id = *self.defense_motion_id.get_or_insert_with(|| {
                let id = self.next_motion_id;
                self.next_motion_id += 1;
                id
            });
            let (speed_request, speed) = if defense.rotorcraft {
                // A helicopter's break keeps its strike speed (slice HF1).
                let speed = super::helicopter_defense::strike_speed(&frame.own.limits);
                (SpeedRequest::Explicit(speed), speed)
            } else if defense.extend {
                (
                    SpeedRequest::Explicit(frame.own.limits.maximum),
                    frame.own.limits.maximum,
                )
            } else {
                (SpeedRequest::Corner, frame.own.limits.corner)
            };
            let request = MotionRequest::new(
                defense.heading_deg.round() as i32,
                PitchRequest::Explicit(defense.pitch_deg.round() as i32),
                Bank::Unconstrained,
                speed_request,
                Duration::Timed(1),
            );
            batch.motion = Some(MotionIntent {
                id,
                request,
                heading_deg: defense.heading_deg,
                flight_path_pitch_deg: defense.pitch_deg,
                speed,
                bank: Bank::Unconstrained,
                completion: Completion::Deadline(
                    motion::deadline_for(Duration::Timed(1), clock).expect("timed defense"),
                ),
                steering_point: None,
                mode: CommandMode::OtherState,
                formation_flight: false,
                afterburner: false,
            });
            batch.activity = Some(Activity::Defending);
            self.trace.0.motion.branch = if defense.source == DefenseSource::IncomingFire {
                MotionBranch::IncomingFire {
                    heading_deg: defense.heading_deg,
                    pitch_deg: defense.pitch_deg,
                }
            } else {
                MotionBranch::MissileDefense {
                    heading_deg: defense.heading_deg,
                    pitch_deg: defense.pitch_deg,
                }
            };
        } else if self.damage_recovery {
            let request = MotionRequest::new(
                self.home_heading(frame, frame.own.heading_deg.round() as i32),
                PitchRequest::Explicit(0),
                Bank::Unconstrained,
                SpeedRequest::Explicit(ScalarSpeed(
                    (frame.own.limits.minimum.0 * 1.6).min(frame.own.limits.maximum.0),
                )),
                Duration::Timed(1),
            );
            batch.motion = Some(self.resolve(frame, clock, request, None, None, &mut batch)?);
            batch.activity = Some(Activity::ReturningToBase);
            self.trace.0.motion.branch = MotionBranch::DamageRecovery;
        } else if let Some(steer) = strike.and_then(|o| o.steer).filter(|_| !recovering) {
            // A striker's pass (slice ST-T): the strike owns the motion, so
            // whatever maneuver was running is dropped.
            self.active = None;
            self.pursuit = None;
            // The gunship's orbit flies its own bank (slice ST-F); every
            // other pass leaves the bank to the adapter.
            let bank =
                steer.bank_deg.map_or(
                    Bank::Unconstrained,
                    |deg| Bank::Explicit(deg.round() as i32),
                );
            self.strike_handling = steer.handling;
            let request = MotionRequest::new(
                steer.heading_deg.round() as i32,
                PitchRequest::Explicit(steer.pitch_deg.round() as i32),
                bank,
                steer.speed,
                Duration::Timed(1),
            );
            let id = self.next_motion_id;
            self.next_motion_id += 1;
            batch.motion = Some(MotionIntent {
                id,
                request,
                heading_deg: steer.heading_deg,
                flight_path_pitch_deg: steer.pitch_deg.clamp(-90., 90.),
                speed: steer.speed.resolve(frame.own.limits)?,
                bank,
                completion: Completion::Deadline(
                    motion::deadline_for(Duration::Timed(1), clock).expect("timed strike"),
                ),
                steering_point: None,
                mode: CommandMode::OtherState,
                formation_flight: false,
                afterburner: steer.afterburner,
            });
            batch.activity = strike.and_then(|o| o.activity);
            self.trace.0.motion.branch = MotionBranch::Strike {
                heading_deg: steer.heading_deg,
                pitch_deg: steer.pitch_deg,
            };
        } else if let Some(point) = self.mission_rejoin.filter(|_| !recovering) {
            let delta = std::array::from_fn::<_, 3, _>(|i| point[i] - frame.own.position[i]);
            let heading = delta[0].atan2(delta[2]).to_degrees();
            let pitch = delta[1]
                .atan2(delta[0].hypot(delta[2]))
                .to_degrees()
                .clamp(-20., 20.);
            let request = MotionRequest::new(
                heading.round() as i32,
                PitchRequest::Explicit(pitch.round() as i32),
                Bank::Unconstrained,
                SpeedRequest::Corner,
                Duration::Timed(3),
            );
            self.pursuit = None;
            batch.motion = Some(self.resolve(
                frame,
                clock,
                request,
                None,
                None,
                &mut IntentBatch::default(),
            )?);
            batch.activity = Some(Activity::Rejoining);
            self.trace.0.motion.branch = MotionBranch::MissionRejoin { point };
        } else if let Some(bearing) = self
            .mission_search_bearing
            .filter(|_| view.is_none() && !recovering)
        {
            let request = MotionRequest::new(
                bearing.round() as i32,
                PitchRequest::Explicit(0),
                Bank::Unconstrained,
                SpeedRequest::Corner,
                Duration::Timed(1),
            );
            self.pursuit = None;
            let motion = self.resolve(frame, clock, request, None, None, &mut batch)?;
            batch.motion = Some(motion);
            batch.activity = Some(Activity::Searching);
            self.trace.0.motion.branch = MotionBranch::SearchBearing {
                bearing_deg: bearing,
            };
        } else if !recovering
            && !self.active.is_some_and(|a| a.ordered)
            && view.is_some()
            && let Some(gun) = self.gun_tracking
            && let Some(solution) = gun.solution
        {
            let direction = solution.aim.direction;
            let heading = direction[0]
                .atan2(direction[2])
                .to_degrees()
                .rem_euclid(360.);
            let pitch = direction[1]
                .atan2(direction[0].hypot(direction[2]))
                .to_degrees()
                - frame.own.body_pitch_offset_deg;
            let speed = (gun.target_speed + (solution.range_ft - 1500.) / 10.).clamp(
                (frame.own.limits.minimum.0 * 1.25).min(frame.own.limits.maximum.0),
                frame.own.limits.maximum.0,
            );
            let id = *self.defense_motion_id.get_or_insert_with(|| {
                let id = self.next_motion_id;
                self.next_motion_id += 1;
                id
            });
            let request = MotionRequest::new(
                heading.round() as i32,
                PitchRequest::Explicit(pitch.round() as i32),
                Bank::Unconstrained,
                SpeedRequest::Explicit(ScalarSpeed(speed)),
                Duration::Timed(1),
            );
            batch.motion = Some(MotionIntent {
                id,
                request,
                heading_deg: heading,
                flight_path_pitch_deg: pitch,
                speed: ScalarSpeed(speed),
                bank: Bank::Unconstrained,
                completion: Completion::Deadline(
                    motion::deadline_for(Duration::Timed(1), clock).expect("timed gun tracking"),
                ),
                steering_point: None,
                mode: CommandMode::OtherState,
                formation_flight: false,
                afterburner: false,
            });
            batch.gun_aim = Some(solution.aim);
            batch.activity = Some(if self.gun_phase == Some(weapon_service::Phase::Fire) {
                Activity::Attacking
            } else if solution.aligned {
                Activity::Pursuing
            } else {
                Activity::Acquiring
            });
            self.trace.0.motion.branch = MotionBranch::GunTracking {
                heading_deg: heading,
                pitch_deg: pitch,
            };
        } else {
            self.motion(
                frame,
                clock,
                reason,
                recovering,
                view,
                geometry.as_ref(),
                &mut batch,
            )?;
        }

        if batch.activity.is_none() {
            batch.activity = Some(self.activity(frame, recovering, view.is_some()));
        }
        if view.is_some() && !has_firing_solution && batch.activity == Some(Activity::Pursuing) {
            batch.activity = Some(Activity::Acquiring);
        }
        // B48: an aircraft whose internal fuel has reached zero is lost. That
        // outranks whatever tactical activity the rest of the tick produced.
        if fuel == Some(FuelState::OutOfFuel) {
            batch.activity = Some(Activity::OutOfFuel);
        }
        if batch.activity != Some(Activity::Formation) {
            self.formation_guidance = super::formation::Guidance::default();
        }
        batch.reason = self.reason;
        self.last_batch = batch.clone();
        self.trace.0.pursuit = self.pursuit;
        Ok(batch)
    }

    /// Refresh instantaneous physical state before between-tick commands.
    pub fn prepare_order(&mut self, heading_deg: f64, limits: SpeedLimits) {
        self.recipient.body_heading_deg = heading_deg.round() as i32;
        self.recipient.speed_limits = limits;
        self.recipient.human_controlled = self.identity.human_controlled;
        self.recipient.active_command = self.active.is_some();
        self.recipient.target = self.target.map(WingTargetId);
    }

    /// The B48 fuel level judged on the latest tick, for radio fuel calls.
    /// Read-only: nothing here feeds a decision.
    pub fn fuel_state(&self) -> Option<FuelState> {
        if self.last_batch.activity == Some(Activity::OutOfFuel) {
            Some(FuelState::OutOfFuel)
        } else {
            self.last_batch.fuel_state
        }
    }

    /// The formation the last formation order selected, if any.
    pub fn ordered_formation(&self) -> Option<Formation> {
        self.recipient.formation
    }

    pub fn wing_settings(&self) -> (Option<WingControl>, Option<i32>, Option<i32>) {
        (
            self.recipient.wing_control,
            self.recipient.horizontal_spacing_ft,
            self.recipient.vertical_spacing_ft,
        )
    }

    /// Receive one wing command (B46).
    ///
    /// The four outcomes stay distinct: a setting applied, a rejection, a
    /// setting applied without motion, and motion installed. The original
    /// handler's single Boolean is deliberately not reproduced. Motion
    /// installed here replaces the active maneuver outright, which is what
    /// B13 records for a wing command.
    pub fn receive_order(&mut self, request: WingRequest, tick: u64) -> Result<ReceiverOutcome> {
        self.recipient.target = self.target.map(WingTargetId);
        self.recipient.active_command = self.active.is_some();
        self.recipient.human_controlled = self.identity.human_controlled;
        let outcome = wing::receive(request, &mut self.recipient, tick)?;
        if matches!(
            outcome,
            ReceiverOutcome::Applied(_) | ReceiverOutcome::MotionInstalled(_)
        ) {
            self.pursuit = None;
            if !matches!(request, WingRequest::Spacing { .. }) {
                self.ordered_approach = None;
            }
        }
        match &outcome {
            ReceiverOutcome::MotionInstalled(summary) => {
                self.install_ordered_motion(summary, tick);
                self.search_started_tick = None;
                self.search_orbit_altitude_ft = None;
                if matches!(request, WingRequest::Approach { .. })
                    && let Some(target) = self.recipient.target
                {
                    self.ordered_approach = Some((
                        target.0,
                        f64::from(summary.heading_deg),
                        f64::from(summary.pitch_deg),
                        true,
                    ));
                }
            }
            ReceiverOutcome::Applied(AppliedSetting::FormationSelection {
                cleared_active_command,
            }) => {
                if *cleared_active_command {
                    self.active = None;
                }
            }
            ReceiverOutcome::Applied(AppliedSetting::WingControl) => {
                // B46: applying wing control resets the active command.
                self.active = None;
            }
            ReceiverOutcome::Applied(AppliedSetting::TargetOrder { .. }) => {
                self.target = match self.recipient.target_order {
                    Some(wing::TargetOrder::ConcreteTarget(id)) => Some(id.0),
                    _ => None,
                };
                self.active = None;
                self.next_choice_quarters = 0;
            }
            ReceiverOutcome::Applied(AppliedSetting::Spacing { .. })
            | ReceiverOutcome::Rejected(_)
            | ReceiverOutcome::AppliedNoMotion => {}
        }
        Ok(outcome)
    }

    /// Fitted moving target approach point; B46 completion is 2000 ft.
    pub fn track_ordered_approach(&mut self, target: u32, heading_offset: f64, pitch_offset: f64) {
        self.ordered_approach = Some((target, heading_offset, pitch_offset, false));
    }

    /// Turn an ordered maneuver into this actor's active motion.
    fn install_ordered_motion(&mut self, summary: &MotionSummary, tick: u64) {
        let clock = CommandClock::at_tick(tick);
        let duration = match summary.duration {
            MotionDuration::NominalSeconds(seconds) => Duration::from_operand(
                i32::try_from(seconds).unwrap_or(motion::MAX_DURATION_SECONDS),
            ),
            // B46 leaves an approach's duration unstated; a geometric
            // completion keeps it running until its axis arrives.
            MotionDuration::Unspecified => Duration::Geometric,
        };
        let request = MotionRequest::new(
            summary.heading_deg,
            PitchRequest::Explicit(summary.pitch_deg),
            Bank::Unconstrained,
            SpeedRequest::Explicit(summary.speed),
            duration,
        );
        let completion = match motion::deadline_for(duration, clock) {
            Some(deadline) => Completion::Deadline(deadline),
            None => Completion::Axis(motion::CompletionAxis::Heading),
        };
        let id = self.next_motion_id;
        self.next_motion_id += 1;
        let intent = MotionIntent {
            formation_flight: false,
            afterburner: false,
            id,
            request,
            heading_deg: f64::from(request.heading_deg),
            flight_path_pitch_deg: f64::from(summary.pitch_deg).clamp(-90.0, 90.0),
            speed: summary.speed,
            bank: Bank::Unconstrained,
            completion,
            steering_point: None,
            mode: CommandMode::OtherState,
        };
        self.active = Some(ActiveManeuver {
            intent,
            submitted_at: clock,
            formation: false,
            search: false,
            ordered: true,
        });
        // A wing command cancels whatever tactic was pending.
        self.next_choice_quarters =
            clock.quarter_count() + fitted::tactical_cadence_quarters(self.experience.level);
    }

    /// Rank this tick's events into a script reason and service B47 warnings.
    fn process_events(
        &mut self,
        frame: &DecisionFrame<'_>,
        batch: &mut IntentBatch,
    ) -> Result<Option<ScriptReason>> {
        let mut highest: Option<ScriptReason> = None;
        let mut due_missiles = Vec::new();
        let mut events = frame.events.to_vec();
        self.pending_warnings.retain(|(due, report)| {
            if frame.tick >= *due {
                due_missiles.push(report.missile_id);
                events.push(FrameEvent::ThreatReported(*report));
                false
            } else {
                true
            }
        });
        for event in &events {
            match event {
                FrameEvent::Hit => {
                    highest = Some(highest.map_or(ScriptReason::Hit, |r| r.max(ScriptReason::Hit)));
                    self.trace.0.events.hit = true;
                }
                FrameEvent::ThreatReported(report) => {
                    if let Some(reason) = self.warning(
                        frame,
                        report,
                        due_missiles.contains(&report.missile_id),
                        batch,
                    )? {
                        highest = Some(highest.map_or(reason, |r| r.max(reason)));
                    }
                }
                FrameEvent::TargetUnavailable(id) | FrameEvent::ActorRemoved(id) => {
                    if self.target == Some(*id) {
                        self.target = None;
                        // B46: a wingman whose target is lost returns to
                        // formation, a leader resumes its waypoint.
                        let role = if self.identity.is_leader() {
                            wing::WingRole::Leader
                        } else {
                            wing::WingRole::Wingman
                        };
                        let _ = wing::on_target_lost(role);
                        self.trace.0.events.lost_targets.push(*id);
                    }
                }
            }
        }

        self.trace.0.events.highest = highest;
        let Some(reason) = highest else {
            return Ok(None);
        };
        Ok(self.accept_reason(reason))
    }

    fn accept_reason(&mut self, reason: ScriptReason) -> Option<ScriptReason> {
        let start = threat::on_new_reason(self.reason, reason);
        self.trace.0.events.previous = self.reason;
        self.trace.0.events.start = Some(start);
        match start {
            ScriptStart::Restart => {
                self.reason = Some(reason);
                self.active = None;
                self.pursuit = None;
                self.next_choice_quarters = 0;
                Some(reason)
            }
            ScriptStart::Resume => None,
        }
    }

    /// One launch warning (B47).
    fn warning(
        &mut self,
        frame: &DecisionFrame<'_>,
        report: &ThreatReport,
        scheduled: bool,
        batch: &mut IntentBatch,
    ) -> Result<Option<ScriptReason>> {
        // The delay is measured from launch; the host delivers the report, and
        // the controller only acts once the delay has elapsed.
        let attack_state = if self.target == Some(report.launcher_id) {
            AttackState::AttackState {
                engaging_launcher: true,
            }
        } else {
            AttackState::OrdinaryFlight
        };
        let target = WarningTarget::Ai {
            experience: self.experience.level,
            state: attack_state,
        };
        let delay = threat::warning_delay(&target, report.distance_at_launch_ft)?;
        let mut trace = WarningTrace {
            report: *report,
            from_queue: scheduled,
            attack_state,
            delay,
            due_tick: None,
            step: WarningStep::NotWarned,
        };
        let WarningDelay::Quarters(quarters) = delay else {
            self.trace.0.events.warnings.push(trace);
            return Ok(None);
        };
        let due = report.launch_tick + quarters as u64 * super::QUARTER_SECOND_TICKS;
        trace.due_tick = Some(due);
        if !scheduled && frame.tick < due {
            let already_queued = self.pending_warnings.iter().any(|(_, r)| r == report);
            if !already_queued {
                self.pending_warnings.push((due, *report));
            }
            trace.step = WarningStep::Queued { already_queued };
            self.trace.0.events.warnings.push(trace);
            return Ok(None);
        }

        let inputs = WarningInputs {
            seeker: report.seeker,
            experience: self.experience.level,
            flight_state: frame.flight_state,
            has_dispenser_station: !frame.dispensers.is_empty(),
            launcher: threat::Launcher {
                same_side: report.launcher_same_side,
                is_current_target: self.target == Some(report.launcher_id),
            },
            hold_until: None,
            now: frame.now,
        };
        let outcome = threat::receive_warning(&inputs, &mut self.random);
        let launch_call = matches!(
            outcome.reaction,
            WarningReaction::NoManeuver | WarningReaction::Maneuver { .. }
        ) && !batch.launch_calls.contains(&report.launcher_id);
        if launch_call {
            batch.launch_calls.push(report.launcher_id);
        }
        let mut weapons_postponed_to = None;
        if let Some(release) = outcome.devices {
            batch.devices = Some(DeviceIntent {
                class: release.class,
                count: release.count,
            });
            // The successful reaction postpones the weapon service by 2 s.
            weapons_postponed_to = self.service.postpone_for_device_reaction();
        }
        trace.step = WarningStep::Received {
            outcome,
            launch_call,
            weapons_postponed_to,
        };
        self.trace.0.events.warnings.push(trace);
        match outcome.reaction {
            WarningReaction::Maneuver { reason, .. } => Ok(Some(reason)),
            WarningReaction::Suppressed(_)
            | WarningReaction::Ignored
            | WarningReaction::NoReaction
            | WarningReaction::RadioOnly
            | WarningReaction::NoManeuver => Ok(None),
        }
    }

    /// B48 fuel state, and the bingo decision.
    fn fuel(&mut self, frame: &DecisionFrame<'_>, batch: &mut IntentBatch) -> Option<FuelState> {
        let mut trace = FuelTrace {
            internal_fuel_lbs: frame.own.internal_fuel_lbs,
            endurance_s: frame.own.fuel_endurance_s,
            time_home_s: frame.own.time_home_s,
            state: None,
            recovering: false,
        };
        if route::internal_fuel_event(frame.own.internal_fuel_lbs).is_some() {
            batch.activity = Some(Activity::OutOfFuel);
            trace.state = Some(FuelState::OutOfFuel);
            self.trace.0.fuel = Some(trace);
            return Some(FuelState::OutOfFuel);
        }
        let state = if frame.own.rotorcraft {
            route::rotorcraft_fuel_state(frame.own.fuel_endurance_s, frame.own.time_home_s)
        } else {
            route::fuel_state(frame.own.fuel_endurance_s, frame.own.time_home_s)
        }
        .ok();
        trace.state = state;
        self.trace.0.fuel = Some(trace);
        let state = state?;
        batch.fuel_state = Some(state);
        Some(state)
    }

    /// B41 retention, eligibility and ranking over the permitted targets.
    fn select_target(&mut self, frame: &DecisionFrame<'_>) -> Result<Option<u32>> {
        self.trace.0.target.previous = self.target;
        if self.recipient.target_order == Some(wing::TargetOrder::HoldFire) {
            self.trace.0.target.path = TargetPath::HoldFire;
            return Ok(None);
        }
        if let Some(selected) = self.mission_target {
            let chosen = selected.filter(|id| {
                frame.targets.iter().any(|t| {
                    t.id == *id
                        && t.id != self.identity.actor.0
                        && t.side != self.identity.side
                        && t.valid
                        && t.type_allowed
                        && t.seeker_eligible
                })
            });
            self.trace.0.target.path = TargetPath::Mission {
                requested: selected,
                refused: selected
                    .filter(|_| chosen.is_none())
                    .map(|id| mission_target_refusal(frame, &self.identity, id)),
            };
            return Ok(chosen);
        }
        let candidates: Vec<CandidateTarget> = frame
            .targets
            .iter()
            .map(|t| CandidateTarget {
                id: ObjectId(t.id),
                side: t.side,
                valid: t.valid,
                type_allowed: t.type_allowed,
                is_aircraft: t.is_aircraft,
                seeker_eligible: t.seeker_eligible,
                spatial_distance_feet: distance(frame.own.position, t.position),
                wing_attackers: t.wing_attackers,
            })
            .collect();

        // Retention shortcut first: an existing valid target inside 20000 ft
        // stays selected. It is not permission to shoot through terrain.
        if let Some(current) = self
            .target
            .and_then(|id| candidates.iter().find(|c| c.id.0 == id))
            && targeting::retain_current_target(Some(current), false)
        {
            self.trace.0.target.path = TargetPath::Retained {
                distance_ft: current.spatial_distance_feet,
            };
            return Ok(self.target);
        }

        let selector = Selector {
            id: ObjectId(self.identity.actor.0),
            side: self.identity.side,
            kind: SelectorKind::Aircraft,
            assignment_allowance: wing::AttackerCap::Two.value(),
        };
        let chosen = targeting::select_target(&selector, SelectionRoute::Ordinary, &candidates)?;
        self.trace.0.target.path = TargetPath::Ranked {
            candidates: candidates.len(),
            score_ft: chosen.map(|candidate| {
                targeting::candidate_score(candidate, selector.assignment_allowance)
            }),
        };
        Ok(chosen.map(|c| c.id.0))
    }

    /// B01 to B03 geometry against the selected target.
    fn geometry(
        &self,
        frame: &DecisionFrame<'_>,
        target: &TargetView,
    ) -> Result<geometry::TargetGeometry> {
        geometry::target_geometry(
            &OwnPose {
                position: frame.own.position,
                heading_deg: frame.own.heading_deg,
                pitch_deg: frame.own.flight_path_pitch_deg,
            },
            &KnownTarget {
                pose: Some(TargetPose {
                    position: target.position,
                    heading_deg: target.heading_deg,
                    pitch_deg: target.pitch_deg,
                }),
            },
        )
    }

    /// B42 weapon cadence and B45 store choice.
    fn weapons(
        &mut self,
        frame: &DecisionFrame<'_>,
        target: Option<TargetView>,
        geometry: Option<&geometry::TargetGeometry>,
        batch: &mut IntentBatch,
    ) -> Result<bool> {
        let class = match target {
            Some(t) if t.is_aircraft => weapon_service::TargetClass::Air,
            Some(_) => weapon_service::TargetClass::Surface,
            None => weapon_service::TargetClass::Air,
        };
        let mut stations = Vec::new();
        let station = self.choose_station(frame, class, target, batch, &mut stations);
        let chosen_gun = station
            .and_then(|i| {
                self.gun_views
                    .iter()
                    .find(|g| g.station == frame.stations[i].station)
            })
            .copied();
        // B42: no suitable station waits the nominal 2 s retry. The motion
        // honours it too: once no store resolved, gun tracking keeps the
        // aircraft until the retry ends, rather than handing it back to the
        // missile tactic on the next tick the missile's zone test passes.
        // Without it a fighter at a zone edge swapped maneuvers every few
        // ticks, its own stick input moving the zone test each time
        // (overnight bug battery, 2026-09-29).
        if station.is_none() && target.is_some() {
            self.missile_retry_until =
                Some(frame.tick + u64::from(weapon_service::NO_STATION_RETRY.quarter_count()) * 30);
        }
        let retrying = self
            .missile_retry_until
            .is_some_and(|until| frame.tick < until);
        // B13: a timed motion runs to its deadline unless an event replaces
        // it. Gun lead tracking is a 1-second motion, so once started it is
        // not handed back to the missile tactic before that second is up,
        // even if the store choice now prefers the missile (a fighter whose
        // best store alternated between gun and missile otherwise swapped
        // maneuvers every few ticks; overnight bug battery, 2026-09-29).
        let tracking_held = self
            .gun_tracking_since
            .is_some_and(|since| frame.tick < since + GUN_TRACKING_MOTION_TICKS);
        self.gun_tracking =
            (station.is_none() || chosen_gun.is_some() || retrying || tracking_held)
                .then(|| {
                    self.gun_views
                        .iter()
                        .find(|g| {
                            g.target == target.map(|t| t.id)
                                && g.target.is_some()
                                && g.solution.is_some()
                                && frame.stations.iter().any(|s| {
                                    s.station == g.station
                                        && target.is_some_and(|t| {
                                            matches!(
                                                s.verdict(&frame.own, &t, class),
                                                StationVerdict::Usable { .. }
                                                    | StationVerdict::OutsideEnvelope
                                            )
                                        })
                                        && !s.inhibited
                                        && !matches!(s.rounds, weapon_service::Rounds::Finite(0))
                                        && g.solution.is_some_and(|p| {
                                            p.range_ft >= s.minimum_range_ft
                                                && s.maximum_range_ft
                                                    .is_none_or(|m| p.range_ft <= m)
                                        })
                                })
                        })
                        .copied()
                })
                .flatten();
        if self.recipient.target_order == Some(wing::TargetOrder::HoldFire) {
            self.gun_tracking = None;
        }
        self.gun_tracking_since = match (self.gun_tracking, self.gun_tracking_since) {
            (None, _) => None,
            (Some(_), Some(since)) if frame.tick < since + GUN_TRACKING_MOTION_TICKS => Some(since),
            (Some(_), _) => Some(frame.tick),
        };
        for (id, cycle) in &mut self.gun_cycles {
            if chosen_gun.is_none_or(|g| g.station.0 != *id) {
                cycle.advance(frame.tick, None, false, 1, 1);
            }
        }
        self.gun_phase = None;
        self.trace.0.gun = self.gun_tracking.map(|view| {
            let cycle = self.gun_cycles.entry(view.station.0).or_default();
            super::gunnery::Trace {
                view,
                phase: cycle.phase(frame.tick),
                deadline: cycle.deadline(frame.tick),
                requested_round: false,
            }
        });
        if let Some(gun) = chosen_gun {
            let ready = !(self.recipient.target_order == Some(wing::TargetOrder::HoldFire))
                && gun.target == target.map(|t| t.id)
                && gun.solution.is_some_and(|s| s.aligned)
                && target.is_some_and(|t| !t.terrain_blocked);
            let cycle = self.gun_cycles.entry(gun.station.0).or_default();
            let fired = cycle.advance(frame.tick, gun.target, ready, gun.rounds, gun.period_ticks);
            let phase = cycle.phase(frame.tick);
            let deadline = cycle.deadline(frame.tick);
            self.gun_phase = Some(phase);
            let inputs = ServiceInputs {
                target: target.map(|t| WeaponTargetId(t.id)),
                station: Some(gun.station),
                unready: false,
                lock: if ready {
                    LockStatus::Locked {
                        tracking_delay: Delay::quarters(0),
                    }
                } else {
                    LockStatus::Failed
                },
                path_blocked: target.is_some_and(|t| t.terrain_blocked),
                pacing: frame.stations[station.unwrap()].pacing,
            };
            let outcome = if fired {
                let request = weapon_service::FireRequest {
                    actor: self.identity.actor,
                    station: gun.station,
                    target: WeaponTargetId(gun.target.unwrap()),
                    request_id: weapon_service::RequestId(self.next_request_id),
                };
                self.next_request_id += 1;
                batch.weapons.push(WeaponIntent { request });
                batch.activity = Some(Activity::Attacking);
                ServiceOutcome::Fire(request)
            } else {
                ServiceOutcome::Waiting(phase)
            };
            self.trace.0.gun = Some(super::gunnery::Trace {
                view: gun,
                phase,
                deadline,
                requested_round: fired,
            });
            self.trace.0.weapons = Some(WeaponTrace {
                class,
                stations,
                chosen: Some(gun.station),
                lock: LockTrace {
                    locked: ready,
                    has_target: target.is_some(),
                    has_station: true,
                    target_ahead: geometry.and_then(|g| g.angles).map(|a| a.ahead),
                    terrain_blocked: inputs.path_blocked,
                },
                inputs,
                phase_before: phase,
                outcome: Some(outcome),
                burst_pacing_restart: false,
                phase_after: phase,
                deadline: deadline.map(|tick| tick / 30),
            });
            return Ok(ready);
        }

        let angles = geometry.and_then(|g| g.angles);
        let locked = target.is_some()
            && station.is_some()
            && angles.is_some_and(|a| a.ahead)
            && target.is_some_and(|t| !t.terrain_blocked);
        let lock = LockTrace {
            locked,
            has_target: target.is_some(),
            has_station: station.is_some(),
            target_ahead: angles.map(|a| a.ahead),
            terrain_blocked: target.is_some_and(|t| t.terrain_blocked),
        };

        let inputs = ServiceInputs {
            target: target.map(|t| WeaponTargetId(t.id)),
            station: station.map(|s| frame.stations[s].station),
            unready: station
                .is_some_and(|i| frame.stations[i].requires_radar && !frame.own.radar_emitting),
            lock: if locked {
                LockStatus::Locked {
                    tracking_delay: station
                        .map(|s| frame.stations[s].tracking_delay)
                        .unwrap_or(Delay::seconds(0)),
                }
            } else {
                LockStatus::Failed
            },
            path_blocked: target.is_some_and(|t| t.terrain_blocked),
            pacing: station
                .map(|s| frame.stations[s].pacing)
                .unwrap_or(ProjectilePacing {
                    burst_count: 1,
                    burst_interval: Delay::seconds(0),
                    reload: Delay::seconds(1),
                    startup: Delay::seconds(0),
                }),
        };

        let phase_before = self.service.phase();
        let outcome = match self.service.advance(frame.tick, &inputs, &mut self.random) {
            Ok(outcome) => outcome,
            Err(AiError::UnspecifiedRule(_)) => {
                // B42 leaves burst and reload pacing after a launch open. The
                // fitted rule restarts the service from search with this
                // aircraft's own search delay, which is the only pacing the
                // spec does give for this aircraft.
                self.note(Fallback::BurstPacing, batch);
                self.service = weapon_service::WeaponService::new(
                    self.identity.actor,
                    TimingProfile::for_aircraft(self.identity.aircraft),
                );
                self.trace.0.weapons = Some(WeaponTrace {
                    class,
                    stations,
                    chosen: station.map(|s| frame.stations[s].station),
                    lock,
                    inputs,
                    phase_before,
                    outcome: None,
                    burst_pacing_restart: true,
                    phase_after: self.service.phase(),
                    deadline: self.service.deadline(),
                });
                return Ok(locked);
            }
            Err(other) => return Err(other),
        };

        let mut recorded = outcome;
        if let ServiceOutcome::Fire(mut request) = outcome {
            request.request_id = weapon_service::RequestId(self.next_request_id);
            self.next_request_id += 1;
            batch.weapons.push(WeaponIntent { request });
            batch.activity = Some(Activity::Attacking);
            recorded = ServiceOutcome::Fire(request);
        }
        self.trace.0.weapons = Some(WeaponTrace {
            class,
            stations,
            chosen: station.map(|s| frame.stations[s].station),
            lock,
            inputs,
            phase_before,
            outcome: Some(recorded),
            burst_pacing_restart: false,
            phase_after: self.service.phase(),
            deadline: self.service.deadline(),
        });
        Ok(locked)
    }

    /// B45 store choice by score, with the fitted hit-chance term.
    /// `trace` receives a write-only record of every store's verdict.
    fn choose_station(
        &mut self,
        frame: &DecisionFrame<'_>,
        class: weapon_service::TargetClass,
        target: Option<TargetView>,
        batch: &mut IntentBatch,
        trace: &mut Vec<StationTrace>,
    ) -> Option<usize> {
        let target = target?;
        let range_ft = distance(frame.own.position, target.position);
        let mut verdicts = Vec::with_capacity(frame.stations.len());
        let usable: Vec<(usize, StoreCandidate)> = frame
            .stations
            .iter()
            .enumerate()
            .filter_map(|(index, s)| {
                let verdict = s.verdict(&frame.own, &target, class);
                verdicts.push(verdict);
                let StationVerdict::Usable { error_deg: error } = verdict else {
                    return None;
                };
                Some((
                    index,
                    StoreCandidate {
                        station: s.station,
                        guided: s.guided,
                        employment_fit: Some(error),
                        hit_chance: fitted::hit_chance(error, s.employment_limit_deg),
                        range_feet: range_ft,
                        damage_vs_category: s.damage_vs_category,
                    },
                ))
            })
            .take(weapon_service::STORE_CANDIDATE_LIMIT)
            .collect();
        trace.extend(frame.stations.iter().enumerate().map(|(index, s)| {
            // Stores after the tenth usable one were never examined.
            let verdict = verdicts.get(index).copied();
            let candidate = usable
                .iter()
                .find(|(usable_index, _)| *usable_index == index)
                .map(|(_, candidate)| candidate);
            StationTrace {
                index,
                station: s.station,
                verdict,
                employment: matches!(verdict, Some(StationVerdict::OutsideEnvelope))
                    .then(|| s.employment_check(&frame.own, &target)),
                hit_chance: candidate.map(|c| c.hit_chance),
                score: candidate.map(weapon_service::store_score),
            }
        }));
        if usable.is_empty() {
            return None;
        }
        // The hit-chance term above is fitted; record it once per selection.
        self.note(Fallback::HitChance, batch);
        let candidates: Vec<StoreCandidate> = usable.iter().map(|(_, c)| *c).collect();
        let chosen = weapon_service::select_store(&candidates).ok().flatten()?;
        Some(usable[chosen].0)
    }

    /// B43 wing requests from a leader.
    fn wing_requests(&mut self, frame: &DecisionFrame<'_>, batch: &mut IntentBatch) {
        if !self.identity.is_leader() || self.target.is_none() {
            return;
        }
        // Air-to-air entry raises horizontal spacing to 5000 ft if smaller.
        let wanted = tactics::air_to_air_wing_spacing(f64::from(
            self.recipient
                .horizontal_spacing_ft
                .unwrap_or(frame.wing.horizontal_spacing_ft),
        ));
        if wanted
            > f64::from(
                self.recipient
                    .horizontal_spacing_ft
                    .unwrap_or(frame.wing.horizontal_spacing_ft),
            )
        {
            let sender = SenderState {
                is_leader: true,
                target: self.target.map(WingTargetId),
                wingman_targets: vec![None; frame.wing.wingmen_in_formation as usize],
            };
            if let SendOutcome::Sent(requests) =
                wing::set_spacing(&sender, wing::SpacingAxis::Horizontal, wanted as i64)
            {
                batch.wing.extend(requests);
            }
        }
    }

    /// Choose and resolve this tick's maneuver.
    #[allow(clippy::too_many_arguments)]
    fn motion(
        &mut self,
        frame: &DecisionFrame<'_>,
        clock: CommandClock,
        reason: Option<ScriptReason>,
        recovering: bool,
        target: Option<TargetView>,
        geometry: Option<&geometry::TargetGeometry>,
        batch: &mut IntentBatch,
    ) -> Result<()> {
        self.trace.0.motion.quarter = Some(clock.quarter_count());
        self.trace.0.motion.choice_due_at = Some(self.next_choice_quarters);
        if (reason.is_some()
            || recovering
            || self.recipient.target_order == Some(wing::TargetOrder::HoldFire))
            && self.search_started_tick.is_some()
        {
            if self.active.is_some_and(|active| active.search) {
                self.active = None;
            }
            self.search_started_tick = None;
            self.search_orbit_altitude_ft = None;
            self.trace.0.motion.search_cancelled = true;
        }
        if let Some((id, mut heading_offset, mut pitch_offset, absolute)) = self.ordered_approach {
            let observed = frame.targets.iter().find(|t| t.id == id && t.valid);
            if reason.is_none()
                && !recovering
                && let Some(t) = observed
                && !wing::approach_complete(distance(frame.own.position, t.position))
                && let Some(mut active) = self.active
            {
                let dx = t.position[0] - frame.own.position[0];
                let dy = t.position[1] - frame.own.position[1];
                let dz = t.position[2] - frame.own.position[2];
                if absolute {
                    heading_offset = angle_difference(dx.atan2(dz).to_degrees(), heading_offset);
                    pitch_offset -= dy.atan2(dx.hypot(dz)).to_degrees();
                    self.ordered_approach = Some((id, heading_offset, pitch_offset, false));
                }
                // Offset tapers near the point so a permanent 45-degree
                // bearing offset does not turn into an orbit (fitted).
                let scale =
                    ((distance(frame.own.position, t.position) - 2000.) / 8000.).clamp(0., 1.);
                active.intent.heading_deg = dx.atan2(dz).to_degrees() + heading_offset * scale;
                active.intent.flight_path_pitch_deg =
                    (dy.atan2(dx.hypot(dz)).to_degrees() + pitch_offset * scale).clamp(-90., 90.);
                self.active = Some(active);
                batch.motion = Some(active.intent);
                batch.activity = Some(if t.side == self.identity.side {
                    Activity::Rejoining
                } else {
                    Activity::Pursuing
                });
                self.trace.0.motion.branch = MotionBranch::OrderedApproach {
                    target: id,
                    distance_ft: distance(frame.own.position, t.position),
                    offset_scale: scale,
                };
                return Ok(());
            }
            self.ordered_approach = None;
            self.active = None;
            self.trace.0.motion.ordered_approach_ended = true;
            if observed.is_none() {
                self.target = None;
                self.recipient.target_order = Some(wing::TargetOrder::HoldFire);
            }
        }
        if let Some(active) = self.active.filter(|active| active.ordered) {
            let finished = match active.intent.completion {
                Completion::Deadline(deadline) => motion::is_expired(deadline, clock),
                Completion::Axis(_) => self.axis_complete(frame, &active.intent),
            };
            if !finished && reason.is_none() && !recovering {
                batch.motion = Some(active.intent);
                self.trace.0.motion.branch = MotionBranch::OrderedMotion;
                return Ok(());
            }
        }
        if reason.is_none()
            && !recovering
            && target.is_none()
            && self.recipient.target_order != Some(wing::TargetOrder::HoldFire)
            && self.search_motion(frame, clock, batch)?
        {
            return Ok(());
        }
        if reason.is_none()
            && !recovering
            && target.is_none()
            && self.recipient.target_order != Some(wing::TargetOrder::HoldFire)
            && self.route_motion(frame, clock, batch)?
        {
            return Ok(());
        }
        // An active maneuver keeps flying until its completion rule fires.
        if let Some(active) = self.active {
            let finished = match active.intent.completion {
                Completion::Deadline(deadline) => motion::is_expired(deadline, clock),
                Completion::Axis(_) => self.axis_complete(frame, &active.intent),
            };
            self.trace.0.motion.active_finished = Some(finished);
            if finished && reason.is_none() {
                self.trace.0.motion.reason_cleared = self.reason.is_some();
                self.reason = None;
            }
            if active.formation
                && reason.is_none()
                && !recovering
                && target.is_none()
                && self.formation_motion(frame, clock, batch)?
            {
                return Ok(());
            }
            if !finished && reason.is_none() && !active.formation {
                let mut intent = active.intent;
                self.update_pursuit(frame, target, &mut intent);
                batch.motion = Some(intent);
                self.trace.0.motion.branch = MotionBranch::ActiveContinues;
                return Ok(());
            }
        }
        if reason.is_none()
            && !recovering
            && target.is_none()
            && self.formation_motion(frame, clock, batch)?
        {
            return Ok(());
        }
        // Not yet due for a new choice, and nothing is running: keep flying.
        let quarters = clock.quarter_count();
        if self.active.is_some() && quarters < self.next_choice_quarters && reason.is_none() {
            batch.motion = self.active.map(|a| a.intent);
            self.trace.0.motion.branch = MotionBranch::NotYetDue;
            self.trace.0.motion.next_choice_at = Some(self.next_choice_quarters);
            return Ok(());
        }
        self.next_choice_quarters =
            quarters + fitted::tactical_cadence_quarters(self.experience.level);
        self.trace.0.motion.next_choice_at = Some(self.next_choice_quarters);

        self.pursuit = None;
        let request = self.choose_maneuver(frame, reason, recovering, target, geometry, batch)?;
        let intent = self.resolve(frame, clock, request, target, geometry, batch)?;
        self.active = Some(ActiveManeuver {
            intent,
            submitted_at: clock,
            formation: false,
            search: false,
            ordered: false,
        });
        batch.motion = Some(intent);
        self.trace.0.motion.branch = MotionBranch::NewManeuver;
        Ok(())
    }

    /// Investigate one frozen observation without turning it into a live
    /// target. Outside one nautical mile the actor flies to the observation;
    /// inside it flies a fitted, bounded clockwise level orbit. Without a
    /// lost contact of its own, the wing's mission of opportunity point is
    /// searched the same way.
    fn search_motion(
        &mut self,
        frame: &DecisionFrame<'_>,
        clock: CommandClock,
        batch: &mut IntentBatch,
    ) -> Result<bool> {
        let own = self
            .search_contact
            .filter(|contact| self.completed_search != Some(*contact));
        let (contact, opportunity) = match (own, self.wing_search) {
            (Some(contact), _) => (contact, false),
            (None, Some(contact)) => (contact, true),
            (None, None) => return Ok(false),
        };
        let started = *self.search_started_tick.get_or_insert(frame.tick);
        if !opportunity
            && self.experience.level == super::Experience::Ace
            && frame.tick.saturating_sub(started) >= ACE_SEARCH_LIMIT_TICKS
        {
            if self.active.is_some_and(|active| active.search) {
                self.active = None;
            }
            self.completed_search = Some(contact);
            batch.activity = Some(Activity::Rejoining);
            self.trace.0.motion.branch = MotionBranch::SearchAbandoned { contact };
            return Ok(true);
        }

        let dx = contact.position[0] - frame.own.position[0];
        let dy = contact.position[1] - frame.own.position[1];
        let dz = contact.position[2] - frame.own.position[2];
        let horizontal = dx.hypot(dz);
        let spatial = horizontal.hypot(dy);
        let bearing = if horizontal > f64::EPSILON {
            dx.atan2(dz).to_degrees()
        } else {
            frame.own.heading_deg
        };
        let (heading, pitch, steering_point) = if spatial > SEARCH_ORBIT_ENTRY_FT {
            self.search_orbit_altitude_ft = None;
            (
                bearing,
                dy.atan2(horizontal.max(f64::EPSILON)).to_degrees(),
                Some(contact.position),
            )
        } else {
            let orbit_altitude = *self
                .search_orbit_altitude_ft
                .get_or_insert(frame.own.position[1]);
            let correction = ((horizontal - SEARCH_ORBIT_RADIUS_FT)
                / SEARCH_ORBIT_CORRECTION_BAND_FT
                * SEARCH_ORBIT_MAX_CORRECTION_DEG)
                .clamp(
                    -SEARCH_ORBIT_MAX_CORRECTION_DEG,
                    SEARCH_ORBIT_MAX_CORRECTION_DEG,
                );
            let altitude_error = orbit_altitude - frame.own.position[1];
            (
                bearing - 90.0 + correction,
                altitude_error
                    .atan2(SEARCH_ORBIT_RADIUS_FT)
                    .to_degrees()
                    .clamp(
                        -SEARCH_ORBIT_MAX_CORRECTION_DEG,
                        SEARCH_ORBIT_MAX_CORRECTION_DEG,
                    ),
                None,
            )
        };
        let duration = Duration::Timed(SEARCH_COMMAND_SECONDS);
        let request = MotionRequest::new(
            heading.round() as i32,
            PitchRequest::Explicit(pitch.round() as i32),
            Bank::Unconstrained,
            SpeedRequest::Corner,
            duration,
        );
        let continuing = self.active.filter(|active| {
            active.search
                && matches!(active.intent.completion,
                    Completion::Deadline(deadline) if !motion::is_expired(deadline, clock))
        });
        let (id, completion, submitted_at) = if let Some(active) = continuing {
            (
                active.intent.id,
                active.intent.completion,
                active.submitted_at,
            )
        } else {
            let id = self.next_motion_id;
            self.next_motion_id += 1;
            (
                id,
                Completion::Deadline(
                    motion::deadline_for(duration, clock).expect("timed search maneuver"),
                ),
                clock,
            )
        };
        let intent = MotionIntent {
            id,
            request,
            heading_deg: heading.rem_euclid(360.0),
            flight_path_pitch_deg: pitch.clamp(-90.0, 90.0),
            speed: frame.own.limits.corner,
            bank: Bank::Unconstrained,
            completion,
            steering_point,
            mode: CommandMode::OtherState,
            formation_flight: false,
            afterburner: false,
        };
        self.active = Some(ActiveManeuver {
            intent,
            submitted_at,
            formation: false,
            search: true,
            ordered: false,
        });
        batch.motion = Some(intent);
        batch.activity = Some(Activity::Searching);
        self.trace.0.motion.branch = MotionBranch::Search {
            contact,
            distance_ft: spatial,
            // Only the fly-to branch steers at the contact itself.
            orbiting: steering_point.is_none(),
            opportunity,
        };
        Ok(true)
    }

    /// Fly toward the wing's mission of opportunity waypoint with the B48
    /// route command: its altitude (no leader jitter, so no draw) at cruise
    /// speed. The climb or dive toward that altitude is bounded to 20 degrees
    /// (fitted, as the mission rejoin).
    fn route_motion(
        &mut self,
        frame: &DecisionFrame<'_>,
        clock: CommandClock,
        batch: &mut IntentBatch,
    ) -> Result<bool> {
        let Some(point) = self.wing_route else {
            return Ok(false);
        };
        let dx = point[0] - frame.own.position[0];
        let dz = point[2] - frame.own.position[2];
        let distance_ft = dx.hypot(dz);
        let command = route::route_command(
            &route::RouteInputs {
                waypoint: Some(route::Waypoint {
                    altitude_ft: point[1],
                    speed: route::cruise_speed(&frame.own.limits),
                    landing: false,
                    distance_ft,
                }),
                has_airport: false,
                wing_leader: false,
                limits: frame.own.limits,
            },
            &mut self.random,
        )?;
        let route::RouteCommand::Fly {
            altitude_ft, speed, ..
        } = command
        else {
            return Ok(false);
        };
        let heading = if distance_ft > f64::EPSILON {
            dx.atan2(dz).to_degrees()
        } else {
            frame.own.heading_deg
        };
        let pitch = (altitude_ft - frame.own.position[1])
            .atan2(distance_ft.max(1.0))
            .to_degrees()
            .clamp(-20., 20.);
        let request = MotionRequest::new(
            heading.round() as i32,
            PitchRequest::Explicit(pitch.round() as i32),
            Bank::Unconstrained,
            SpeedRequest::Explicit(speed),
            Duration::Timed(3),
        );
        self.pursuit = None;
        batch.motion = Some(self.resolve(
            frame,
            clock,
            request,
            None,
            None,
            &mut IntentBatch::default(),
        )?);
        batch.activity = Some(Activity::Searching);
        self.trace.0.motion.branch = MotionBranch::WingRoute { point, distance_ft };
        Ok(true)
    }

    /// B43 slot geometry with opinionated physical departure/rejoin guidance.
    /// See the formation procedure in docs/spec/ai.md.
    fn formation_motion(
        &mut self,
        frame: &DecisionFrame<'_>,
        clock: CommandClock,
        batch: &mut IntentBatch,
    ) -> Result<bool> {
        let Some(point) = self.formation_point(frame)? else {
            return Ok(false);
        };
        let leader = frame
            .wing
            .leader
            .expect("formation point requires a leader");
        let configuration = (
            self.recipient.formation.unwrap_or(frame.wing.formation),
            self.recipient
                .horizontal_spacing_ft
                .unwrap_or(frame.wing.horizontal_spacing_ft),
            self.recipient
                .vertical_spacing_ft
                .unwrap_or(frame.wing.vertical_spacing_ft),
        );
        let slot_changed = self
            .formation_configuration
            .is_some_and(|old| old != configuration);
        if slot_changed {
            self.formation_guidance
                .change_slot(&frame.own, leader, point);
        }
        self.formation_configuration = Some(configuration);
        let guidance = self.formation_guidance.step(
            self.identity.actor.0,
            &frame.own,
            leader,
            leader.velocity,
            point,
            &self.formation_traffic,
            1. / 120.,
        );
        let aim = guidance.aim;
        let bank = guidance
            .bank_deg
            .map(|deg| Bank::Explicit(deg.round() as i32))
            .unwrap_or(Bank::Unconstrained);
        let dx = aim[0] - frame.own.position[0];
        let dy = aim[1] - frame.own.position[1];
        let dz = aim[2] - frame.own.position[2];
        let heading_deg = dx.atan2(dz).to_degrees();
        let mut pitch_deg = dy.atan2(dx.hypot(dz)).to_degrees();
        if frame.own.rotorcraft {
            // A transport helicopter is not asked for a fighter's descent
            // to its slot (slice HF3).
            pitch_deg = super::formation::rotorcraft_formation_path_deg(
                pitch_deg,
                frame.own.speed.0,
                leader.velocity[1],
            );
        }
        let speed = ScalarSpeed(guidance.speed);
        let duration = Duration::Timed(wing::FORMATION_REQUEST_SECONDS as u8);
        let request = MotionRequest::new(
            heading_deg.round() as i32,
            PitchRequest::Explicit(pitch_deg.round() as i32),
            bank,
            SpeedRequest::Explicit(speed),
            duration,
        );
        let continuing = self.active.filter(|a| {
            a.formation
                && matches!(a.intent.completion,
            Completion::Deadline(deadline) if !motion::is_expired(deadline, clock))
        });
        let (id, completion, submitted_at) = if let Some(active) = continuing {
            (
                active.intent.id,
                active.intent.completion,
                active.submitted_at,
            )
        } else {
            let id = self.next_motion_id;
            self.next_motion_id += 1;
            (
                id,
                Completion::Deadline(
                    motion::deadline_for(duration, clock).expect("timed formation"),
                ),
                clock,
            )
        };
        let intent = MotionIntent {
            formation_flight: guidance.close,
            afterburner: guidance.burner,
            id,
            request,
            heading_deg,
            flight_path_pitch_deg: pitch_deg,
            speed,
            bank,
            completion,
            steering_point: Some(aim),
            mode: CommandMode::OtherState,
        };
        self.active = Some(ActiveManeuver {
            intent,
            submitted_at,
            formation: true,
            search: false,
            ordered: false,
        });
        batch.motion = Some(intent);
        batch.activity = Some(Activity::Formation);
        self.trace.0.motion.branch = MotionBranch::Formation(FormationTrace {
            slot_point: point,
            aim,
            speed_fps: guidance.speed,
            bank_deg: guidance.bank_deg,
            close: guidance.close,
            burner: guidance.burner,
            slot_changed,
            continuing: continuing.is_some(),
        });
        Ok(true)
    }

    /// Whether a zero-duration maneuver's chosen axis has arrived (B13).
    fn axis_complete(&self, frame: &DecisionFrame<'_>, intent: &MotionIntent) -> bool {
        let Completion::Axis(axis) = intent.completion else {
            return false;
        };
        let tolerance = 1.0;
        match axis {
            motion::CompletionAxis::Heading => {
                angle_difference(frame.own.heading_deg, intent.heading_deg).abs() <= tolerance
            }
            motion::CompletionAxis::Pitch => {
                (frame.own.flight_path_pitch_deg - intent.flight_path_pitch_deg).abs() <= tolerance
            }
            motion::CompletionAxis::Bank => match intent.bank {
                Bank::Explicit(deg) => {
                    angle_difference(frame.own.bank_deg, f64::from(deg)).abs() <= tolerance
                }
                Bank::Unconstrained => true,
            },
        }
    }

    /// The tactical choice for this tick, as a bounded motion request.
    #[allow(clippy::too_many_arguments)]
    fn choose_maneuver(
        &mut self,
        frame: &DecisionFrame<'_>,
        reason: Option<ScriptReason>,
        recovering: bool,
        target: Option<TargetView>,
        geometry: Option<&geometry::TargetGeometry>,
        batch: &mut IntentBatch,
    ) -> Result<MotionRequest> {
        let heading = frame.own.heading_deg.round() as i32;
        let can_climb = geometry::can_climb(frame.own.speed, &frame.own.limits);

        // B48: bingo sends the actor home. The spec closes this for a wingman
        // with an AI leader only, so a leader or singleton uses the fitted
        // rule that flies the same private route.
        if recovering {
            if self.identity.is_leader() || !frame.route.leader_is_ai {
                self.note(Fallback::LeaderReturnToBase, batch);
            }
            batch.activity = Some(Activity::ReturningToBase);
            let speed = if frame.own.rotorcraft {
                route::rotorcraft_cruise_speed(&frame.own.limits)
            } else {
                route::cruise_speed(&frame.own.limits)
            };
            let home_heading_deg = self.home_heading(frame, heading);
            let request = MotionRequest::new(
                home_heading_deg,
                PitchRequest::Engagement,
                Bank::Unconstrained,
                SpeedRequest::Explicit(speed),
                Duration::Timed(route::ROUTE_COMMAND_SECONDS as u8),
            );
            self.trace.0.maneuver = Some(ManeuverTrace::new(
                ManeuverPath::ReturnToBase { home_heading_deg },
                Some(request),
            ));
            return Ok(request);
        }

        // B10: a launch reason has its own reactions.
        if let Some(launch) = match reason {
            Some(ScriptReason::IrLaunch) => Some(MissileLaunch::Infrared),
            Some(ScriptReason::RadarLaunch) => Some(MissileLaunch::Radar),
            _ => None,
        } {
            batch.activity = Some(Activity::Defending);
            let bearing = geometry
                .and_then(|g| g.angles)
                .map(|a| wrap_signed(frame.own.heading_deg + a.heading_error_deg))
                .unwrap_or(frame.own.heading_deg)
                .round() as i32;
            let off_beam = geometry
                .and_then(|g| g.angles)
                .map(|a| a.off_beam_deg)
                .unwrap_or(180.0);
            let response = tactics::missile_reaction(
                launch,
                MissileReactionInputs {
                    current_heading_deg: heading,
                    target_bearing_deg: bearing,
                    off_beam_deg: off_beam,
                },
                &mut self.random,
            );
            if let Some(wing::WingRequest::Break {
                heading_offset_deg,
                pitch_deg,
            }) = wingman_break_request(&response)
            {
                batch.wing.push(WingRequest::Break {
                    heading_offset_deg,
                    pitch_deg,
                });
            }
            let request = self.missile_request(response.reaction, frame);
            self.trace.0.maneuver = Some(ManeuverTrace::new(
                ManeuverPath::MissileReaction { launch, response },
                Some(request),
            ));
            return Ok(request);
        }

        // Without a target there is nothing to be tactical about; B48's
        // no-route rule is to hold the current heading.
        let (Some(target), Some(geometry)) = (target, geometry) else {
            let request = fitted::straight_flight(ManeuverFrame {
                current_heading_deg: heading,
                bank: Bank::Unconstrained,
                duration: Duration::Timed(route::ROUTE_COMMAND_SECONDS as u8),
            });
            self.trace.0.maneuver = Some(ManeuverTrace::new(ManeuverPath::NoTarget, Some(request)));
            return Ok(request);
        };
        let Some(angles) = geometry.angles else {
            // Directly above or below: the bearing is unknown, so hold.
            let request = fitted::straight_flight(ManeuverFrame {
                current_heading_deg: heading,
                bank: Bank::Unconstrained,
                duration: Duration::Timed(route::ROUTE_COMMAND_SECONDS as u8),
            });
            self.trace.0.maneuver =
                Some(ManeuverTrace::new(ManeuverPath::NoBearing, Some(request)));
            return Ok(request);
        };

        let situation = self.situation(frame, &target, geometry, &angles);
        let thresholds = self.thresholds(&angles);
        let quadrant = situation.quadrant();
        let mut trace = ManeuverTrace {
            situation: Some(situation),
            quadrant: Some(quadrant),
            thresholds: Some(thresholds.for_quadrant(quadrant)),
            ..ManeuverTrace::new(ManeuverPath::Approach, None)
        };

        // B11 evasion entry when the reason is evade or a hit.
        if matches!(reason, Some(ScriptReason::Evade) | Some(ScriptReason::Hit)) {
            batch.activity = Some(Activity::Evading);
            let choice = tactics::evasion_choice(&situation, &mut self.random);
            trace.path = ManeuverPath::Evasion;
            trace.choice = Some(choice);
            self.trace.0.maneuver = Some(trace);
            let request = self.request_for(choice, frame, &situation, &thresholds, batch)?;
            if let Some(maneuver) = self.trace.0.maneuver.as_mut() {
                maneuver.request = Some(request);
            }
            return Ok(request);
        }

        // B12 approach and tactical choice.
        let choice = match tactics::approach_choice(
            &situation,
            self.experience.level,
            &thresholds,
            &mut self.random,
        ) {
            Ok(choice) => choice,
            Err(AiError::UnspecifiedRule(branch)) => {
                // Two distinct unresolved branches surface here: the
                // random-tactic menu contents and the remaining tactics after
                // both draws fail. Record whichever one fired.
                if branch.contains("random-tactic menu") {
                    self.note(Fallback::RandomTacticMenu, batch);
                    let request = fitted::random_tactic_menu(
                        ManeuverFrame {
                            current_heading_deg: heading,
                            bank: Bank::Unconstrained,
                            duration: Duration::Timed(tactics::PURSUIT_DURATION_NEAR_S),
                        },
                        can_climb,
                        frame.own.agl_ft,
                        &mut self.random,
                    );
                    trace.fallback = Some(Fallback::RandomTacticMenu);
                    trace.request = Some(request);
                    self.trace.0.maneuver = Some(trace);
                    return Ok(request);
                }
                self.note(Fallback::RemainingTactics, batch);
                trace.fallback = Some(Fallback::RemainingTactics);
                fitted::remaining_tactic()
            }
            Err(other) => return Err(other),
        };
        trace.choice = Some(choice);
        self.trace.0.maneuver = Some(trace);
        let request = self.request_for(choice, frame, &situation, &thresholds, batch)?;
        if let Some(maneuver) = self.trace.0.maneuver.as_mut() {
            maneuver.request = Some(request);
        }
        Ok(request)
    }

    /// Turn a behavior choice into a bounded motion request.
    fn request_for(
        &mut self,
        choice: BehaviorChoice,
        frame: &DecisionFrame<'_>,
        situation: &TacticalSituation,
        _thresholds: &TacticalThresholds,
        batch: &mut IntentBatch,
    ) -> Result<MotionRequest> {
        let heading = frame.own.heading_deg.round() as i32;
        let can_climb = geometry::can_climb(frame.own.speed, &frame.own.limits);
        let near = situation.spatial_distance_ft < tactics::PURSUIT_NEAR_DISTANCE_FT;
        let duration = Duration::Timed(if near {
            tactics::PURSUIT_DURATION_NEAR_S
        } else {
            tactics::PURSUIT_DURATION_FAR_S
        });
        let frame_for = |duration| ManeuverFrame {
            current_heading_deg: heading,
            bank: Bank::Unconstrained,
            duration,
        };

        Ok(match choice {
            BehaviorChoice::Pursuit => {
                batch.activity = Some(Activity::Pursuing);
                // The steering point and the regulated speed are resolved in
                // `resolve`; the request carries the recovered bounds.
                MotionRequest::new(
                    heading,
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Corner,
                    duration,
                )
            }
            BehaviorChoice::Straight => fitted::straight_flight(frame_for(duration)),
            BehaviorChoice::FlyAway => {
                batch.activity = Some(Activity::Evading);
                MotionRequest::new(
                    heading + 180,
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Maximum,
                    duration,
                )
            }
            BehaviorChoice::VerticalJink => {
                batch.activity = Some(Activity::Defending);
                motion::straight_climb(frame_for(Duration::Timed(2)), can_climb)
            }
            BehaviorChoice::LastDitch => {
                batch.activity = Some(Activity::Defending);
                // B11 leaves candidate suitability and weights open.
                self.note(Fallback::LastDitchCandidate, batch);
                let rates = self.rates(frame, batch)?;
                let radius = steering::turn_radius_feet(frame.own.speed, rates.turn_deg_per_s)
                    .unwrap_or(0.0);
                let candidate = fitted::last_ditch_candidate(
                    frame.own.agl_ft,
                    radius,
                    frame.own.speed,
                    &frame.own.limits,
                    &mut self.random,
                );
                if let Some(maneuver) = self.trace.0.maneuver.as_mut() {
                    maneuver.last_ditch = Some(candidate);
                }
                self.last_ditch_request(candidate, frame, can_climb)
            }
            BehaviorChoice::WingSplit => {
                batch.wing.push(WingRequest::Break {
                    heading_offset_deg: tactics::RADAR_BREAK_TURN_DEG,
                    pitch_deg: 0,
                });
                fitted::straight_flight(frame_for(duration))
            }
            BehaviorChoice::SpecialApproach(_) => {
                batch.activity = Some(Activity::Pursuing);
                MotionRequest::new(
                    heading,
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Corner,
                    duration,
                )
            }
            BehaviorChoice::CoordinatedEscape(escape) => {
                batch.activity = Some(Activity::Evading);
                let offset = match escape {
                    tactics::CoordinatedEscape::CrossTurn => 180,
                    tactics::CoordinatedEscape::LeftRightSplit => -90,
                    tactics::CoordinatedEscape::HighLowSplit => 0,
                };
                let pitch = match escape {
                    tactics::CoordinatedEscape::HighLowSplit => PitchRequest::Explicit(45),
                    _ => PitchRequest::Engagement,
                };
                MotionRequest::new(
                    heading + offset,
                    pitch,
                    Bank::Unconstrained,
                    SpeedRequest::Corner,
                    duration,
                )
            }
            BehaviorChoice::NonAircraftEvasion(evasion) => {
                batch.activity = Some(Activity::Evading);
                // B11 records the altitude frame as unresolved; the request
                // keeps the current heading and maximum speed, which is the
                // part the spec does establish.
                let _ = evasion.altitude_operand_ft;
                MotionRequest::new(
                    heading,
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Maximum,
                    duration,
                )
            }
            BehaviorChoice::MissileReaction(reaction) => {
                batch.activity = Some(Activity::Defending);
                self.missile_request(reaction, frame)
            }
            BehaviorChoice::SurfaceAttack(_) => {
                batch.activity = Some(Activity::Attacking);
                MotionRequest::new(
                    heading,
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Corner,
                    duration,
                )
            }
        })
    }

    fn last_ditch_request(
        &mut self,
        candidate: tactics::LastDitchCandidate,
        frame: &DecisionFrame<'_>,
        can_climb: bool,
    ) -> MotionRequest {
        let heading = frame.own.heading_deg.round() as i32;
        let short = Duration::Timed(3);
        let frame_for = ManeuverFrame {
            current_heading_deg: heading,
            bank: Bank::Unconstrained,
            duration: short,
        };
        match candidate {
            tactics::LastDitchCandidate::SplitS => {
                motion::straight_dive(frame_for, frame.own.agl_ft)
            }
            tactics::LastDitchCandidate::Loop => motion::straight_climb(frame_for, can_climb),
            tactics::LastDitchCandidate::Reverse
            | tactics::LastDitchCandidate::HorizontalScissors => {
                motion::turn_around(frame_for, &mut self.random).1
            }
            tactics::LastDitchCandidate::Overshoot => MotionRequest::new(
                heading,
                PitchRequest::Engagement,
                Bank::Unconstrained,
                SpeedRequest::Explicit(frame.own.limits.minimum),
                short,
            ),
            tactics::LastDitchCandidate::HorizontalJink => {
                if self.random.site("last-ditch jink left").chance(50) {
                    motion::break_left(frame_for)
                } else {
                    motion::break_right(frame_for)
                }
            }
            tactics::LastDitchCandidate::VerticalJink => {
                motion::straight_climb(frame_for, can_climb)
            }
        }
    }

    fn missile_request(
        &mut self,
        reaction: MissileReaction,
        frame: &DecisionFrame<'_>,
    ) -> MotionRequest {
        match reaction {
            MissileReaction::ClimbToPitch90 { heading_deg } => MotionRequest::new(
                i32::from(heading_deg),
                PitchRequest::Explicit(tactics::INFRARED_CLIMB_PITCH_DEG),
                Bank::Unconstrained,
                SpeedRequest::Maximum,
                Duration::Timed(3),
            ),
            MissileReaction::TurnTowardTarget { heading_deg } => MotionRequest::new(
                i32::from(heading_deg),
                PitchRequest::Engagement,
                Bank::Unconstrained,
                SpeedRequest::Corner,
                Duration::Timed(3),
            ),
            MissileReaction::BreakNinety { heading_deg, .. } => {
                let _ = frame;
                MotionRequest::new(
                    i32::from(heading_deg),
                    PitchRequest::Engagement,
                    Bank::Unconstrained,
                    SpeedRequest::Corner,
                    Duration::Timed(3),
                )
            }
        }
    }

    /// Resolve a bounded request into concrete numbers for a steering adapter.
    fn resolve(
        &mut self,
        frame: &DecisionFrame<'_>,
        clock: CommandClock,
        request: MotionRequest,
        target: Option<TargetView>,
        geometry: Option<&geometry::TargetGeometry>,
        batch: &mut IntentBatch,
    ) -> Result<MotionIntent> {
        let can_climb = geometry::can_climb(frame.own.speed, &frame.own.limits);

        // B10/B13 engagement pitch is unresolved; the fitted rule supplies one.
        let pitch_source;
        let pitch = match request.pitch {
            PitchRequest::Explicit(deg) => {
                let pitch = f64::from(deg);
                pitch_source = PitchSource::Explicit { pitch_deg: pitch };
                pitch
            }
            PitchRequest::Engagement => {
                self.note(Fallback::EngagementPitch, batch);
                let relative = target
                    .map(|t| t.position[1] - frame.own.position[1])
                    .unwrap_or(0.0);
                let run = geometry.map(|g| g.horizontal_distance_feet).unwrap_or(1.0);
                let pitch = fitted::engagement_pitch_deg(relative, run, can_climb);
                pitch_source = PitchSource::Engagement {
                    relative_altitude_ft: relative,
                    horizontal_distance_ft: run,
                    can_climb,
                    pitch_deg: pitch,
                };
                pitch
            }
        };

        // B15 speed regulation and the pursuit steering point.
        let mut steering_point = None;
        let mut speed = request.speed.resolve(frame.own.limits)?;
        let resolved_speed = speed;
        let mut pursuit_trace = None;
        if batch.activity == Some(Activity::Pursuing)
            && let (Some(target), Some(geometry)) = (target, geometry)
            && let Some(angles) = geometry.angles
        {
            let condition = self.pursuit_condition(&angles);
            let offsets = tactics::pursuit_offsets(
                condition,
                &self.thresholds(&angles),
                geometry.spatial_distance_feet,
                &mut self.random,
            );
            let point = pursuit::steering_point(
                target.position,
                target.heading_deg,
                PursuitOffset {
                    longitudinal_feet: f64::from(offsets.longitudinal_ft),
                    lateral_feet: f64::from(offsets.lateral_ft),
                    vertical_feet: f64::from(offsets.vertical_ft),
                },
            );
            self.pursuit = Some((
                target.id,
                PursuitOffset {
                    longitudinal_feet: f64::from(offsets.longitudinal_ft),
                    lateral_feet: f64::from(offsets.lateral_ft),
                    vertical_feet: f64::from(offsets.vertical_ft),
                },
            ));
            steering_point = Some(point);
            let mut regulated_speed_fps = None;
            if request.speed == SpeedRequest::Corner {
                speed = pursuit::regulate_speed(
                    f64::from(offsets.longitudinal_ft.abs()),
                    geometry.spatial_distance_feet,
                    target.speed,
                    angles.heading_error_deg,
                    angles.pitch_error_deg,
                    &frame.own.limits,
                    false,
                );
                regulated_speed_fps = Some(speed.0);
            }
            pursuit_trace = Some(PursuitTrace {
                condition,
                offsets,
                steering_point: point,
                regulated_speed_fps,
            });
        }

        // The completion rule (B13).
        let mut completion_axis_fitted = false;
        let completion = match motion::deadline_for(request.duration, clock) {
            Some(deadline) => Completion::Deadline(deadline),
            None => {
                // Zero duration selects one axis; the selection function is
                // unresolved, so the fitted rule picks the slowest axis.
                self.note(Fallback::CompletionAxis, batch);
                completion_axis_fitted = true;
                let rates = self.rates(frame, batch)?;
                Completion::Axis(fitted::completion_axis(
                    angle_difference(frame.own.heading_deg, f64::from(request.heading_deg)),
                    frame.own.flight_path_pitch_deg - pitch,
                    match request.bank {
                        Bank::Explicit(deg) => {
                            Some(angle_difference(frame.own.bank_deg, f64::from(deg)))
                        }
                        Bank::Unconstrained => None,
                    },
                    rates.turn_deg_per_s,
                    rates.pitch_deg_per_s,
                    rates.roll_deg_per_s,
                ))
            }
        };

        self.trace.0.resolve = Some(ResolveTrace {
            request,
            pitch: pitch_source,
            speed_fps: resolved_speed.0,
            pursuit: pursuit_trace,
            completion,
            completion_axis_fitted,
        });

        let id = self.next_motion_id;
        self.next_motion_id += 1;
        let mut intent = MotionIntent {
            formation_flight: false,
            afterburner: false,
            id,
            request,
            heading_deg: f64::from(request.heading_deg),
            flight_path_pitch_deg: pitch.clamp(-90.0, 90.0),
            speed,
            bank: request.bank,
            completion,
            steering_point,
            mode: CommandMode::OtherState,
        };
        self.update_pursuit(frame, target, &mut intent);
        Ok(intent)
    }

    fn update_pursuit(
        &self,
        frame: &DecisionFrame<'_>,
        target: Option<TargetView>,
        intent: &mut MotionIntent,
    ) {
        let Some((id, offset)) = self.pursuit else {
            return;
        };
        let Some(target) = target.filter(|t| t.id == id) else {
            return;
        };
        let point = pursuit::steering_point(target.position, target.heading_deg, offset);
        let delta: [f64; 3] = std::array::from_fn(|i| point[i] - frame.own.position[i]);
        intent.steering_point = Some(point);
        intent.heading_deg = delta[0].atan2(delta[2]).to_degrees().rem_euclid(360.0);
        intent.flight_path_pitch_deg = delta[1].atan2(delta[0].hypot(delta[2])).to_degrees();
        if let Ok(g) = self.geometry(frame, &target)
            && let Some(a) = g.angles
        {
            intent.speed = pursuit::regulate_speed(
                offset.longitudinal_feet.abs(),
                g.spatial_distance_feet,
                target.speed,
                a.heading_error_deg,
                a.pitch_error_deg,
                &frame.own.limits,
                false,
            );
        }
    }

    /// B44 axis rates, with the fitted base pitch rate.
    fn rates(&mut self, frame: &DecisionFrame<'_>, batch: &mut IntentBatch) -> Result<AxisRates> {
        self.note(Fallback::BasePitchRate, batch);
        let pitch = fitted::base_pitch_rate_deg_per_s(frame.own.g_limit, frame.own.speed)?;
        AxisRates::from_limits(
            frame.own.g_limit,
            frame.own.speed,
            frame.own.roll_limit_deg_per_s,
            pitch,
        )
    }

    fn pursuit_condition(&self, angles: &RelativeAngles) -> PursuitCondition {
        if !angles.ahead && angles.facing {
            PursuitCondition::Chased
        } else if angles.ahead && angles.facing && angles.heading_difference_deg > 155.0 {
            PursuitCondition::HeadOn
        } else {
            PursuitCondition::Ordinary
        }
    }

    fn situation(
        &self,
        frame: &DecisionFrame<'_>,
        target: &TargetView,
        geometry: &geometry::TargetGeometry,
        angles: &RelativeAngles,
    ) -> TacticalSituation {
        TacticalSituation {
            target: if target.is_aircraft {
                TargetClass::Aircraft {
                    fighter: target.is_fighter,
                    human: target.human_controlled,
                }
            } else {
                TargetClass::NonAircraft
            },
            spatial_distance_ft: geometry.spatial_distance_feet,
            ahead: angles.ahead,
            facing: angles.facing,
            off_beam_deg: angles.off_beam_deg,
            heading_difference_deg: angles.heading_difference_deg,
            pitch_difference_deg: angles.pitch_difference_deg,
            wing_combat: frame.wing.wing_combat,
            wing_approach: frame.wing.wing_approach,
            wing_approach_value_ft: frame.wing.wing_approach_value_ft,
            own_agl_ft: frame.own.agl_ft,
        }
    }

    /// Assemble the tactics thresholds from the experience tables.
    fn thresholds(&self, _angles: &RelativeAngles) -> TacticalThresholds {
        let quadrant = |situation| {
            let t = experience::tactical_thresholds(self.experience.level, situation);
            QuadrantThresholds {
                best_attack_percent: t.best_attack_percent,
                random_tactic_percent: t.random_tactic_percent,
            }
        };
        TacticalThresholds {
            ahead_facing: quadrant(experience::TargetSituation::AheadFacing),
            ahead_facing_away: quadrant(experience::TargetSituation::AheadFacingAway),
            behind_facing: quadrant(experience::TargetSituation::BehindFacing),
            behind_facing_away: quadrant(experience::TargetSituation::BehindFacingAway),
            straight_on_random_menu_percent: experience::straight_on_random_menu_percent(
                self.experience.level,
            ),
            chased_vertical_displacement_percent: experience::pursuit_vertical_displacement_percent(
                self.experience.level,
            ),
            pursuit_offset_bound: experience::pursuit_offset_draw_bound(self.experience.level),
        }
    }

    fn home_heading(&self, frame: &DecisionFrame<'_>, current: i32) -> i32 {
        let Some(home) = frame.route.home_airport else {
            return current;
        };
        let dx = home.x - frame.own.position[0];
        let dz = home.z - frame.own.position[2];
        if dx == 0.0 && dz == 0.0 {
            return current;
        }
        dx.atan2(dz).to_degrees().round() as i32
    }

    fn activity(&self, frame: &DecisionFrame<'_>, recovering: bool, has_target: bool) -> Activity {
        if recovering {
            return Activity::ReturningToBase;
        }
        if has_target {
            return Activity::Pursuing;
        }
        if !self.identity.is_leader() && frame.wing.leader.is_some() {
            return Activity::Formation;
        }
        if self.profile.role == MissionRole::AirToAir {
            return Activity::Searching;
        }
        Activity::Idle
    }

    fn note(&mut self, fallback: Fallback, batch: &mut IntentBatch) {
        self.fallbacks.record(fallback);
        if !batch.fallbacks.contains(&fallback) {
            batch.fallbacks.push(fallback);
        }
    }

    /// The formation point this actor should fly to, in world feet (B43).
    ///
    /// Exposed separately from `step` because a wingman's formation motion is
    /// the leader-relative alternative to its own maneuver; the host installs
    /// it when the controller produces no other motion.
    pub fn formation_point(&mut self, frame: &DecisionFrame<'_>) -> Result<Option<[f64; 3]>> {
        let Some(leader) = frame.wing.leader else {
            return Ok(None);
        };
        if self.identity.is_leader() {
            return Ok(None);
        }
        self.variation.advance(frame.tick, &mut self.random);
        let slot = wing::formation_slot_point(
            self.recipient.formation.unwrap_or(frame.wing.formation),
            frame.wing.slot,
            self.recipient
                .horizontal_spacing_ft
                .unwrap_or(frame.wing.horizontal_spacing_ft),
            self.recipient
                .vertical_spacing_ft
                .unwrap_or(frame.wing.vertical_spacing_ft),
        )?;
        let raw = self.variation.offset_ft();
        if self.variation_tick != Some(frame.tick) {
            let dt = self.variation_tick.map_or(1. / 120., |last| {
                frame.tick.saturating_sub(last) as f64 / 120.
            });
            let blend = 1. - (-dt / 3.).exp();
            for (i, value) in raw.into_iter().enumerate() {
                let target = f64::from(value) * if i == 2 { 0.1 } else { 1. };
                self.smooth_variation[i] += (target - self.smooth_variation[i]) * blend;
            }
            self.variation_tick = Some(frame.tick);
        }
        let [lateral, longitudinal, vertical] = self.smooth_variation;
        let h = leader.heading_deg.to_radians();
        let offset = [
            (slot[0] + lateral) * h.cos() + (slot[2] + longitudinal) * h.sin(),
            slot[1] + vertical,
            (slot[2] + longitudinal) * h.cos() - (slot[0] + lateral) * h.sin(),
        ];
        Ok(Some([
            leader.position[0] + offset[0],
            leader.position[1] + offset[1],
            leader.position[2] + offset[2],
        ]))
    }

    /// The B44 terrain floor for this actor, when one is active.
    pub fn terrain_floor(&mut self, frame: &DecisionFrame<'_>) -> Result<Option<f64>> {
        let mut scratch = IntentBatch::default();
        let rates = self.rates(frame, &mut scratch)?;
        let radius = steering::turn_radius_feet(frame.own.speed, rates.turn_deg_per_s)?;
        let floor = steering::terrain_pitch_floor(&TerrainInputs {
            altitude_feet: frame.own.altitude_msl_ft,
            terrain_ahead_feet: frame.own.terrain_ahead_ft,
            minimum_altitude_feet: frame.own.minimum_altitude_ft,
            turn_radius_feet: radius,
        })?;
        // Opinionated (John, 2026-09-29): with the longer look-ahead the
        // floor also asks for the climb gradient that clears the highest
        // ground it can see, which B44's arc test alone reduces to 5 degrees
        // at supersonic turn radii.
        Ok(Some(
            floor.pitch_floor_deg.max(
                frame
                    .own
                    .terrain_climb_deg
                    .min(steering::PITCH_REQUEST_LIMIT_DEG),
            ),
        ))
    }

    /// Build the steering request for a resolved intent (B44).
    pub fn steering_request(
        &self,
        frame: &DecisionFrame<'_>,
        intent: &MotionIntent,
        terrain_pitch_floor_deg: Option<f64>,
    ) -> SteeringRequest {
        SteeringRequest {
            heading_deg: intent.heading_deg,
            flight_path_pitch_deg: intent.flight_path_pitch_deg,
            bank_deg: match intent.bank {
                Bank::Explicit(deg) => f64::from(deg),
                Bank::Unconstrained => 0.0,
            },
            reference_bank_deg: frame.own.maximum_bank_deg,
            at_ceiling: frame.own.at_ceiling,
            terrain_pitch_floor_deg,
            on_ground: frame.own.on_ground,
            mode: intent.mode,
        }
    }
}

/// B13: gun lead tracking is issued as a 1-second timed motion.
const GUN_TRACKING_MOTION_TICKS: u64 = 120;

/// Why the controller could not use the mission's target: the first failing
/// test of the mission-target filter in `select_target`. Explanation only.
fn mission_target_refusal(
    frame: &DecisionFrame<'_>,
    identity: &ActorIdentity,
    id: u32,
) -> MissionTargetRefusal {
    let Some(target) = frame.targets.iter().find(|t| t.id == id) else {
        return MissionTargetRefusal::NotObserved;
    };
    if target.id == identity.actor.0 {
        MissionTargetRefusal::Own
    } else if target.side == identity.side {
        MissionTargetRefusal::SameSide
    } else if !target.valid {
        MissionTargetRefusal::Invalid
    } else if !target.type_allowed {
        MissionTargetRefusal::TypeNotAllowed
    } else {
        MissionTargetRefusal::NoUsableWeapon
    }
}

fn wingman_break_request(response: &tactics::MissileResponse) -> Option<WingRequest> {
    match response.wingman_break? {
        tactics::WingmanBreak::Offsets {
            heading_deg,
            pitch_deg,
        } => Some(WingRequest::Break {
            heading_offset_deg: heading_deg,
            pitch_deg,
        }),
        tactics::WingmanBreak::UnrecoveredOffsets => None,
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Signed smallest difference from `current` to `requested`, in degrees.
fn angle_difference(current: f64, requested: f64) -> f64 {
    let mut delta = (requested - current) % 360.0;
    if delta > 180.0 {
        delta -= 360.0;
    }
    if delta < -180.0 {
        delta += 360.0;
    }
    delta
}

fn wrap_signed(value: f64) -> f64 {
    let mut v = value % 360.0;
    if v < 0.0 {
        v += 360.0;
    }
    v
}

#[cfg(test)]
impl Controller {
    /// Raw state of the decision generator, for the golden behaviour tests:
    /// recording code must never consume a draw. Test builds only.
    pub(crate) fn decision_draw_state(&self) -> u64 {
        self.random.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Experience;

    fn identity() -> ActorIdentity {
        ActorIdentity {
            actor: ActorId(7),
            side: targeting::Side(1),
            wing: 0,
            member: 0,
            leads: true,
            aircraft: AircraftId::F18,
            human_controlled: false,
        }
    }

    fn profile() -> BehaviorProfile {
        BehaviorProfile {
            family: BehaviorFamily::FighterStrike,
            role: MissionRole::AirToAir,
        }
    }

    fn resolved(level: Experience) -> ResolvedExperience {
        ResolvedExperience {
            level,
            origin: experience::ExperienceOrigin::QuickMission { selected: level },
        }
    }

    fn controller(level: Experience) -> Controller {
        Controller::new(identity(), profile(), resolved(level), 1234).unwrap()
    }

    fn own() -> OwnState {
        OwnState {
            position: [0.0, 20000.0, 0.0],
            heading_deg: 0.0,
            flight_path_pitch_deg: 0.0,
            body_pitch_offset_deg: 2.0,
            bank_deg: 0.0,
            speed: ScalarSpeed(800.0),
            limits: SpeedLimits {
                minimum: ScalarSpeed(220.0),
                maximum: ScalarSpeed(1600.0),
                corner: ScalarSpeed(700.0),
            },
            altitude_msl_ft: 20000.0,
            agl_ft: 20000.0,
            terrain_ahead_ft: 0.0,
            terrain_climb_deg: -90.0,
            minimum_altitude_ft: 300.0,
            at_ceiling: false,
            on_ground: false,
            g_limit: 7.0,
            roll_limit_deg_per_s: 180.0,
            maximum_bank_deg: 80.0,
            alive: true,
            fuel_endurance_s: 3600.0,
            time_home_s: Some(600.0),
            rotorcraft: false,
            internal_fuel_lbs: 8000.0,
            radar_emitting: true,
        }
    }

    fn enemy(id: u32, position: [f64; 3]) -> TargetView {
        TargetView {
            infrared_strength: 1.,
            id,
            side: targeting::Side(2),
            position,
            heading_deg: 180.0,
            pitch_deg: 0.0,
            speed: ScalarSpeed(750.0),
            maximum_speed: ScalarSpeed(1500.0),
            is_aircraft: true,
            is_fighter: true,
            human_controlled: false,
            valid: true,
            type_allowed: true,
            seeker_eligible: true,
            wing_attackers: 0,
            terrain_blocked: false,
            sensor_supported: true,
            link_track: false,
        }
    }

    fn wing_view() -> WingView {
        WingView {
            control: WingControl::Loose,
            formation: Formation::Echelon,
            horizontal_spacing_ft: 2048,
            vertical_spacing_ft: 512,
            slot: 1,
            leader: None,
            wingmen_in_formation: 1,
            wing_combat: false,
            wing_approach: false,
            wing_approach_value_ft: None,
        }
    }

    struct Scene {
        targets: Vec<TargetView>,
        events: Vec<FrameEvent>,
        stations: Vec<StationView>,
        dispensers: Vec<threat::DispenserStore>,
    }

    impl Scene {
        fn new() -> Self {
            Self {
                targets: vec![enemy(2, [0.0, 20000.0, 8000.0])],
                events: Vec::new(),
                stations: vec![StationView {
                    station: StationId(0),
                    guided: true,
                    capability: StoreCapability::AIR_TO_AIR_MISSILE,
                    inhibited: false,
                    rounds: weapon_service::Rounds::Finite(4),
                    pointing_error_deg: 2.0,
                    employment_limit_deg: Some(30.0),
                    employment_fit: Some(1.0),
                    minimum_range_ft: 0.0,
                    maximum_range_ft: Some(40000.0),
                    energy_target: None,
                    infrared_envelope: None,
                    requires_radar: false,
                    requires_sensor: false,
                    employment_zone: None,
                    mount: [0.0; 3],
                    damage_vs_category: 100.0,
                    store_speed: ScalarSpeed(2000.0),
                    tracking_delay: Delay::seconds(1),
                    pacing: ProjectilePacing {
                        burst_count: 1,
                        burst_interval: Delay::seconds(0),
                        reload: Delay::seconds(2),
                        startup: Delay::seconds(0),
                    },
                }],
                dispensers: vec![
                    threat::DispenserStore {
                        class: SeekerClass::Infrared,
                        count: 30,
                    },
                    threat::DispenserStore {
                        class: SeekerClass::Radar,
                        count: 30,
                    },
                ],
            }
        }

        fn frame(&self, tick: u64, own: OwnState) -> DecisionFrame<'_> {
            DecisionFrame {
                tick,
                own,
                targets: &self.targets,
                events: &self.events,
                stations: &self.stations,
                dispensers: &self.dispensers,
                wing: wing_view(),
                route: RouteView {
                    home_airport: Some(route::Position { x: 0.0, z: 0.0 }),
                    leader_is_ai: true,
                },
                now: TimeOfDay(0),
                flight_state: FlightState::Free,
            }
        }
    }

    #[test]
    fn only_the_fighter_strike_family_is_implemented() {
        for family in [
            BehaviorFamily::F117,
            BehaviorFamily::Helicopter,
            BehaviorFamily::Bomber,
            BehaviorFamily::AC130,
            BehaviorFamily::LargeAircraft,
            BehaviorFamily::Airliner,
            BehaviorFamily::Moth,
        ] {
            let built = Controller::new(
                identity(),
                BehaviorProfile {
                    family,
                    role: MissionRole::AirToAir,
                },
                resolved(Experience::Ace),
                1,
            );
            assert!(
                matches!(built, Err(AiError::UnspecifiedRule(_))),
                "{family:?}"
            );
        }
        assert!(Controller::new(identity(), profile(), resolved(Experience::Ace), 1).is_ok());
    }

    #[test]
    fn formation_motion_tracks_its_leader_and_preserves_repeated_tick_determinism() {
        let mut identity = identity();
        identity.member = 1;
        identity.leads = false;
        let mut c = Controller::new(identity, profile(), resolved(Experience::Ace), 1234).unwrap();
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut frame = scene.frame(0, own());
        frame.wing.horizontal_spacing_ft = 512;
        frame.wing.vertical_spacing_ft = 0;
        frame.wing.leader = Some(LeaderView {
            velocity: [0., 0., 800.],
            position: [10000., 20000., 10000.],
            heading_deg: 0.,
            speed: ScalarSpeed(800.),
            target: None,
            recovering: false,
            on_ground: false,
        });
        let first = c.step(&frame).unwrap();
        assert_eq!(first.activity, Some(Activity::Formation));
        assert!(first.motion.unwrap().heading_deg > 0.);
        let random = c.random.clone();
        assert_eq!(first, c.step(&frame).unwrap());
        assert_eq!(random, c.random);
        frame.tick = 1;
        frame.wing.leader.as_mut().unwrap().position[0] = -10000.;
        let next = c.step(&frame).unwrap();
        assert!(next.motion.unwrap().heading_deg < 0.);
        assert_eq!(first.motion.unwrap().id, next.motion.unwrap().id);
        assert_eq!(random, c.random, "slot tracking must not redraw every tick");
    }

    #[test]
    fn a_human_controlled_aircraft_is_never_given_a_controller() {
        let mut human = identity();
        human.human_controlled = true;
        assert!(matches!(
            Controller::new(human, profile(), resolved(Experience::Ace), 1),
            Err(AiError::InvalidInput(_))
        ));
    }

    #[test]
    fn identical_seeds_and_inputs_give_identical_results() {
        let scene = Scene::new();
        let mut a = controller(Experience::Experienced);
        let mut b = controller(Experience::Experienced);
        for tick in 0..2400 {
            let frame = scene.frame(tick, own());
            let left = a.step(&frame).unwrap();
            let right = b.step(&frame).unwrap();
            assert_eq!(left, right, "diverged at tick {tick}");
        }
        assert_eq!(a.fallbacks().total(), b.fallbacks().total());
    }

    #[test]
    fn different_seeds_can_diverge() {
        let scene = Scene::new();
        let mut a =
            Controller::new(identity(), profile(), resolved(Experience::Novice), 1).unwrap();
        let mut b =
            Controller::new(identity(), profile(), resolved(Experience::Novice), 2).unwrap();
        let mut differed = false;
        for tick in 0..2400 {
            let frame = scene.frame(tick, own());
            if a.step(&frame).unwrap() != b.step(&frame).unwrap() {
                differed = true;
                break;
            }
        }
        assert!(differed, "two seeds produced identical streams");
    }

    #[test]
    fn a_repeated_tick_repeats_the_batch_without_drawing() {
        let scene = Scene::new();
        let mut c = controller(Experience::Average);
        let frame = scene.frame(0, own());
        let first = c.step(&frame).unwrap();
        let random_after_first = c.random.clone();
        for _ in 0..50 {
            let again = c.step(&frame).unwrap();
            assert_eq!(first, again);
        }
        assert_eq!(
            c.random, random_after_first,
            "a repeated tick must not advance the draw state"
        );
    }

    #[test]
    fn a_backwards_tick_is_invalid_input() {
        let scene = Scene::new();
        let mut c = controller(Experience::Average);
        c.step(&scene.frame(100, own())).unwrap();
        assert!(matches!(
            c.step(&scene.frame(99, own())),
            Err(AiError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_tactic_is_not_redrawn_every_tick() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let mut ids = Vec::new();
        for tick in 0..1200 {
            let batch = c.step(&scene.frame(tick, own())).unwrap();
            if let Some(motion) = batch.motion
                && ids.last() != Some(&motion.id)
            {
                ids.push(motion.id);
            }
        }
        // Ten seconds of simulation must not produce anywhere near 1200
        // separate maneuvers.
        assert!(ids.len() < 60, "{} maneuvers in 10 s", ids.len());
        assert!(!ids.is_empty(), "no maneuver was ever produced");
    }

    #[test]
    fn the_controller_always_produces_motion_rather_than_stalling() {
        let scene = Scene::new();
        for level in Experience::ALL {
            let mut c = controller(level);
            let mut produced = 0;
            for tick in 0..2400 {
                let batch = c.step(&scene.frame(tick, own())).unwrap();
                if batch.motion.is_some() {
                    produced += 1;
                }
            }
            assert_eq!(produced, 2400, "{level:?} stalled without a maneuver");
        }
    }

    #[test]
    fn every_fitted_fallback_used_is_recorded() {
        let scene = Scene::new();
        let mut c = controller(Experience::Novice);
        for tick in 0..2400 {
            let batch = c.step(&scene.frame(tick, own())).unwrap();
            for fallback in &batch.fallbacks {
                assert!(c.fallbacks().count(*fallback) > 0);
            }
        }
        let applied = c.fallbacks().applied();
        assert!(!applied.is_empty(), "no fallback was recorded at all");
        // The engagement pitch is unavoidable for a pursuing fighter.
        assert!(c.fallbacks().count(Fallback::EngagementPitch) > 0);
    }

    #[test]
    fn a_dead_actor_produces_no_intents() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let mut state = own();
        state.alive = false;
        let batch = c.step(&scene.frame(0, state)).unwrap();
        assert_eq!(batch.activity, Some(Activity::Destroyed));
        assert!(batch.motion.is_none());
        assert!(batch.weapons.is_empty());
    }

    #[test]
    fn a_missile_warning_raises_the_reason_and_can_release_devices() {
        let mut scene = Scene::new();
        // B47 flies no evasive maneuver when the launcher is already the
        // aircraft's current target, so the shooter here is a third aircraft.
        scene.events.push(FrameEvent::ThreatReported(ThreatReport {
            missile_id: 99,
            seeker: SeekerClass::Infrared,
            launcher_id: 77,
            launcher_same_side: false,
            distance_at_launch_ft: 5000.0,
            launch_tick: 0,
        }));
        let mut c = controller(Experience::Ace);
        // An Ace in ordinary flight is warned after 6 s plus its 0 s
        // experience term, so nothing happens on the first tick.
        let early = c.step(&scene.frame(0, own())).unwrap();
        assert_eq!(early.reason, None);
        assert!(early.launch_calls.is_empty(), "no call before the warning");
        // Well past the delay the reason is raised.
        let late = c.step(&scene.frame(1200, own())).unwrap();
        assert_eq!(late.reason, Some(ScriptReason::IrLaunch));
        // The accepted opposite-side warning is the radio's launch call.
        assert_eq!(late.launch_calls, [77]);
    }

    #[test]
    fn a_warning_from_the_same_side_flies_no_evasive_maneuver() {
        let mut scene = Scene::new();
        scene.events.push(FrameEvent::ThreatReported(ThreatReport {
            missile_id: 99,
            seeker: SeekerClass::Radar,
            launcher_id: 77,
            launcher_same_side: true,
            distance_at_launch_ft: 5000.0,
            launch_tick: 0,
        }));
        let mut c = controller(Experience::Ace);
        let batch = c.step(&scene.frame(1200, own())).unwrap();
        assert_eq!(batch.reason, None);
        assert!(batch.launch_calls.is_empty(), "same side: no launch call");
    }

    #[test]
    fn target_selection_survives_and_clears() {
        let scene = Scene::new();
        let mut c = controller(Experience::Average);
        let batch = c.step(&scene.frame(0, own())).unwrap();
        assert_eq!(batch.sensor.designate, Some(2));
        assert_eq!(c.target(), Some(2));

        // An empty world clears it.
        let empty = Scene {
            targets: Vec::new(),
            events: Vec::new(),
            stations: scene.stations.clone(),
            dispensers: scene.dispensers.clone(),
        };
        let batch = c.step(&empty.frame(1, own())).unwrap();
        assert_eq!(batch.sensor.designate, None);
        assert!(batch.sensor.clear_designation);
        assert_eq!(c.target(), None);
    }

    #[test]
    fn a_same_side_candidate_is_never_selected() {
        let mut scene = Scene::new();
        scene.targets[0].side = targeting::Side(1);
        let mut c = controller(Experience::Ace);
        let batch = c.step(&scene.frame(0, own())).unwrap();
        assert_eq!(batch.sensor.designate, None);
    }

    #[test]
    fn a_removed_actor_stops_being_the_target() {
        let mut scene = Scene::new();
        let mut c = controller(Experience::Ace);
        c.step(&scene.frame(0, own())).unwrap();
        assert_eq!(c.target(), Some(2));
        scene.events.push(FrameEvent::ActorRemoved(2));
        scene.targets.clear();
        c.step(&scene.frame(1, own())).unwrap();
        assert_eq!(c.target(), None);
    }

    #[test]
    fn an_unarmed_aircraft_still_maneuvers() {
        let mut scene = Scene::new();
        scene.stations.clear();
        let mut c = controller(Experience::Experienced);
        for tick in 0..600 {
            let batch = c.step(&scene.frame(tick, own())).unwrap();
            assert!(batch.motion.is_some());
            assert!(batch.weapons.is_empty());
        }
    }

    #[test]
    fn an_empty_store_never_fires() {
        let mut scene = Scene::new();
        scene.stations[0].rounds = weapon_service::Rounds::Finite(0);
        let mut c = controller(Experience::Ace);
        for tick in 0..2400 {
            let batch = c.step(&scene.frame(tick, own())).unwrap();
            assert!(batch.weapons.is_empty(), "fired with no rounds at {tick}");
        }
    }

    #[test]
    fn an_inhibited_station_never_fires() {
        let mut scene = Scene::new();
        scene.stations[0].inhibited = true;
        let mut c = controller(Experience::Ace);
        for tick in 0..2400 {
            assert!(
                c.step(&scene.frame(tick, own()))
                    .unwrap()
                    .weapons
                    .is_empty()
            );
        }
    }

    #[test]
    fn fire_requests_carry_unique_identities() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let mut ids = Vec::new();
        for tick in 0..12_000 {
            for intent in c.step(&scene.frame(tick, own())).unwrap().weapons {
                assert!(
                    !ids.contains(&intent.request.request_id),
                    "duplicate request id"
                );
                ids.push(intent.request.request_id);
            }
        }
    }

    #[test]
    fn bingo_fuel_sends_the_actor_home_and_records_the_leader_fallback() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let mut state = own();
        // Endurance under time-to-home plus five minutes is bingo.
        state.time_home_s = Some(600.0);
        state.fuel_endurance_s = 700.0;
        let batch = c.step(&scene.frame(0, state)).unwrap();
        assert_eq!(batch.fuel_state, Some(FuelState::Bingo));
        assert_eq!(c.fuel_state(), Some(FuelState::Bingo), "the radio reads it");
        assert_eq!(batch.activity, Some(Activity::ReturningToBase));
        assert!(batch.fallbacks.contains(&Fallback::LeaderReturnToBase));
    }

    #[test]
    fn zero_internal_fuel_is_reported_as_out_of_fuel() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let mut state = own();
        state.internal_fuel_lbs = 0.0;
        let batch = c.step(&scene.frame(0, state)).unwrap();
        assert_eq!(batch.activity, Some(Activity::OutOfFuel));
    }

    #[test]
    fn a_wingman_has_a_formation_point_and_a_leader_does_not() {
        let scene = Scene::new();
        let mut leader = controller(Experience::Ace);
        let frame = scene.frame(0, own());
        assert_eq!(leader.formation_point(&frame).unwrap(), None);

        let mut wingman_identity = identity();
        wingman_identity.member = 1;
        wingman_identity.leads = false;
        let mut wingman =
            Controller::new(wingman_identity, profile(), resolved(Experience::Ace), 5).unwrap();
        let mut view = wing_view();
        view.leader = Some(LeaderView {
            velocity: [0., 0., 800.],
            position: [0.0, 20000.0, 0.0],
            heading_deg: 0.0,
            speed: ScalarSpeed(800.0),
            target: None,
            recovering: false,
            on_ground: false,
        });
        let framed = DecisionFrame {
            tick: 0,
            own: own(),
            targets: &scene.targets,
            events: &scene.events,
            stations: &scene.stations,
            dispensers: &scene.dispensers,
            wing: view,
            route: RouteView {
                home_airport: None,
                leader_is_ai: true,
            },
            now: TimeOfDay(0),
            flight_state: FlightState::Free,
        };
        let point = wingman.formation_point(&framed).unwrap().unwrap();
        // Echelon slot 1 is one spacing right and one back of the leader.
        assert!(point[0] > 0.0, "{point:?}");
        assert!(point[2] < 0.0, "{point:?}");
    }

    #[test]
    fn every_ported_aircraft_builds_a_controller_at_every_level() {
        for aircraft in AircraftId::ALL {
            for level in Experience::ALL {
                let mut id = identity();
                id.aircraft = aircraft;
                let built = Controller::new(id, profile(), resolved(level), 9);
                assert!(built.is_ok(), "{aircraft:?} {level:?}");
            }
        }
    }

    #[test]
    fn every_ported_aircraft_flies_at_every_level() {
        let scene = Scene::new();
        for aircraft in AircraftId::ALL {
            for level in Experience::ALL {
                let mut id = identity();
                id.aircraft = aircraft;
                let mut c = Controller::new(id, profile(), resolved(level), 17).unwrap();
                for tick in 0..600 {
                    let batch = c.step(&scene.frame(tick, own())).unwrap();
                    assert!(batch.motion.is_some(), "{aircraft:?} {level:?} at {tick}");
                }
            }
        }
    }

    #[test]
    fn a_defence_flies_corner_speed_but_a_helicopters_flies_its_strike_speed() {
        let scene = Scene::new();
        let strike = super::super::helicopter_defense::strike_speed(&own().limits);
        assert!(strike.0 < own().limits.maximum.0 && strike.0 >= own().limits.minimum.0);
        for (rotorcraft, speed) in [(false, own().limits.corner), (true, strike)] {
            let mut c = controller(Experience::Average);
            c.set_defense_motion(Some(DefenseMotion {
                source: DefenseSource::Missile,
                heading_deg: 90.,
                pitch_deg: -4.,
                rotorcraft,
                extend: false,
            }));
            let motion = c.step(&scene.frame(0, own())).unwrap().motion.unwrap();
            assert_eq!(motion.speed, speed, "rotorcraft {rotorcraft}");
            assert_eq!(motion.heading_deg, 90.);
            assert_eq!(motion.flight_path_pitch_deg, -4.);
            assert!(c.defending());
        }
    }

    #[test]
    fn a_break_order_installs_motion_and_replaces_the_active_maneuver() {
        let scene = Scene::new();
        let mut c = controller(Experience::Average);
        let before = c.step(&scene.frame(0, own())).unwrap().motion.unwrap();
        let outcome = c
            .receive_order(
                WingRequest::Break {
                    heading_offset_deg: 170,
                    pitch_deg: 0,
                },
                1,
            )
            .unwrap();
        assert!(matches!(outcome, ReceiverOutcome::MotionInstalled(_)));
        let after = c.step(&scene.frame(1, own())).unwrap().motion.unwrap();
        assert_ne!(
            before.id, after.id,
            "the break did not replace the maneuver"
        );
        // B46: a break is a nominal five second request at corner speed.
        assert_eq!(after.speed, own().limits.corner);
    }

    #[test]
    fn the_player_break_values_reach_the_receiver() {
        let scene = Scene::new();
        for order in wing::PlayerBreak::ALL {
            let mut c = controller(Experience::Ace);
            c.step(&scene.frame(0, own())).unwrap();
            let outcome = c.receive_order(order.request(), 1).unwrap();
            assert!(
                matches!(outcome, ReceiverOutcome::MotionInstalled(_)),
                "{order:?} installed no motion"
            );
        }
    }

    #[test]
    fn a_formation_order_applies_its_setting() {
        let mut c = controller(Experience::Ace);
        let outcome = c
            .receive_order(WingRequest::FormationSelection(Formation::LineAstern), 0)
            .unwrap();
        assert!(matches!(
            outcome,
            ReceiverOutcome::Applied(AppliedSetting::FormationSelection { .. })
        ));
        assert_eq!(c.recipient.formation, Some(Formation::LineAstern));
    }

    #[test]
    fn an_engage_order_assigns_the_named_target() {
        let scene = Scene::new();
        let mut c = controller(Experience::Ace);
        let outcome = c
            .receive_order(
                WingRequest::TargetAssignment(wing::TargetOrder::ConcreteTarget(WingTargetId(2))),
                0,
            )
            .unwrap();
        assert!(matches!(
            outcome,
            ReceiverOutcome::Applied(AppliedSetting::TargetOrder { .. })
        ));
        assert_eq!(c.target(), Some(2));
        // B46 establishes a nominal 20 second target deadline.
        assert!(c.recipient.target_deadline.is_some());
        c.step(&scene.frame(1, own())).unwrap();
    }

    #[test]
    fn a_spacing_order_applies_without_installing_motion() {
        let mut c = controller(Experience::Ace);
        let outcome = c
            .receive_order(
                WingRequest::Spacing {
                    axis: wing::SpacingAxis::Horizontal,
                    feet: 5000,
                },
                0,
            )
            .unwrap();
        assert!(matches!(
            outcome,
            ReceiverOutcome::Applied(AppliedSetting::Spacing { .. })
        ));
        assert_eq!(c.recipient.horizontal_spacing_ft, Some(5000));
    }

    #[test]
    fn the_fallback_log_counts_and_lists_correctly() {
        let mut log = FallbackLog::default();
        assert_eq!(log.total(), 0);
        assert!(log.applied().is_empty());
        log.record(Fallback::EngagementPitch);
        log.record(Fallback::EngagementPitch);
        log.record(Fallback::HitChance);
        assert_eq!(log.count(Fallback::EngagementPitch), 2);
        assert_eq!(log.count(Fallback::HitChance), 1);
        assert_eq!(log.count(Fallback::BurstPacing), 0);
        assert_eq!(log.total(), 3);
        assert_eq!(log.applied().len(), 2);
    }

    #[test]
    fn angle_difference_takes_the_short_way_round() {
        assert!((angle_difference(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert!((angle_difference(10.0, 350.0) + 20.0).abs() < 1e-9);
        assert!((angle_difference(0.0, 180.0) - 180.0).abs() < 1e-9);
        assert!(angle_difference(0.0, 0.0).abs() < 1e-9);
    }

    #[test]
    fn activity_labels_are_distinct_and_non_empty() {
        let all = [
            Activity::Idle,
            Activity::Formation,
            Activity::Searching,
            Activity::Acquiring,
            Activity::Pursuing,
            Activity::Attacking,
            Activity::Defending,
            Activity::Evading,
            Activity::Breaking,
            Activity::Rejoining,
            Activity::ReturningToBase,
            Activity::OutOfFuel,
            Activity::Destroyed,
        ];
        let mut labels: Vec<&str> = all.iter().map(|a| a.label()).collect();
        assert!(labels.iter().all(|l| !l.is_empty()));
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before);
    }

    #[test]
    fn remembered_contact_searches_frozen_point_without_targeting_or_firing() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Average);
        c.set_search_contact(Some(SearchContact {
            id: 91,
            position: [0.0, 20_000.0, 10_000.0],
            observed_tick: 0,
        }));

        let output = c.step(&scene.frame(1, own())).unwrap();
        assert_eq!(output.activity, Some(Activity::Searching));
        assert_eq!(output.sensor.designate, None);
        assert_eq!(c.target(), None);
        assert!(output.weapons.is_empty());
        let motion = output.motion.unwrap();
        assert_eq!(motion.steering_point, Some([0.0, 20_000.0, 10_000.0]));
        assert!(motion.heading_deg.abs() < 1e-9);
    }

    #[test]
    fn air_to_air_leader_without_a_contact_reports_mission_search() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let output = controller(Experience::Average)
            .step(&scene.frame(1, own()))
            .unwrap();
        assert_eq!(output.activity, Some(Activity::Searching));
        assert!(output.motion.is_some());
    }

    #[test]
    fn search_orbit_is_level_bounded_and_does_not_restart_each_tick() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Average);
        let contact = SearchContact {
            id: 91,
            position: [0.0, 20_000.0, SEARCH_ORBIT_RADIUS_FT],
            observed_tick: 0,
        };
        c.set_search_contact(Some(contact));
        let first = c.step(&scene.frame(1, own())).unwrap().motion.unwrap();
        c.set_search_contact(Some(contact));
        let second = c.step(&scene.frame(2, own())).unwrap().motion.unwrap();

        assert_eq!(first.id, second.id);
        assert!((first.heading_deg - 270.0).abs() < 1e-9);
        assert!(first.flight_path_pitch_deg.abs() < 1e-9);
        assert_eq!(first.steering_point, None);

        let mut displaced = own();
        displaced.position[1] += 100.0;
        let correcting = c.step(&scene.frame(3, displaced)).unwrap().motion.unwrap();
        assert!(correcting.flight_path_pitch_deg < 0.0);
        assert!(correcting.flight_path_pitch_deg >= -SEARCH_ORBIT_MAX_CORRECTION_DEG);
    }

    #[test]
    fn level_search_orbit_radius_ignores_the_lost_targets_altitude() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Average);
        c.set_search_contact(Some(SearchContact {
            id: 91,
            position: [0.0, 21_000.0, SEARCH_ORBIT_RADIUS_FT],
            observed_tick: 0,
        }));
        let motion = c.step(&scene.frame(1, own())).unwrap().motion.unwrap();
        assert!((motion.heading_deg - 270.0).abs() < 1e-9);
        assert!(motion.flight_path_pitch_deg.abs() < 1e-9);
    }

    #[test]
    fn clockwise_search_orbit_has_the_expected_cardinal_tangents() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let cases = [
            ([0.0, 20_000.0, SEARCH_ORBIT_RADIUS_FT], 270.0),
            ([SEARCH_ORBIT_RADIUS_FT, 20_000.0, 0.0], 0.0),
            ([0.0, 20_000.0, -SEARCH_ORBIT_RADIUS_FT], 90.0),
            ([-SEARCH_ORBIT_RADIUS_FT, 20_000.0, 0.0], 180.0),
        ];
        for (position, expected_heading) in cases {
            let mut c = controller(Experience::Average);
            c.set_search_contact(Some(SearchContact {
                id: 91,
                position,
                observed_tick: 0,
            }));
            let heading = c
                .step(&scene.frame(1, own()))
                .unwrap()
                .motion
                .unwrap()
                .heading_deg;
            assert!((heading - expected_heading).abs() < 1e-9);
        }
    }

    #[test]
    fn ace_search_cap_rejoins_without_discarding_the_remembered_contact() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Ace);
        let contact = SearchContact {
            id: 91,
            position: [0.0, 20_000.0, 10_000.0],
            observed_tick: 0,
        };
        c.set_search_contact(Some(contact));
        assert_eq!(
            c.step(&scene.frame(1, own())).unwrap().activity,
            Some(Activity::Searching)
        );
        c.set_search_contact(Some(contact));
        let capped = c
            .step(&scene.frame(1 + ACE_SEARCH_LIMIT_TICKS, own()))
            .unwrap();
        assert_eq!(capped.activity, Some(Activity::Rejoining));
        assert!(capped.motion.is_none());
        assert_eq!(c.search_contact, Some(contact));
        let resumed = c
            .step(&scene.frame(2 + ACE_SEARCH_LIMIT_TICKS, own()))
            .unwrap();
        assert_eq!(resumed.activity, Some(Activity::Searching));
        assert!(resumed.motion.is_some());
    }

    #[test]
    fn live_contact_preempts_search_and_without_a_solution_is_acquiring() {
        let mut scene = Scene::new();
        scene.targets.clear();
        scene.targets.push(enemy(22, [0.0, 20_000.0, -10_000.0]));
        let mut c = controller(Experience::Average);
        c.set_search_contact(Some(SearchContact {
            id: 91,
            position: [0.0, 20_000.0, 10_000.0],
            observed_tick: 0,
        }));
        let output = c.step(&scene.frame(1, own())).unwrap();
        assert_eq!(c.target(), Some(22));
        assert_eq!(output.activity, Some(Activity::Acquiring));
        assert!(output.weapons.is_empty());
    }

    #[test]
    fn explicit_break_preempts_a_remembered_contact_search() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Average);
        c.set_search_contact(Some(SearchContact {
            id: 91,
            position: [0.0, 20_000.0, 10_000.0],
            observed_tick: 0,
        }));
        c.receive_order(
            WingRequest::Break {
                heading_offset_deg: 170,
                pitch_deg: 0,
            },
            0,
        )
        .unwrap();

        let output = c.step(&scene.frame(1, own())).unwrap();
        assert_eq!(output.motion.unwrap().heading_deg, 170.0);
    }

    #[test]
    fn motion_order_interrupt_restarts_the_ace_investigation_clock() {
        let mut scene = Scene::new();
        scene.targets.clear();
        let mut c = controller(Experience::Ace);
        c.set_search_contact(Some(SearchContact {
            id: 91,
            position: [0.0, 20_000.0, 10_000.0],
            observed_tick: 0,
        }));
        assert_eq!(
            c.step(&scene.frame(1, own())).unwrap().activity,
            Some(Activity::Searching)
        );
        c.receive_order(
            WingRequest::Break {
                heading_offset_deg: 170,
                pitch_deg: 0,
            },
            2,
        )
        .unwrap();

        assert_eq!(
            c.step(&scene.frame(603, own())).unwrap().activity,
            Some(Activity::Searching)
        );
        assert_eq!(
            c.step(&scene.frame(1 + ACE_SEARCH_LIMIT_TICKS, own()))
                .unwrap()
                .activity,
            Some(Activity::Searching)
        );
    }

    #[test]
    fn pursuit_tracks_lateral_and_moving_targets_without_restarting_or_redirecting_breaks() {
        let mut scene = Scene::new();
        scene.targets[0].position = [20000.0, 20000.0, 20000.0];
        let mut c = controller(Experience::Ace);
        let first = c.step(&scene.frame(0, own())).unwrap().motion.unwrap();
        assert!(
            (first.heading_deg - 45.0).abs() < 10.0,
            "{}",
            first.heading_deg
        );
        scene.targets[0].position[0] = -20000.0;
        let next = c.step(&scene.frame(1, own())).unwrap().motion.unwrap();
        assert_eq!(first.id, next.id);
        assert!(next.heading_deg > 300.0);
        c.receive_order(wing::PlayerBreak::Right.request(), 2)
            .unwrap();
        let ordered = c.step(&scene.frame(2, own())).unwrap().motion.unwrap();
        assert_eq!(ordered.heading_deg, 170.0);
        assert!(ordered.steering_point.is_none());
    }

    #[test]
    fn all_reason_priority_pairs_preserve_or_replace_active_motion() {
        let scene = Scene::new();
        for saved in ScriptReason::ALL {
            for new in ScriptReason::ALL {
                let mut c = controller(Experience::Ace);
                c.step(&scene.frame(0, own())).unwrap();
                c.reason = Some(saved);
                let active = c.active;
                let reason = c.accept_reason(new);
                if new > saved {
                    assert_eq!(reason, Some(new));
                    assert!(c.active.is_none());
                } else {
                    assert_eq!(reason, None);
                    assert_eq!(c.active, active);
                }
            }
        }
    }

    #[test]
    fn fresh_warnings_are_queued_until_due_once_at_all_levels_and_distance_edges() {
        for level in Experience::ALL {
            for distance in [0.0, 10559.0, 10560.0, 211200.0, 211201.0] {
                let mut scene = Scene::new();
                scene.targets.clear();
                let report = ThreatReport {
                    missile_id: 42,
                    seeker: SeekerClass::Infrared,
                    launcher_id: 99,
                    launcher_same_side: false,
                    distance_at_launch_ft: distance,
                    launch_tick: 0,
                };
                scene.events.push(FrameEvent::ThreatReported(report));
                let mut c = controller(level);
                c.step(&scene.frame(0, own())).unwrap();
                let WarningDelay::Quarters(q) = threat::warning_delay(
                    &WarningTarget::Ai {
                        experience: level,
                        state: AttackState::OrdinaryFlight,
                    },
                    distance,
                )
                .unwrap() else {
                    panic!()
                };
                assert_eq!(c.pending_warnings, vec![(u64::from(q) * 30, report)]);
                scene.events.clear();
                let early = c.step(&scene.frame(u64::from(q) * 30 - 1, own())).unwrap();
                assert!(early.devices.is_none());
                assert_eq!(c.pending_warnings.len(), 1);
                c.step(&scene.frame(u64::from(q) * 30, own())).unwrap();
                assert!(c.pending_warnings.is_empty());
                assert!(
                    c.step(&scene.frame(u64::from(q) * 30 + 1, own()))
                        .unwrap()
                        .devices
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn selected_store_requires_angle_range_support_and_clear_terrain() {
        for gate in 0..6 {
            let mut scene = Scene::new();
            match gate {
                0 => scene.targets[0].position = [8000.0 * 3f64.sqrt(), 20000.0, 8000.0],
                1 => scene.stations[0].maximum_range_ft = Some(7999.0),
                2 => scene.stations[0].minimum_range_ft = 8001.0,
                3 => scene.targets[0].terrain_blocked = true,
                4 => scene.stations[0].requires_radar = true,
                _ => {
                    scene.stations[0].requires_sensor = true;
                    scene.targets[0].sensor_supported = false;
                }
            }
            let mut own = own();
            if gate == 4 {
                own.radar_emitting = false;
            }
            let mut c = controller(Experience::Ace);
            for tick in 0..2400 {
                assert!(
                    c.step(&scene.frame(tick, own)).unwrap().weapons.is_empty(),
                    "gate {gate}"
                );
            }
        }
    }
    #[test]
    fn approach_uses_distance_completion_and_disengage_cancels_it() {
        let scene = Scene::new();
        let mut c = controller(Experience::Average);
        c.step(&scene.frame(0, own())).unwrap();
        let id = scene.frame(1, own()).targets[0].id;
        c.receive_order(
            WingRequest::TargetAssignment(wing::TargetOrder::ConcreteTarget(WingTargetId(id))),
            1,
        )
        .unwrap();
        c.receive_order(
            WingRequest::Approach {
                heading_deg: 0,
                pitch_deg: 0,
                speed: ScalarSpeed(0.),
            },
            1,
        )
        .unwrap();
        c.track_ordered_approach(id, 45., 0.);
        let frame = scene.frame(1, own());
        let result = c.step(&frame).unwrap();
        assert!(result.motion.is_some());
        if distance(frame.own.position, frame.targets[0].position) > 2000. {
            assert!(c.ordered_approach.is_some());
        }
        c.receive_order(
            WingRequest::TargetAssignment(wing::TargetOrder::HoldFire),
            2,
        )
        .unwrap();
        assert!(c.ordered_approach.is_none());
        assert!(c.target().is_none());
        assert!(c.recipient.target.is_none());
        assert!(c.recipient.target_deadline.is_none());
    }
    #[test]
    fn ordered_approach_survives_heading_alignment_and_finishes_at_2000_feet() {
        let mut scene = Scene::new();
        let mut c = controller(Experience::Average);
        c.step(&scene.frame(0, own())).unwrap();
        c.receive_order(
            WingRequest::Approach {
                heading_deg: 0,
                pitch_deg: 0,
                speed: ScalarSpeed(0.),
            },
            1,
        )
        .unwrap();
        let mut physical = own();
        physical.heading_deg = 0.;
        physical.position = [0., 20000., 5999.];
        let output = c.step(&scene.frame(1, physical)).unwrap();
        assert!(
            c.ordered_approach.is_some(),
            "aligned heading is not arrival"
        );
        assert_eq!(output.activity, Some(Activity::Pursuing));
        physical.position[2] = 6000.;
        c.step(&scene.frame(2, physical)).unwrap();
        assert!(c.ordered_approach.is_none(), "2000 ft is inclusive");
        c.receive_order(
            WingRequest::Approach {
                heading_deg: 45,
                pitch_deg: 0,
                speed: ScalarSpeed(0.),
            },
            3,
        )
        .unwrap();
        physical.position[2] = -2000.;
        let output = c.step(&scene.frame(3, physical)).unwrap();
        assert!((output.motion.unwrap().heading_deg - 45.).abs() < 1e-8);
        scene.targets.clear();
        let output = c.step(&scene.frame(4, physical)).unwrap();
        assert!(c.ordered_approach.is_none());
        assert!(c.target().is_none());
        assert!(
            output.weapons.is_empty(),
            "lost approach target cannot trigger opportunistic fire"
        );
    }
    #[test]
    fn live_formation_variation_is_smooth_and_vertical_target_stays_within_five_feet() {
        let mut id = identity();
        id.member = 1;
        id.leads = false;
        let mut c = Controller::new(id, profile(), resolved(Experience::Average), 5).unwrap();
        let scene = Scene::new();
        let mut frame = scene.frame(0, own());
        frame.wing.leader = Some(LeaderView {
            position: [0., 20000., 0.],
            velocity: [0., 0., 800.],
            heading_deg: 0.,
            speed: ScalarSpeed(800.),
            target: None,
            recovering: false,
            on_ground: false,
        });
        frame.wing.vertical_spacing_ft = 0;
        let mut previous = [0.; 3];
        for tick in 0..14400 {
            frame.tick = tick;
            let point = c.formation_point(&frame).unwrap().unwrap();
            assert!((point[1] - 20000.).abs() <= 5.);
            for (i, limit) in [0.09, 0.3, 0.03].into_iter().enumerate() {
                assert!((c.smooth_variation[i] - previous[i]).abs() < limit);
            }
            previous = c.smooth_variation;
            assert_eq!(
                c.formation_point(&frame).unwrap().unwrap(),
                point,
                "repeated same-tick queries cannot advance the filter"
            );
        }
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "controller_checkpoint.rs"]
mod checkpoint;
