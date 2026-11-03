//! Per-actor AI runtime (AI-5): actor-owned sensors, stores and movement.
//!
//! This module is the seam between the decision components and the rest of the
//! simulation. Each [`AiActor`] owns its own [`flight::State`], its own
//! [`Sensors`], its own stores and its own [`Controller`]. Nothing here reads
//! the player's combat state, the player's sensors or any shared global, and
//! motion goes exclusively through the actor's own flight model using the
//! inputs from [`steering_adapter`](super::steering_adapter).
//!
//! **Missile physics are not duplicated.** AI-5's exit evidence forbids it. A
//! weapon release produces a [`LaunchEvent`] after this module has debited the
//! actor's own ammunition through
//! [`weapon_service::release`](super::weapon_service::release), except live guns:
//! those authorize one physical round and the host debits immediately before
//! emission, after capacity and barrel checks. The host's
//! existing combat layer creates and flies the projectile with the same code
//! the player's shots use. In the other direction the host reports launches
//! back as [`controller::ThreatReport`]s, which is what B47 warnings are built
//! from. An empty or inhibited store refuses both release paths.
//!
//! Frames are built from one world snapshot per tick. An actor's permitted
//! targets are the objects its own sensors can see, never the whole world, so
//! one actor cannot acquire a contact another actor earned.

use tore_formats::aircraft::AircraftId;
use tore_input::PilotInput;

use super::{defense, engagement, incoming_fire};
use crate::combat::threats::{MissileSnapshot, Receiver, ThreatRecord, ThreatService};
use crate::flight;
use crate::models::FlightModel;
use crate::research::Surface;
use crate::sensors::{self, Observable, Observer, Sensors};

use super::awareness::{self, Memory};
#[cfg(test)]
use super::awareness::{Observation, ObservationSource};
use super::controller::{
    Activity, ActorIdentity, BehaviorProfile, Controller, DecisionFrame, FrameEvent, IntentBatch,
    LeaderView, MotionIntent, OwnState, RouteView, SearchContact, StationView, TargetView,
    ThreatReport, WingView,
};
use super::experience::ResolvedExperience;
use super::fitted::Fallback;
use super::steering_adapter::{ControlAdapter, ai_g_limits};
use super::test_settings::AiTestSettings;
use super::thought::{
    self, ActorPath, ActorTrace, AirfieldExit, DropReason, EjectionTrace, EngagementTrace,
    ExpiryReason, FlyTrace, IgnoreReason, JournalEntry, Message, Outcome, Receipt, RejoinReason,
    RejoinTrace, ReportTrace, WarningStep,
};
use super::threat::{DispenserStore, FlightState, SeekerClass, TimeOfDay};
use super::weapon_service::{
    self, Delay, ProjectilePacing, Rounds, StationId, StoreCapability, StoreState,
};
use super::wing::{Formation, WingControl};
use super::{Result, ScalarSpeed, SpeedLimits};

#[path = "mission_observation.rs"]
mod observation;
#[path = "mission_strike.rs"]
mod strike_step;

// What an AI lead does with the flight data link (stage G, slice G4).
#[path = "mission_leads.rs"]
mod leads;

#[cfg(test)]
#[path = "mission_observation_tests.rs"]
mod observation_tests;

#[cfg(test)]
#[path = "mission_test_settings_tests.rs"]
mod test_settings_tests;

#[cfg(test)]
#[path = "link_tests.rs"]
mod link_tests;

#[cfg(test)]
#[path = "mission_leads_tests.rs"]
mod leads_tests;

// The lead hold's claims, stand-ins and re-forms (slice R2 of the lobby pass).
#[cfg(test)]
#[path = "mission_lead_hold_tests.rs"]
mod lead_hold_tests;

/// Fitted dispatch floor: smaller missions keep their observation state in
/// place. The worker-count comparison probe covers preparation and joining.
const MIN_PARALLEL_OBSERVATIONS: usize = 4;

/// One carried store on an AI aircraft.
///
/// This is the actor's own inventory. It is never shared with the player's
/// stores and never refilled by this module.
#[derive(Clone, Debug, PartialEq)]
pub struct StationSpec {
    pub station: StationId,
    pub guided: bool,
    pub capability: StoreCapability,
    pub store: StoreState,
    /// Rounds debited per release (B45 actual-rounds-per-game-round).
    pub debit: u32,
    /// External mass per remaining round, using the shared live payload convention.
    pub external_round_lbs: f64,
    /// Projectiles one release creates; pod and burst metadata can make this
    /// differ from the ammunition debit (B45).
    pub projectile_count: u32,
    /// The employment envelope's angular limit, when it has one.
    pub employment_limit_deg: Option<f64>,
    pub damage_vs_category: f64,
    pub store_speed: ScalarSpeed,
    pub tracking_delay: Delay,
    pub pacing: ProjectilePacing,
    /// Maximum employment range, feet. `None` leaves the bound unrestricted.
    pub maximum_range_ft: Option<f64>,
    /// Energy-model permission for this target; Some(None) is pending or unreachable.
    pub energy_target: Option<Option<u32>>,
    pub minimum_range_ft: f64,
    /// All-aspect flag and nominal IR seeker reach, absent for other stores.
    pub infrared_envelope: Option<(bool, f64)>,
    pub requires_radar: bool,
    pub requires_sensor: bool,
    pub employment_zone: Option<tore_formats::weapons::Zone>,
    pub mount: [f64; 3],
}

impl StationSpec {
    pub fn rounds(&self) -> Rounds {
        self.store.rounds
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.store.rounds, Rounds::Finite(0))
    }
}

/// One object in the world, as the mission sees it.
///
/// The host builds these once per tick for every participant, the player
/// included. The player appears here as an ordinary object with
/// `human_controlled` set; it is never a special case in the decision path.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldObject {
    /// Sensor input only; copied into a target view when the object is observed.
    pub infrared_strength: f64,
    pub id: u32,
    pub side: super::targeting::Side,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub heading_deg: f64,
    pub pitch_deg: f64,
    pub speed: ScalarSpeed,
    pub maximum_speed: ScalarSpeed,
    pub is_aircraft: bool,
    pub is_fighter: bool,
    pub human_controlled: bool,
    pub alive: bool,
    pub destroyed: bool,
    /// Supported by the ground (wheels on a runway), not flying.
    pub on_ground: bool,
    /// Sensor input for the actors that can see it. `None` means the object is
    /// not presented to the sensor component at all.
    pub observable: Option<Observable>,
}

/// A weapon release this tick, for the host's existing combat layer to realise.
///
/// The ammunition has already been debited from the firing actor's own store,
/// so the host must not debit again. `projectiles` is what the host should
/// create; it is not necessarily the ammunition debit (B45).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchEvent {
    pub actor: u32,
    pub station: StationId,
    pub target: u32,
    pub request_id: weapon_service::RequestId,
    pub projectiles: u32,
}

/// A countermeasure release this tick (B47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceEvent {
    pub actor: u32,
    pub class: SeekerClass,
    /// Devices actually released after the dispenser debit, which stops as
    /// soon as the matching dispenser is empty.
    pub released: u8,
}

/// What one tick of the mission produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissionOutput {
    pub jettison: Vec<(u32, super::jettison::Drop)>,
    pub launches: Vec<LaunchEvent>,
    pub devices: Vec<DeviceEvent>,
    /// Player-readable activity per actor, for a radio or status display.
    pub activities: Vec<(u32, Activity)>,
    /// Every fitted fallback applied this tick, by actor.
    pub fallbacks: Vec<(u32, Fallback)>,
    pub wing: Vec<(u32, super::wing::WingRequest)>,
    /// Accepted opposite-side launch warnings as (actor, launcher), for the
    /// radio's "SAM launch" and "AAM launch" calls.
    pub launch_calls: Vec<(u32, u32)>,
    /// Wings whose lead passed to another aircraft this tick, in the order
    /// it happened.
    pub leadership: Vec<LeadershipChange>,
    /// What the AI leads did with the flight data link this tick: targets
    /// shared and sorted, and bandits left alone (slice G4). The world
    /// records the assignments and voices them.
    pub link: Vec<super::link::LinkEvent>,
}

/// A human-flown aircraft in a wing, which the leadership rule needs beside
/// the AI's own actors. The host hands the current set to
/// [`AiMission::set_humans`] every step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HumanMember {
    /// The aircraft id, also its id in the world snapshot.
    pub id: u32,
    pub side: super::targeting::Side,
    pub wing: u8,
    /// Roster place inside the wing, from 0; the first member leads at the
    /// start.
    pub member: u8,
    /// The pilot survives the loss of the aircraft (ejected and not hurt);
    /// read when this aircraft stops leading.
    pub pilot_alive: bool,
}

/// Lead of one wing passed on: the aircraft that was destroyed or whose pilot
/// ejected, and the one that leads now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeadershipChange {
    pub tick: u64,
    pub side: super::targeting::Side,
    pub wing: u8,
    pub leader: u32,
    pub previous: u32,
    /// The previous leader's pilot is alive, for example after ejecting.
    pub previous_pilot_alive: bool,
    /// The new leader stands in for the wing's owner, a human whose claim
    /// does not fly now (the lead hold, slice R2 of the lobby pass).
    pub acting: bool,
    /// The lead went back to the wing's owner, who led it before: a claimed
    /// plane crowned that was not crowned for the first time.
    pub reclaimed: bool,
}

/// The lead hold's claim on one wing (slice R2 of the lobby pass; John,
/// 2026-10-09): the human owner of the wing's lead keeps it while dead,
/// reviving or flying back. The host hands every owned wing's claim to
/// [`AiMission::set_lead_claims`] before each step; single player hands
/// none, which is today's succession rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeadClaim {
    pub side: super::targeting::Side,
    pub wing: u8,
    /// The owner's current plane in the wing, lost or flying; `None` when
    /// the owner has none there now. A claimed plane that flies and does not
    /// lead is crowned; any other leader is a stand-in.
    pub plane: Option<u32>,
    /// The owner has not led the wing yet, so crowning its plane makes it
    /// the new lead rather than giving the lead back.
    pub fresh: bool,
}

/// The aircraft that currently leads one wing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WingLeader {
    side: super::targeting::Side,
    wing: u8,
    leader: u32,
    /// Lead has passed on at least once, so the followers' slots are ranks in
    /// member order rather than the numbers they started with.
    reformed: bool,
    /// The leader stands in for the wing's owner under the lead hold.
    acting: bool,
}

/// The mission's airfield decisions for one actor this tick (see
/// docs/spec/ai-airfield.md). Read-only outside the mission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AirfieldClearance {
    /// Turn gate: every earlier wing member is past its first taxiway leg,
    /// and a human leader is airborne.
    pub turn: bool,
    /// Nothing else is using the runway and the player holds no landing
    /// priority there.
    pub runway_free: bool,
    /// Every earlier wing member landing here is on the ground.
    pub wing_landed: bool,
    /// The parking slot this aircraft may hold.
    pub free_slot: Option<u32>,
    /// A joining wingman's leader is landing or on the ground.
    pub leader_landing: bool,
}

/// Everything needed to build one AI aircraft.
pub struct ActorSetup {
    pub identity: ActorIdentity,
    pub profile: BehaviorProfile,
    pub experience: ResolvedExperience,
    pub seed: u64,
    pub flight: flight::State,
    /// The actor's own sensor component. `None` means the host supplies the
    /// permitted target list itself, which is the headless fixture path.
    pub sensors: Option<Sensors>,
    pub stations: Vec<StationSpec>,
    pub dispensers: Vec<DispenserStore>,
    pub wing_slot: u8,
    pub home_airport: Option<super::route::Position>,
}

/// What an AI aircraft leaves behind when it stops being AI-flown: the parts
/// of it that belong to the aircraft rather than to its pilot. A human
/// taking the aircraft over builds its cockpit from these; the AI's awareness,
/// orders and decisions are dropped.
pub struct ActorParts {
    pub identity: ActorIdentity,
    pub flight: flight::State,
    pub sensors: Option<Sensors>,
    pub stations: Vec<StationSpec>,
    pub dispensers: Vec<DispenserStore>,
    /// Its missile warnings.
    pub warnings: ThreatService,
    pub equipment: EquipmentFaults,
}

/// Perceived attack information shared with assigned escorts. The bearing is
/// world-relative, not an invented target position or fire-control solution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservedAttack {
    pub report: engagement::ThreatReport,
    pub bearing_world_deg: Option<f64>,
    pub observed_tick: u64,
    /// Observed projectile identity, never a hidden launcher identity.
    pub event_id: Option<u32>,
}

/// Damaged avionics on an AI aircraft, from system faults on its hardpoints.
/// A failed sensor sees nothing; a failed RWR gives no missile warning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EquipmentFaults {
    pub radar: bool,
    pub infrared: bool,
    pub visual: bool,
    pub rwr: bool,
}

/// One AI-flown aircraft and everything it owns.
#[cfg_attr(test, derive(Clone, Debug))]
pub struct AiActor {
    identity: ActorIdentity,
    controller: Controller,
    adapter: ControlAdapter,
    flight: flight::State,
    sensors: Option<Sensors>,
    awareness: Memory,
    search_target: Option<u32>,
    missile_threats: ThreatService,
    defense_state: defense::DefenseState,
    last_defense: Option<defense::DefenseDecision>,
    incoming_fire: incoming_fire::Service,
    lookout: Option<awareness::Lookout>,
    fire_defending: bool,
    assignment: engagement::Assignment,
    mission_policy: engagement::Policy,
    observed_attacks: Vec<ObservedAttack>,
    neutral: bool,
    formation_order_tick: Option<u64>,
    ignored_attack_ids: Vec<u32>,
    received_emitters: Vec<sensors::passive::Emitter>,
    equipment: EquipmentFaults,
    damage_return: Option<super::damage::Reason>,
    stations: Vec<StationSpec>,
    guns: std::collections::BTreeMap<u8, tore_formats::weapons::Weapon>,
    gun_target: Option<super::gunnery::Target>,
    dispensers: Vec<DispenserStore>,
    wing_slot: u8,
    home_airport: Option<super::route::Position>,
    /// The runway this aircraft returns to, when the host knows one.
    home_runway: Option<super::airfield::RunwayView>,
    /// Set when the aircraft begins the mission parked on a runway.
    ground_start: Option<super::airfield::GroundStart>,
    /// The landing this aircraft is flying or will resume, if any.
    landing_order: Option<super::airfield::LandingOrder>,
    /// Set once a bug-out order is accepted; the aircraft then ignores orders.
    bugged_out: bool,
    /// A join-the-leader landing was cancelled by order; it is not re-joined
    /// until the leader is no longer recovering (fitted, 2026-09-23).
    join_cancelled: bool,
    /// Traffic avoidance in force: the tick it may end and the heading it
    /// holds, in degrees.
    avoiding: Option<(u64, f64)>,
    /// The aircraft whose conflict set the held avoidance heading.
    avoiding_from: Option<u32>,
    /// The running takeoff or landing sequence, if any.
    airfield: Option<super::airfield::Sequence>,
    /// Draws for the private return route, separate from decision draws.
    route_random: super::DecisionRandom,
    pending_threats: Vec<ThreatReport>,
    pending_events: Vec<FrameEvent>,
    /// Due tick, class, count, and whether loss of the observed threat cancels it.
    device_schedule: Vec<(u64, SeekerClass, u8, bool)>,
    activity: Activity,
    last_input: PilotInput,
    alive: bool,
    dummy: bool,
    escape_monitor: crate::ejection::Monitor,
    /// Bandits this actor leaves alone for a while because a flightmate it
    /// was not sorted with holds them too (slice G4).
    yields: Vec<super::link::Yield>,
    /// The ground picture of a striker (the Strike role or an ordered ground
    /// target); `None` for every other actor (docs/spec/ai-strike.md).
    ground: Option<Box<super::awareness::ground::GroundPicture>>,
    /// The aircraft's decision seed, from its setup: a striker's skill draw
    /// reads it (docs/spec/ai-strike.md, "Skill and release precision").
    seed: u64,
    /// A striker's strike state (slice ST-T); `None` for every other actor.
    strike: Option<Box<super::strike::Strike>>,
    /// The ground target a flight lead ordered this wingman to lase ("Lase
    /// my target", slice ST-O). Slice ST-L flies the orbit and lases it;
    /// until it does the order only stands.
    lase: Option<u32>,
    /// The helicopters' and the V-22's control memory (slice H1): the
    /// rotor laws' integrators and regime. Default on other aircraft.
    rotor: super::rotor_adapter::RotorAdapter,
    /// The AV-8's and Yak-141's jetborne memory (slice H1), for the hovers
    /// the airfield sequences ask for. Default on other aircraft.
    jet_lift: super::jet_lift::JetLift,
    /// Write-only record of the latest mission step. No decision reads it.
    trace: ActorTrace,
    /// Write-only memory of what the journal already said about this actor.
    journal_memory: JournalMemory,
    /// The lookout's verdicts of the last observation, shown again in the
    /// trace of the ticks between observations (AI test settings). A
    /// why-record: no decision reads it and a checkpoint leaves it out.
    last_visual: Vec<awareness::VisualTrace>,
}

/// Places by id, sorted: of a repeated id the first place answers, as a
/// walk from the front finds it.
#[derive(Clone, Debug, Default)]
pub struct Places(Vec<(u32, usize)>);
impl Places {
    /// The places of `ids`, in order.
    pub fn of(ids: impl Iterator<Item = u32>) -> Self {
        let mut places: Vec<(u32, usize)> = ids.enumerate().map(|(at, id)| (id, at)).collect();
        // Sorted by id and then place, so the first of each id is its first
        // place.
        places.sort_unstable();
        places.dedup_by_key(|(id, _)| *id);
        Self(places)
    }
    /// The first place of `id`.
    pub fn get(&self, id: u32) -> Option<usize> {
        self.0
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|at| self.0[at].1)
    }
}

/// What the journal already said about one actor, so a state that holds for
/// many ticks is journaled once. Never read by a decision.
#[derive(Clone, Copy, Debug, Default)]
struct JournalMemory {
    /// The escort selection last journaled; `None` before the first.
    escort_selection: Option<Option<engagement::Selection>>,
}

impl AiActor {
    pub fn new(setup: ActorSetup) -> Result<Self> {
        let controller =
            Controller::new(setup.identity, setup.profile, setup.experience, setup.seed)?;
        Ok(Self {
            identity: setup.identity,
            controller,
            adapter: ControlAdapter::new(),
            flight: setup.flight,
            sensors: setup.sensors,
            awareness: Memory::new(setup.experience, setup.identity.side),
            search_target: None,
            missile_threats: ThreatService::new(setup.identity.actor.0),
            defense_state: defense::DefenseState::default(),
            last_defense: None,
            incoming_fire: incoming_fire::Service::default(),
            lookout: None,
            fire_defending: false,
            assignment: engagement::Assignment::default(),
            mission_policy: engagement::Policy::default(),
            observed_attacks: Vec::new(),
            // Standalone simulation fixtures supply authorized assignments.
            // Quick Mission explicitly initializes every wing as neutral.
            neutral: false,
            formation_order_tick: None,
            ignored_attack_ids: Vec::new(),
            received_emitters: Vec::new(),
            equipment: EquipmentFaults::default(),
            damage_return: None,
            stations: setup.stations,
            guns: Default::default(),
            gun_target: None,
            dispensers: setup.dispensers,
            wing_slot: setup.wing_slot,
            home_airport: setup.home_airport,
            home_runway: None,
            ground_start: None,
            landing_order: None,
            bugged_out: false,
            join_cancelled: false,
            avoiding: None,
            avoiding_from: None,
            airfield: None,
            route_random: super::DecisionRandom::seeded(setup.seed ^ 0x6c61_6e64_696e_6721),
            pending_threats: Vec::new(),
            pending_events: Vec::new(),
            device_schedule: Vec::new(),
            activity: Activity::Idle,
            last_input: PilotInput::default(),
            escape_monitor: crate::ejection::Monitor::seeded(
                setup.seed ^ u64::from(setup.identity.actor.0).wrapping_mul(0x9e3779b97f4a7c15),
            ),
            alive: true,
            dummy: false,
            yields: Vec::new(),
            ground: None,
            seed: setup.seed,
            strike: None,
            lase: None,
            rotor: super::rotor_adapter::RotorAdapter::new(),
            jet_lift: super::jet_lift::JetLift::new(),
            trace: ActorTrace::default(),
            journal_memory: JournalMemory::default(),
            last_visual: Vec::new(),
        })
    }

    /// What the mission decided for this actor on its latest step. A
    /// write-only record: no decision reads it. The controller's own record
    /// is [`Controller::trace`].
    pub fn trace(&self) -> &ActorTrace {
        &self.trace
    }

    /// Draws for the private return route on the latest step.
    pub fn route_draws(&self) -> &super::DrawLog {
        self.route_random.log()
    }

    /// Begin the mission parked on `start.runway`. The flight model must
    /// already be on the researched adapter and placed with
    /// `flight::State::start_on_runway`: the position read here picks the
    /// parking slot (with airport anchors) or the line-up spot (without).
    pub fn start_on_ground(&mut self, start: super::airfield::GroundStart) {
        self.home_runway = Some(start.runway);
        self.ground_start = Some(start);
        let position = self.flight.position;
        // With airport anchors a ground start sitting on a parking slot holds it.
        let slot = start.runway.anchors.and_then(|anchors| {
            (0..super::airfield::PARKING_SLOTS).find(|&k| {
                let p = anchors.parking[k as usize];
                (p[0] - position[0]).hypot(p[2] - position[2]) <= 100.0
            })
        });
        let lift = self.airfield_lift();
        self.airfield =
            Some(super::airfield::Sequence::departure(start, position, slot).with_lift(lift));
    }

    /// How this aircraft takes off and lands now (slice H2): a helicopter
    /// or the V-22 vertically, a vectoring jet vertically when its engines
    /// hold its weight at no more than
    /// [`super::airfield::HOVER_THROTTLE_LIMIT`] and by a short takeoff and
    /// a runway landing when heavier, any other aircraft on a runway.
    /// A vectoring jet's engines hold it, stores included, in a hover over
    /// ground at `ground_ft` at no more than
    /// [`super::airfield::HOVER_THROTTLE_LIMIT`].
    fn hover_capable(&self, ground_ft: f64) -> bool {
        super::jet_lift::hover_throttle(&self.flight, ground_ft)
            .is_some_and(|t| t <= super::airfield::HOVER_THROTTLE_LIMIT)
    }

    pub fn airfield_lift(&self) -> super::airfield::Lift {
        use super::airfield::Lift;
        if super::rotor_adapter::flies(&self.flight) {
            Lift::Rotor
        } else if super::jet_lift::flies(&self.flight) {
            if self.hover_capable(self.flight.position[1]) {
                Lift::Jet
            } else {
                Lift::ShortJet
            }
        } else {
            Lift::Conventional
        }
    }

    pub fn ground_start(&self) -> Option<&super::airfield::GroundStart> {
        self.ground_start.as_ref()
    }

    /// The runway used for return to base and bingo fuel.
    pub fn set_home_runway(&mut self, runway: Option<super::airfield::RunwayView>) {
        self.home_runway = runway;
    }

    pub fn home_runway(&self) -> Option<&super::airfield::RunwayView> {
        self.home_runway.as_ref()
    }

    /// The landing this aircraft is flying, or will resume after a missile
    /// defense interrupts its early approach.
    pub fn landing_order(&self) -> Option<&super::airfield::LandingOrder> {
        self.landing_order.as_ref()
    }

    /// True once a bug-out order was accepted. It stays true: the aircraft
    /// returns to base and no longer answers wing orders (manual p.160).
    pub fn bugged_out(&self) -> bool {
        self.bugged_out
    }

    /// The current airfield phase, `None` in free flight.
    pub fn airfield_phase(&self) -> Option<super::airfield::Phase> {
        self.airfield.as_ref().map(|s| s.phase())
    }

    /// The running takeoff or landing sequence, for diagnostics.
    pub fn airfield(&self) -> Option<&super::airfield::Sequence> {
        self.airfield.as_ref()
    }

    pub fn equipment(&self) -> EquipmentFaults {
        self.equipment
    }

    /// Record avionics the host found damaged. A failed radar also stops
    /// transmitting. Faults are permanent for the flight.
    pub fn fail_equipment(&mut self, faults: EquipmentFaults) {
        self.equipment.radar |= faults.radar;
        self.equipment.infrared |= faults.infrared;
        self.equipment.visual |= faults.visual;
        self.equipment.rwr |= faults.rwr;
        if self.equipment.radar {
            self.flight.radar = false;
        }
    }

    /// Opinionated training target requested on 2026-09-21.
    pub fn set_dummy(&mut self) {
        self.dummy = true;
        self.sensors = None;
        self.flight.radar = false;
        self.flight.pitch = 0.;
        self.flight.bank = 0.;
        self.flight.speed = super::launch::DUMMY_SPEED_FPS;
        self.flight.velocity = crate::attitude::Basis::new(self.flight.yaw, 0., 0.)
            .forward
            .map(|v| v * super::launch::DUMMY_SPEED_FPS);
    }

    /// The pilot's load-shedding picture without any fight in it: why it may
    /// be flying home and what it carries (docs/spec/jettison.md).
    fn load_situation(&self) -> super::jettison::Situation {
        super::jettison::Situation {
            skill: self.experience().level,
            strike: self.strikes(),
            missile_defense: false,
            defensive_break: false,
            engagement_ft: None,
            attacker_ft: None,
            damage_return: self.damage_return.is_some(),
            fuel_home: matches!(
                self.controller.fuel_state(),
                Some(
                    super::route::FuelState::Bingo
                        | super::route::FuelState::Critical
                        | super::route::FuelState::OutOfFuel
                )
            ),
            damage: self.flight.damage_fraction,
            tanks_dry: self.flight.systems.external_lbs() <= 0.,
        }
    }

    pub fn damage_return(&self) -> Option<super::damage::Reason> {
        self.damage_return
    }

    pub fn is_dummy(&self) -> bool {
        self.dummy
    }

    pub fn id(&self) -> u32 {
        self.identity.actor.0
    }

    pub fn identity(&self) -> &ActorIdentity {
        &self.identity
    }

    /// Opinionated (requested by John, 2026-09-29; numbers are agent
    /// decisions): an aircraft flying on its own (not in a formation
    /// procedure, not defending against a missile or gunfire, not in an
    /// airfield sequence) predicts its closest approach to every other
    /// airborne aircraft, friend or foe, over [`AVOID_HORIZON_S`]. When one
    /// comes inside [`AVOID_SEPARATION_FT`] plus [`AVOID_CLOSURE_S`] of the
    /// closing speed, it takes up a heading [`AVOID_TURN_DEG`] away from the
    /// other aircraft (to the right when meeting it head-on), keeping its
    /// pitch, and holds that heading until [`AVOID_HOLD_TICKS`] after the last
    /// predicted conflict so it does not flap. Nothing changes while no conflict is predicted.
    fn avoid_traffic(
        &mut self,
        tick: u64,
        mut intent: MotionIntent,
        world: &[WorldObject],
    ) -> MotionIntent {
        let formation = self.controller.formation_trace().is_some();
        let defending = self.defense_flying();
        if formation || defending || self.airfield.is_some() || !self.alive {
            self.avoiding = None;
            self.avoiding_from = None;
            return intent;
        }
        let p = self.flight.position;
        let v = self.flight.velocity;
        // Every predicted conflict, and the soonest.
        let mut conflicts: Vec<(u32, [f64; 3])> = Vec::new();
        let mut conflict: Option<(f64, u32, [f64; 3])> = None;
        for other in world.iter().filter(|o| {
            o.id != self.id() && o.is_aircraft && o.alive && !o.destroyed && !o.on_ground
        }) {
            let r: [f64; 3] = std::array::from_fn(|i| other.position[i] - p[i]);
            let rv: [f64; 3] = std::array::from_fn(|i| other.velocity[i] - v[i]);
            let rv2 = rv.iter().map(|x| x * x).sum::<f64>();
            if rv2 < 1.0 {
                continue;
            }
            let t = -r.iter().zip(&rv).map(|(a, b)| a * b).sum::<f64>() / rv2;
            if !(0.0..=AVOID_HORIZON_S).contains(&t) {
                continue;
            }
            let miss = (0..3)
                .map(|i| (r[i] + rv[i] * t).powi(2))
                .sum::<f64>()
                .sqrt();
            // The aircraft it is attacking is closed on deliberately: only a
            // near collision with it counts.
            let safety = if self.controller.target() == Some(other.id) {
                AVOID_TARGET_FT
            } else {
                AVOID_SEPARATION_FT + AVOID_CLOSURE_S * rv2.sqrt()
            };
            if miss < safety {
                conflicts.push((other.id, r));
                if conflict.is_none_or(|(best, _, _)| t < best) {
                    conflict = Some((t, other.id, r));
                }
            }
        }
        let heading = self.flight.yaw;
        // The heading is held while the aircraft that set it is still a
        // conflict. Once it is not, a conflict with another aircraft sets a
        // new heading, away from that one: two aircraft that took up the same
        // heading to avoid a third must not hold each other on it.
        let held = self
            .avoiding_from
            .and_then(|id| conflicts.iter().find(|(other, _)| *other == id).copied());
        let pick = held.or(conflict.map(|(_, id, r)| (id, r)));
        if let Some((from, r)) = pick {
            let kept = self.avoiding.filter(|_| held.is_some());
            self.avoiding_from = Some(from);
            let target = match kept {
                Some((_, target)) => target,
                None => {
                    // Bearing of the other aircraft off our nose, positive right.
                    let bearing = (r[0].atan2(r[2]) - heading + std::f64::consts::PI)
                        .rem_euclid(std::f64::consts::TAU)
                        - std::f64::consts::PI;
                    let side = if bearing.abs() < AVOID_HEAD_ON_DEG.to_radians() {
                        1.0
                    } else {
                        -bearing.signum()
                    };
                    (heading.to_degrees() + side * AVOID_TURN_DEG).rem_euclid(360.0)
                }
            };
            self.avoiding = Some((tick + AVOID_HOLD_TICKS, target));
        } else if self.avoiding.is_some_and(|(until, _)| tick >= until) {
            self.avoiding = None;
            self.avoiding_from = None;
        }
        if let Some((_, target)) = self.avoiding {
            intent.heading_deg = target;
            intent.bank = super::motion::Bank::Unconstrained;
        }
        intent
    }

    /// The heading held to avoid other traffic, if any (for tests and traces).
    pub fn avoiding_heading_deg(&self) -> Option<f64> {
        self.avoiding.map(|(_, heading)| heading)
    }

    pub fn flight(&self) -> &flight::State {
        &self.flight
    }

    pub fn flight_mut(&mut self) -> &mut flight::State {
        &mut self.flight
    }

    pub fn controller(&self) -> &Controller {
        &self.controller
    }

    pub fn sensors(&self) -> Option<&Sensors> {
        self.sensors.as_ref()
    }

    /// Take away this actor's radar, infrared sensor and eyes, so that
    /// nothing it observes for itself reaches its decisions: only what the
    /// flight data link tells it does. A harness for the probe's
    /// `--probe-blind-wing`; no mission calls it.
    pub fn blind(&mut self) {
        if let Some(sensors) = self.sensors.as_mut() {
            sensors.profiles.radar = None;
            sensors.profiles.infrared = None;
            sensors.profiles.visual = None;
        }
    }

    /// The aircraft this actor is engaged with or searching for, which a
    /// human taking the aircraft over gets designated.
    pub fn current_target(&self) -> Option<u32> {
        self.controller.target().or(self.search_target)
    }

    /// Current observations and frozen records, exposed for diagnostics only.
    pub fn awareness(&self) -> &Memory {
        &self.awareness
    }

    pub fn assignment(&self) -> &engagement::Assignment {
        &self.assignment
    }

    pub fn set_assignment(&mut self, assignment: engagement::Assignment) {
        self.assignment = assignment;
        self.mission_policy.reset();
        self.controller.set_mission_target(None);
        self.controller
            .set_mission_hold_fire(self.assignment.stance == engagement::Stance::WeaponsHold);
        self.controller.set_mission_rejoin(None);
        self.search_target = None;
        self.controller.set_search_contact(None);
    }

    pub fn perceived_attacks(&self) -> &[ObservedAttack] {
        &self.observed_attacks
    }

    pub fn is_neutral(&self) -> bool {
        self.neutral
    }

    /// Lead succession: this aircraft now leads its wing, or no longer does.
    pub fn set_leads(&mut self, leads: bool) {
        self.identity.leads = leads;
        self.controller.set_leads(leads);
    }

    /// The place in the wing's formation this aircraft flies.
    pub fn wing_slot(&self) -> u8 {
        self.wing_slot
    }

    pub fn return_to_formation(&mut self, tick: u64) {
        self.neutral = true;
        self.formation_order_tick = Some(tick);
        self.ignored_attack_ids = self
            .observed_attacks
            .iter()
            .filter_map(|a| a.event_id)
            .chain(self.missile_threats.records().map(|r| r.missile_id))
            .collect();
        self.ignored_attack_ids.sort_unstable();
        self.ignored_attack_ids.dedup();
        self.controller.return_to_formation();
        self.activity = Activity::Formation;
        self.search_target = None;
        self.mission_policy.reset();
        if let Some(sensors) = self.sensors.as_mut() {
            sensors.clear_selection();
        }
    }

    fn permits_attack_response(&self, attack: &ObservedAttack) -> bool {
        !self.neutral
            || (self
                .formation_order_tick
                .is_none_or(|tick| attack.observed_tick > tick)
                && attack
                    .event_id
                    .is_none_or(|id| !self.ignored_attack_ids.contains(&id)))
    }

    /// Why this actor does not act on `attack`, or `None` when it does. The
    /// same tests as [`Self::permits_attack_response`], for explanation only.
    fn attack_ignore_reason(&self, attack: &ObservedAttack) -> Option<IgnoreReason> {
        if !self.neutral {
            return None;
        }
        if let Some(recalled_at) = self.formation_order_tick
            && attack.observed_tick <= recalled_at
        {
            return Some(IgnoreReason::SeenBeforeRecall { recalled_at });
        }
        attack
            .event_id
            .filter(|id| self.ignored_attack_ids.contains(id))
            .map(|event_id| IgnoreReason::KnownAtRecall { event_id })
    }

    /// Whether this actor already remembers this evidence (same report and
    /// projectile), so a new copy only refreshes it.
    fn holds_attack(&self, attack: &ObservedAttack) -> bool {
        self.observed_attacks
            .iter()
            .any(|held| held.report == attack.report && held.event_id == attack.event_id)
    }

    /// The attitude and speed the flight model delivered, for the trace.
    fn achieved(&self) -> thought::Achieved {
        thought::Achieved {
            heading_deg: self.flight.yaw.to_degrees(),
            flight_path_pitch_deg: self.flight_path_pitch_deg(),
            bank_deg: self.flight.bank.to_degrees(),
            speed_fps: self.flight.speed,
        }
    }

    fn remember_attack(&mut self, attack: ObservedAttack) {
        if let Some(previous) = self.observed_attacks.iter_mut().find(|previous| {
            previous.report == attack.report && previous.event_id == attack.event_id
        }) {
            *previous = attack;
        } else {
            self.observed_attacks.push(attack);
        }
    }

    pub fn missile_threats(&self) -> impl Iterator<Item = &ThreatRecord> {
        self.missile_threats.records()
    }

    pub fn lookout(&self) -> Option<awareness::Lookout> {
        self.lookout
    }

    pub fn incoming_fire_cue(&self) -> Option<incoming_fire::Cue> {
        self.incoming_fire.cue()
    }

    /// A B47 defence is being flown: a missile or gunfire maneuver, or a
    /// helicopter's break (which starts at the warning).
    fn defense_flying(&self) -> bool {
        self.last_defense.is_some_and(|d| d.motion.is_some())
            || self.fire_defending
            || self.controller.defending()
    }

    pub fn defense_decision(&self) -> Option<defense::DefenseDecision> {
        self.last_defense
    }

    pub fn stations(&self) -> &[StationSpec] {
        &self.stations
    }

    pub fn experience(&self) -> ResolvedExperience {
        self.controller.experience()
    }

    /// Change skill mid-flight, for decisions and awareness alike.
    pub fn set_experience(&mut self, experience: ResolvedExperience) {
        self.controller.set_experience(experience);
        self.awareness.set_experience(experience);
    }

    pub fn stations_mut(&mut self) -> &mut [StationSpec] {
        &mut self.stations
    }

    pub fn set_guns(
        &mut self,
        guns: std::collections::BTreeMap<u8, tore_formats::weapons::Weapon>,
    ) {
        self.guns = guns;
    }
    pub fn gun_target(&self) -> Option<super::gunnery::Target> {
        self.gun_target
    }
    pub fn physical_gun(&self, station: StationId) -> bool {
        self.guns.contains_key(&station.0)
    }
    pub fn debit_gun_round(&mut self, station: StationId) -> bool {
        let Some(store) = self.stations.iter_mut().find(|s| s.station == station) else {
            return false;
        };
        if store.store.inhibited || store.is_empty() {
            return false;
        }
        if weapon_service::debit_ammunition(&mut store.store, 1).is_err() {
            return false;
        }
        self.flight.payload_lbs = (self.flight.payload_lbs - store.external_round_lbs).max(0.);
        true
    }

    pub fn set_stations(&mut self, stations: Vec<StationSpec>) {
        self.stations = stations;
    }

    /// Hand the aircraft's parts over and drop its pilot: the AI's awareness,
    /// orders, assignment and decisions go with the actor.
    pub fn into_parts(self) -> ActorParts {
        ActorParts {
            identity: self.identity,
            flight: self.flight,
            sensors: self.sensors,
            stations: self.stations,
            dispensers: self.dispensers,
            warnings: self.missile_threats,
            equipment: self.equipment,
        }
    }

    /// Replace the missile warnings, for an aircraft whose warning record
    /// comes from a cockpit.
    pub fn set_missile_threats(&mut self, warnings: ThreatService) {
        self.missile_threats = warnings;
    }

    pub fn set_dispensers(&mut self, dispensers: Vec<DispenserStore>) {
        self.dispensers = dispensers;
    }

    pub fn dispensers(&self) -> &[DispenserStore] {
        &self.dispensers
    }

    pub fn activity(&self) -> Activity {
        self.activity
    }

    pub fn alive(&self) -> bool {
        self.alive && !self.flight.crashed
    }

    pub fn last_input(&self) -> &PilotInput {
        &self.last_input
    }

    /// Total rounds remaining across every station, for a no-free-ammunition
    /// check in tests and reports.
    pub fn rounds_remaining(&self) -> u32 {
        self.stations
            .iter()
            .map(|s| match s.store.rounds {
                Rounds::Finite(n) => n,
                Rounds::Unlimited => 0,
            })
            .sum()
    }

    /// Report a missile launched at this actor. Only the aircraft the missile
    /// was fired at is ever told (B47); the host must not broadcast.
    pub fn report_threat(&mut self, report: ThreatReport) {
        self.pending_threats.push(report);
    }

    /// Legacy damage event, without evidence that a weapon caused it.
    pub fn report_damage(&mut self) {
        self.pending_events.push(FrameEvent::Hit);
    }

    /// An actual weapon hit, with no implied shooter identity.
    pub fn report_hit(&mut self) {
        self.pending_events.push(FrameEvent::Hit);
        self.incoming_fire.hit();
    }

    /// Report that an object left the world.
    pub fn report_removed(&mut self, id: u32) {
        self.pending_events.push(FrameEvent::ActorRemoved(id));
    }

    pub fn set_alive(&mut self, alive: bool) {
        self.alive = alive;
        if !alive {
            self.awareness.clear();
            self.missile_threats.clear();
            self.defense_state = defense::DefenseState::default();
            self.last_defense = None;
            self.incoming_fire = incoming_fire::Service::default();
            self.fire_defending = false;
            self.lookout = None;
            self.controller.set_defense_motion(None);
            self.search_target = None;
            self.controller.set_search_contact(None);
        }
    }

    /// Deliver one wing command to this actor (B46).
    ///
    /// The four outcomes stay distinct, so a caller can tell an applied
    /// setting from a rejection and from motion actually installed.
    pub fn order(
        &mut self,
        request: super::wing::WingRequest,
        tick: u64,
    ) -> Result<super::wing::ReceiverOutcome> {
        use super::wing::{ReceiverOutcome, RejectReason, TargetOrder, WingRequest};
        if self.dummy {
            return Ok(ReceiverOutcome::Rejected(RejectReason::Dummy));
        }
        // Manual p.160: a wingman that bugged out no longer answers.
        if self.bugged_out {
            return Ok(ReceiverOutcome::Rejected(RejectReason::BuggedOut));
        }
        if self.damage_return.is_some()
            && !matches!(
                request,
                WingRequest::Land(_) | WingRequest::TargetAssignment(TargetOrder::HoldFire)
            )
        {
            return Ok(ReceiverOutcome::Rejected(RejectReason::DamageRecovery));
        }
        let phase = self.airfield_phase();
        if phase == Some(super::airfield::Phase::Parked) {
            return Ok(ReceiverOutcome::Rejected(RejectReason::Landed));
        }
        if let WingRequest::Land(order) = request {
            // Retail bug out is ignored in any airport state or on the ground;
            // the private route home is free flight and does not count.
            if order.reason == super::airfield::LandingReason::BugOut
                && (phase.is_some_and(|p| p != super::airfield::Phase::Inbound)
                    || self.flight.research.as_ref().is_some_and(|r| r.on_ground))
            {
                return Ok(ReceiverOutcome::Rejected(RejectReason::OnAirfield));
            }
            self.accept_landing(order, tick);
        } else if matches!(
            request,
            WingRequest::FormationSelection(_)
                | WingRequest::TargetAssignment(TargetOrder::HoldFire)
        ) && self.damage_return.is_none()
            && self.landing_order.is_some_and(|o| o.reason.cancellable())
            && phase.is_none_or(|p| {
                matches!(
                    p,
                    super::airfield::Phase::Inbound
                        | super::airfield::Phase::Marshal
                        | super::airfield::Phase::Approach
                )
            })
        {
            // Fitted: disengage and formation orders cancel an ordered
            // landing that has not yet reached final. A cancelled join
            // stays cancelled until the leader is next airborne.
            if self
                .landing_order
                .is_some_and(|o| o.reason == super::airfield::LandingReason::JoinLeader)
            {
                self.join_cancelled = true;
            }
            self.landing_order = None;
            if self.airfield.as_ref().is_some_and(|s| !s.is_departure()) {
                self.leave_airfield();
            }
        }
        self.controller
            .prepare_order(self.flight.yaw.to_degrees(), self.speed_limits());
        let outcome = self.controller.receive_order(request, tick)?;
        if matches!(
            outcome,
            super::wing::ReceiverOutcome::Applied(_)
                | super::wing::ReceiverOutcome::MotionInstalled(_)
        ) {
            match request {
                super::wing::WingRequest::TargetAssignment(
                    super::wing::TargetOrder::ConcreteTarget(id),
                ) => {
                    self.neutral = false;
                    self.set_assignment(engagement::Assignment {
                        role: engagement::Role::Intercept,
                        stance: engagement::Stance::EngageAssigned,
                        destroy_ids: vec![id.0],
                        ..engagement::Assignment::default()
                    });
                }
                super::wing::WingRequest::TargetAssignment(super::wing::TargetOrder::HoldFire) => {
                    self.return_to_formation(tick);
                }
                super::wing::WingRequest::FormationSelection(_) => self.return_to_formation(tick),
                super::wing::WingRequest::TargetAssignment(
                    super::wing::TargetOrder::FreeSelection,
                ) => {
                    self.neutral = false;
                }
                _ => {}
            }
        }
        Ok(outcome)
    }

    /// Take a landing order. A bug out also leaves the wing for good.
    fn accept_landing(&mut self, order: super::airfield::LandingOrder, tick: u64) {
        use super::airfield::Phase;
        if order.reason == super::airfield::LandingReason::BugOut {
            self.bugged_out = true;
            self.neutral = true;
            self.formation_order_tick = Some(tick);
            self.controller.return_to_formation();
            self.search_target = None;
        }
        self.landing_order = Some(order);
        // A landing already committed to final at the same runway continues;
        // any other landing restarts toward the new runway. A departure in
        // progress finishes its climb-out first.
        if let Some(sequence) = &self.airfield
            && !sequence.is_departure()
        {
            let committed = sequence.runway().object == order.runway.object
                && !matches!(
                    sequence.phase(),
                    Phase::Inbound | Phase::Marshal | Phase::Approach
                );
            if !committed {
                self.leave_airfield();
            }
        }
    }

    /// Back to free flight from a landing sequence: the takeoff-finish tail
    /// (retail `0x4bbfe0`) raises the gear and flaps and closes the
    /// speedbrake, so nothing is left hanging in combat.
    fn leave_airfield(&mut self) {
        use tore_input::{PilotCommand, Switch};
        self.airfield = None;
        self.flight.command(PilotCommand::Set(Switch::Gear, false));
        self.flight.command(PilotCommand::Set(Switch::Flaps, false));
        self.flight
            .command(PilotCommand::Set(Switch::Airbrake, false));
    }

    /// Start the landing sequence for the stored order. Legacy-model aircraft
    /// switch to the researched model here so runway contact is modelled.
    fn begin_landing(
        &mut self,
        order: super::airfield::LandingOrder,
        own: &OwnState,
        surface: &dyn Fn(f64, f64) -> Surface,
        height: &dyn Fn(f64, f64) -> f64,
    ) {
        if self.flight.research.is_none() {
            let seed = (self.identity.actor.0 as i32).wrapping_mul(7919) | 1;
            if self.flight.enable_research(seed).is_err() {
                // The restricted native adapter cannot land: keep the
                // ordinary return-to-base heading instead.
                self.landing_order = None;
                return;
            }
        }
        // B48 join-landing enters the marshal directly; every other landing
        // first flies the private route home.
        let joining = order.reason == super::airfield::LandingReason::JoinLeader;
        let route = super::route::bingo_route(
            super::route::WingCrew {
                wingman_ai: true,
                leader_ai: true,
            },
            super::route::Position {
                x: order.runway.center[0],
                z: order.runway.center[2],
            },
            &own.limits,
            &mut self.route_random,
        )
        .map(|r| (f64::from(r.altitude_ft), r.speed.0))
        .filter(|_| !joining);
        let wind = surface(order.runway.center[0], order.runway.center[2]).wind;
        let max_takeoff_lbs = self.flight.model().configuration().mass.max_takeoff_lbs;
        let mut sequence = super::airfield::Sequence::landing(
            order,
            self.flight.position,
            wind,
            max_takeoff_lbs,
            route,
        );
        sequence.prefer_clear_approach(wind, max_takeoff_lbs, &|x, z| height(x, z));
        // Helicopters, the V-22 and a hovering jet land vertically (slice
        // H2; John 2026-10-12: vertical for all three VTOL types).
        self.airfield = Some(sequence.with_lift(self.airfield_lift()));
    }

    /// One tick of a takeoff or landing sequence. Returns `false` when the
    /// aircraft is in free flight and the ordinary controller should run.
    #[allow(clippy::too_many_arguments)]
    fn airfield_tick(
        &mut self,
        clearance: &AirfieldClearance,
        own: &OwnState,
        tick: u64,
        ground: &dyn Fn(f64, f64) -> f64,
        surface: &dyn Fn(f64, f64) -> Surface,
        height: &dyn Fn(f64, f64) -> f64,
        output: &mut MissionOutput,
    ) -> Result<bool> {
        use super::airfield::{Control, LandingReason, Phase, Situation};
        let defending = self.defense_flying();
        let warned = self.pending_threats.iter().any(|r| !r.launcher_same_side);
        let mut trace = thought::AirfieldTrace::default();
        if self.airfield.is_none()
            && let Some(order) = self.landing_order
            && !defending
            && !warned
        {
            self.begin_landing(order, own, surface, height);
            trace.began_landing = self.airfield.is_some();
        }
        let Some(phase) = self.airfield.as_ref().map(|s| s.phase()) else {
            return Ok(false);
        };
        if !clearance.leader_landing
            && matches!(phase, Phase::Inbound | Phase::Marshal | Phase::Approach)
            && self
                .landing_order
                .is_some_and(|o| o.reason == LandingReason::JoinLeader)
        {
            // Retail wing abort: gear and flaps up, free flight.
            self.landing_order = None;
            self.leave_airfield();
            trace.left = Some(AirfieldExit::WingAbort);
            self.trace.airfield = Some(trace);
            return Ok(false);
        }
        match phase.flight_state() {
            // B47: a warning in the first approach states abandons the
            // approach; the landing resumes once the threat has gone.
            // Free flight on the inbound route reacts like any other flight.
            FlightState::EarlyApproach | FlightState::Free if defending || warned => {
                self.leave_airfield();
                trace.left = Some(AirfieldExit::MissileThreat { defending, warned });
                self.trace.airfield = Some(trace);
                return Ok(false);
            }
            // B47: warnings are ignored while taking off and landing.
            _ => {
                self.trace.dropped_warnings.extend(
                    self.pending_threats
                        .iter()
                        .map(|report| (*report, DropReason::Airfield { phase })),
                );
                self.pending_threats.clear();
                self.pending_events.clear();
            }
        }
        let config = self.flight.model().configuration();
        let situation = Situation {
            tick,
            position: self.flight.position,
            velocity: self.flight.velocity,
            heading_deg: self.flight.yaw.to_degrees(),
            body_pitch_deg: self.flight.pitch.to_degrees(),
            speed_fps: self.flight.speed,
            on_ground: self.flight.research.as_ref().is_some_and(|r| r.on_ground),
            ground_clearance_ft: config.equipment.ground_clearance_ft,
            agl_ft: own.agl_ft,
            terrain_ahead_ft: {
                let [x, _, z] = self.flight.position;
                let [vx, _, vz] = self.flight.velocity;
                let speed = vx.hypot(vz).max(1.0);
                (0..=6)
                    .map(|i| {
                        let d = super::airfield::TERRAIN_LOOKAHEAD_FT * f64::from(i) / 6.0;
                        ground(x + vx / speed * d, z + vz / speed * d)
                    })
                    .fold(f64::MIN, f64::max)
            },
            // On the approach the aircraft turns at each gate, so only the
            // ground on the way to the gate it is flying to matters.
            terrain_to_gate_ft: match self.airfield.as_ref() {
                Some(sequence) if phase == Phase::Approach => {
                    let [x, _, z] = self.flight.position;
                    let gate = sequence.gate(sequence.leg());
                    (0..=6)
                        .map(|i| {
                            let f = f64::from(i) / 6.0;
                            ground(x + (gate[0] - x) * f, z + (gate[2] - z) * f)
                        })
                        .fold(f64::MIN, f64::max)
                }
                _ => f64::MIN,
            },
            minimum_speed_fps: own.limits.minimum.0,
            approach_minimum_fps: self.approach_minimum_fps(),
            maximum_speed_fps: own.limits.maximum.0,
            corner_speed_fps: own.limits.corner.0,
            // A rotorcraft's own cruise (slice H3) on its way home.
            cruise_speed_fps: if super::rotor_adapter::flies(&self.flight) {
                super::route::rotorcraft_cruise_speed(&own.limits).0
            } else {
                super::route::cruise_speed(&own.limits).0
            },
            has_afterburner: config.propulsion.afterburner_thrust_lbf > 0.0,
            wind: surface(self.flight.position[0], self.flight.position[2]).wind,
            wing_position: self.identity.member,
            turn_clear: clearance.turn,
            runway_free: clearance.runway_free,
            wing_landed: clearance.wing_landed,
            free_slot: clearance.free_slot,
            touchdown_safe: self.touchdown_safe(surface),
            hover_capable: matches!(
                self.airfield.as_ref().map(|s| s.lift()),
                Some(super::airfield::Lift::Jet | super::airfield::Lift::ShortJet)
            ) && self
                .hover_capable(surface(self.flight.position[0], self.flight.position[2]).height),
            detour: if phase == Phase::Inbound {
                self.home_detour()
            } else {
                None
            },
        };
        let sequence = self.airfield.as_mut().expect("phase read above");
        let mut step = sequence.step(&situation);
        // A hover law starting afresh (a liftoff, a landing's hover, the air
        // taxi off the ground) forgets its memory of the last one (slice H2).
        let hovering = |p: Phase| {
            matches!(
                p,
                Phase::VerticalClimb
                    | Phase::Transition
                    | Phase::HoverApproach
                    | Phase::VerticalDescent
                    | Phase::AirTaxi
            )
        };
        if hovering(sequence.phase()) && !hovering(phase) {
            self.rotor.reset();
            self.jet_lift.reset();
        }
        if self.damage_return.is_some()
            && let Control::Air(guidance) = &mut step.command.control
            && !guidance.full_power
        {
            guidance.speed_fps = guidance.speed_fps.min(own.limits.minimum.0 * 1.6);
            if matches!(phase, Phase::Inbound | Phase::Marshal | Phase::Approach) {
                step.command.brakes = self.flight.speed > guidance.speed_fps + 20.;
            }
        }
        trace.situation = Some(situation);
        trace.step = Some(step);
        self.trace.airfield = Some(trace);
        if step.complete {
            self.airfield = None;
        }
        let command = step.command;
        self.activity = command.activity;
        output.activities.push((self.id(), command.activity));
        let mut fly = FlyTrace {
            intent: None,
            terrain_floor_deg: None,
            adapter: None,
            rotor: None,
            jet_lift: None,
            achieved: thought::Achieved::default(),
        };
        let mut input = match command.control {
            Control::Ground {
                throttle,
                pitch,
                yaw,
            } => PilotInput {
                pitch,
                roll: 0.0,
                yaw,
                throttle: Some(throttle),
                ..PilotInput::default()
            },
            Control::Air(guidance) => {
                let clock = super::motion::CommandClock::at_tick(tick);
                let bank = if guidance.wings_level {
                    super::motion::Bank::Explicit(0)
                } else {
                    super::motion::Bank::Unconstrained
                };
                let duration = super::motion::Duration::Timed(1);
                let intent = MotionIntent {
                    id: 0,
                    request: super::motion::MotionRequest::new(
                        guidance.heading_deg.round() as i32,
                        super::motion::PitchRequest::Explicit(
                            guidance.flight_path_pitch_deg.round() as i32,
                        ),
                        bank,
                        super::motion::SpeedRequest::Explicit(ScalarSpeed(guidance.speed_fps)),
                        duration,
                    ),
                    heading_deg: guidance.heading_deg,
                    flight_path_pitch_deg: guidance.flight_path_pitch_deg,
                    speed: ScalarSpeed(guidance.speed_fps),
                    bank,
                    completion: super::controller::Completion::Deadline(
                        super::motion::deadline_for(duration, clock).expect("timed guidance"),
                    ),
                    steering_point: None,
                    mode: super::steering::CommandMode::OtherState,
                    formation_flight: false,
                    afterburner: command.afterburner,
                };
                let floor = if guidance.terrain_floor {
                    self.controller.terrain_floor(&dummy_frame(own))?
                } else {
                    None
                };
                fly.intent = Some(intent);
                fly.terrain_floor_deg = floor;
                if super::rotor_adapter::flies(&self.flight) {
                    // Helicopters and the V-22 (slice H1): the engines
                    // are governed at full power anyway.
                    self.fly_rotor(
                        super::rotor_adapter::RotorRequest::Steer {
                            intent: &intent,
                            terrain_pitch_floor_deg: floor,
                        },
                        own,
                        surface,
                        &mut fly,
                    )
                } else {
                    let adapter = self.adapter.controls(
                        &self.flight,
                        &intent,
                        &own.limits,
                        own.g_limit,
                        own.roll_limit_deg_per_s,
                        own.maximum_bank_deg,
                        floor,
                        flight::DT,
                    )?;
                    let mut input = adapter.input.clone();
                    fly.adapter = Some(adapter);
                    if guidance.full_power {
                        input.throttle = Some(1.0);
                    }
                    input
                }
            }
            Control::Hover(guidance) => self.fly_hover(&guidance, own, surface, height, &mut fly),
        };
        input.commands = vec![
            tore_input::PilotCommand::Set(tore_input::Switch::Burner, command.afterburner),
            tore_input::PilotCommand::Set(tore_input::Switch::Gear, command.gear_down),
            tore_input::PilotCommand::Set(tore_input::Switch::Flaps, command.flaps_down),
            tore_input::PilotCommand::Set(tore_input::Switch::Airbrake, command.brakes),
        ];
        // The collective and nozzle an airfield phase sets itself (slice H2).
        if let Some(collective) = command.lift.collective {
            input.collective = Some(collective);
            input.collective_rate = 0.0;
        }
        // The engine switch, only when it changes: a parked helicopter
        // shuts down (slice HF1).
        if let Some(on) = command.lift.engine
            && on != self.flight.engine
        {
            input.commands.push(tore_input::PilotCommand::Set(
                tore_input::Switch::Engine,
                on,
            ));
        }
        if let Some(degrees) = command.lift.nozzle_deg
            && let Some(jet) = self.flight.model().powered_lift().and_then(|l| l.jet)
        {
            input.vector_pitch = Some((degrees / jet.nozzle_range_degrees).clamp(0.0, 1.0));
            input.vector_pitch_rate = 0.0;
        }
        self.fly_input(input, ground, surface);
        fly.achieved = self.achieved();
        self.trace.fly = Some(fly);
        Ok(true)
    }

    /// A helicopter's (or the V-22's) way home past the threat rings in its
    /// ground picture (slice H2): the point to fly to first, when a known
    /// ring stands across the straight line to the landing point or it is
    /// inside one ([`super::threat_route`]).
    fn home_detour(&self) -> Option<[f64; 2]> {
        let sequence = self.airfield.as_ref()?;
        if sequence.lift() != super::airfield::Lift::Rotor {
            return None;
        }
        let picture = self.ground.as_deref()?;
        let rings = super::helicopter_threat::known_rings(picture, self.identity.side);
        super::threat_route::detour(&rings, self.flight.position, sequence.landing_point())
    }

    /// Whether the shared touchdown rule ([`crate::touchdown`]) would accept
    /// this aircraft touching down now on the ground under it, held to the
    /// rule's off-field limits (sink and ground speed) on an airfield too, so
    /// an AI vertical landing is a gentle one anywhere (slice H2). An
    /// aircraft whose touchdown class may not land off an airfield is judged
    /// by the airfield rule alone.
    fn touchdown_safe(&self, surface: &dyn Fn(f64, f64) -> Surface) -> bool {
        use crate::touchdown::{Verdict, assess, assess_with};
        if self.airfield.as_ref().is_none_or(|s| !s.lift().vertical()) {
            return true;
        }
        let below = surface(self.flight.position[0], self.flight.position[2]);
        let now = assess(&self.flight, &below);
        if now.verdict != Verdict::Safe {
            return false;
        }
        if !now.class.can_land_off_airfield() {
            return true;
        }
        let open = Surface {
            landable: false,
            ..below
        };
        assess_with(now.class, now.sink_fps, now.ground_speed_knots, &open).verdict == Verdict::Safe
    }

    /// The home position for fuel planning: the home airport, or the home
    /// runway's centre when only the runway is known.
    fn home(&self) -> Option<super::route::Position> {
        self.home_airport.or_else(|| {
            self.home_runway.map(|r| super::route::Position {
                x: r.center[0],
                z: r.center[2],
            })
        })
    }

    /// Set this actor's remaining internal fuel, in pounds.
    ///
    /// Used by the host when fuel is tracked outside the flight model, and by
    /// fixtures that need a specific B48 fuel state.
    pub fn set_internal_fuel(&mut self, pounds: f64) {
        self.flight.fuel = pounds;
    }
}

/// The AI side of a live mission: every AI-flown aircraft, stepped together.
///
/// The player is not an actor here. It enters as a [`WorldObject`] like any
/// other participant, so no player state can contaminate an AI decision.
#[cfg_attr(test, derive(Clone, Debug))]
pub struct AiMission {
    actors: Vec<AiActor>,
    tick: u64,
    wing_control: WingControl,
    formation: Formation,
    horizontal_spacing_ft: i32,
    vertical_spacing_ft: i32,
    /// Every human-flown aircraft that belongs to a wing.
    humans: Vec<HumanMember>,
    /// The current leader of each wing that has one.
    leaders: Vec<WingLeader>,
    /// The humans that were flying at the last leadership check, for placing
    /// an aircraft that joins between steps.
    humans_flying: Vec<u32>,
    missiles: Vec<MissileSnapshot>,
    gun_rounds: Vec<incoming_fire::Round>,
    /// The highest the ground reaches over each part of the world, when the
    /// host knows it: a shortcut for sight lines above it, never a change to
    /// any answer.
    ground_ceiling: Option<std::sync::Arc<crate::ground_ceiling::GroundCeiling>>,
    /// The AI test settings (observation rate, lean sensors), set by the host
    /// before every step and not part of a checkpoint. See
    /// [`super::test_settings`].
    test: AiTestSettings,
    /// The mission assignment of each human-flown aircraft, by aircraft id.
    human_assignments: std::collections::BTreeMap<u32, engagement::Assignment>,
    /// The aircraft each human-flown aircraft must keep alive, by aircraft id.
    must_survive: std::collections::BTreeMap<u32, Vec<u32>>,
    pending_attack_reports: Vec<(u32, ObservedAttack)>,
    /// What the host last handed over from the flight data link, consumed by
    /// the next step ([`Self::set_link`]). Per-step scratch, not state.
    link: super::link::LinkInput,
    /// Airport where each human-flown aircraft holds landing priority, by
    /// aircraft id.
    priority_landing: std::collections::BTreeMap<u32, u32>,
    /// Sides that have seen a living hostile aircraft (mission RTB).
    hostiles_seen: Vec<super::targeting::Side>,
    /// External leaders seen airborne, so a later touchdown reads as landing.
    airborne_seen: Vec<u32>,
    /// The mission of opportunity of every wing whose human leader was lost
    /// and whose lead passed to an AI aircraft (John, 2026-09-30).
    opportunities: Vec<super::opportunity::Opportunity>,
    /// Each wing's remaining waypoints, in order, as the host last gave them
    /// ([`Self::set_wing_route`]).
    routes: Vec<(super::targeting::Side, u8, Vec<[f64; 3]>)>,
    /// The tick each flight's AI lead last sorted its wingmen (slice G4), so
    /// a flight sorts at most once every
    /// [`SORT_INTERVAL_TICKS`](super::link::SORT_INTERVAL_TICKS).
    sort_clock: Vec<super::link::SortStamp>,
    /// Write-only journal of messages between aircraft. No decision reads
    /// it; the host drains it with [`Self::take_journal`].
    journal: thought::Journal,
    /// The lead hold's claims, as the host last gave them
    /// ([`Self::set_lead_claims`]); empty in single player.
    lead_claims: Vec<LeadClaim>,
    /// Wingmen that were fighting when their owner took the lead back: each
    /// re-forms on the owner once its fight is over (John, 2026-10-09).
    reform_after: Vec<u32>,
}

impl Default for AiMission {
    fn default() -> Self {
        Self::new()
    }
}

impl AiMission {
    pub fn new() -> Self {
        Self {
            actors: Vec::new(),
            tick: 0,
            wing_control: WingControl::Loose,
            formation: Formation::Echelon,
            horizontal_spacing_ft: super::wing::PLAYER_SPACING_SPREAD_FT,
            vertical_spacing_ft: super::wing::PLAYER_STACKING_FT,
            humans: Vec::new(),
            leaders: Vec::new(),
            humans_flying: Vec::new(),
            missiles: Vec::new(),
            gun_rounds: Vec::new(),
            ground_ceiling: None,
            test: AiTestSettings::default(),
            human_assignments: Default::default(),
            must_survive: Default::default(),
            pending_attack_reports: Vec::new(),
            link: Default::default(),
            priority_landing: Default::default(),
            hostiles_seen: Vec::new(),
            airborne_seen: Vec::new(),
            opportunities: Vec::new(),
            routes: Vec::new(),
            sort_clock: Vec::new(),
            journal: thought::Journal::default(),
            lead_claims: Vec::new(),
            reform_after: Vec::new(),
        }
    }

    /// Drain the message journal: attack reports, wing orders, escort
    /// priorities and missile warnings since the last drain, oldest first.
    /// Hosts call this once per tick after [`Self::step`]; an undrained
    /// journal keeps its latest [`thought::JOURNAL_LIMIT`] entries.
    pub fn take_journal(&mut self) -> thought::JournalBatch {
        self.journal.take()
    }

    /// The journal entries not yet drained.
    pub fn journal(&self) -> &thought::Journal {
        &self.journal
    }

    /// The human-flown aircraft `human` is landing at this airport (retail:
    /// gear down, below 4000 ft above ground, at most 953 ft/s, within 25000
    /// ft of a friendly airport). It keeps that runway busy: AI aircraft
    /// landing there hold at marshal and none start a takeoff until it is
    /// cleared with `None` (manual p.65: "your aircraft always receives first
    /// landing clearance"). Every human keeps its own claim. A plain store,
    /// cheap and idempotent to call every tick.
    pub fn set_priority_landing(&mut self, human: u32, airport: Option<u32>) {
        match airport {
            Some(airport) => {
                self.priority_landing.insert(human, airport);
            }
            None => {
                self.priority_landing.remove(&human);
            }
        }
    }

    /// The airport where `human` holds landing priority, if it does.
    pub fn priority_landing(&self, human: u32) -> Option<u32> {
        self.priority_landing.get(&human).copied()
    }

    /// Every landing priority as (human aircraft id, airport), in id order.
    pub fn priority_landings(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.priority_landing.iter().map(|(h, a)| (*h, *a))
    }

    /// What `human` must keep alive; empty for an aircraft with no list.
    pub fn must_survive(&self, human: u32) -> &[u32] {
        self.must_survive.get(&human).map_or(&[], Vec::as_slice)
    }

    /// Mission requirements do not silently replace aircraft combat orders.
    pub fn set_must_survive(&mut self, human: u32, mut ids: Vec<u32>) {
        ids.sort_unstable();
        ids.dedup();
        self.must_survive.insert(human, ids);
    }

    /// The mission assignment of human-flown aircraft `human`; the default
    /// (free fire, nothing assigned) for one with none.
    pub fn human_assignment(&self, human: u32) -> engagement::Assignment {
        self.human_assignments
            .get(&human)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_human_assignment(&mut self, human: u32, assignment: engagement::Assignment) {
        self.human_assignments.insert(human, assignment);
    }

    /// Objectives follow lineages (the lobby pass's follow-up F1, agent
    /// decision): `new`, a plane a respawn or a revival added, continues the
    /// lineage of the planes `lineage`, oldest first. Every mission list that
    /// names a plane of the lineage names `new` too: each actor's and each
    /// human-flown plane's assignment ([`engagement::Assignment::join_lineage`],
    /// changed in place, so no actor's engagement starts afresh) and each
    /// survival list. `new` itself takes the newest plane of the lineage's
    /// human assignment and survival list when it has none of its own, so
    /// whoever flies it is asked what was asked of the lineage. Single player
    /// never adds a plane, so nothing here runs there.
    pub fn join_lineage(&mut self, new: u32, lineage: &[u32]) {
        for actor in &mut self.actors {
            actor.assignment.join_lineage(new, lineage);
        }
        for assignment in self.human_assignments.values_mut() {
            assignment.join_lineage(new, lineage);
        }
        for ids in self.must_survive.values_mut() {
            if ids.iter().any(|id| lineage.contains(id)) && !ids.contains(&new) {
                ids.push(new);
                ids.sort_unstable();
            }
        }
        let elder = |plane: &&u32| **plane != new;
        if !self.human_assignments.contains_key(&new)
            && let Some(assignment) = lineage
                .iter()
                .rev()
                .filter(elder)
                .find_map(|plane| self.human_assignments.get(plane))
                .cloned()
        {
            self.human_assignments.insert(new, assignment);
        }
        if !self.must_survive.contains_key(&new)
            && let Some(ids) = lineage
                .iter()
                .rev()
                .filter(elder)
                .find_map(|plane| self.must_survive.get(plane))
                .cloned()
        {
            self.must_survive.insert(new, ids);
        }
    }

    /// Quick Mission startup permission is independent of its group objectives.
    pub fn start_in_formation(&mut self) {
        for actor in &mut self.actors {
            actor.return_to_formation(self.tick);
            actor.formation_order_tick = None;
        }
    }

    /// Queue a legitimately perceived attack for the observer and its assigned
    /// friendly escorts on the next simulation step. Unknown attackers remain
    /// unknown; these reports never populate aircraft observation or memory.
    pub fn report_attack(&mut self, receiver: u32, report: engagement::ThreatReport) {
        self.report_attack_bearing(receiver, report, None);
    }

    pub fn report_attack_bearing(
        &mut self,
        receiver: u32,
        report: engagement::ThreatReport,
        bearing_world_deg: Option<f64>,
    ) {
        self.report_attack_evidence(receiver, report, bearing_world_deg, None);
    }

    pub fn report_attack_evidence(
        &mut self,
        receiver: u32,
        report: engagement::ThreatReport,
        bearing_world_deg: Option<f64>,
        event_id: Option<u32>,
    ) {
        let attack = ObservedAttack {
            report,
            bearing_world_deg,
            observed_tick: self.tick,
            event_id,
        };
        let ignored = |reason| JournalEntry {
            tick: attack.observed_tick,
            sender: Some(receiver),
            message: Message::AttackEvidence(attack),
            receipts: vec![Receipt {
                actor: receiver,
                outcome: Outcome::Ignored(reason),
            }],
        };
        if report.defended_id != receiver {
            self.journal
                .push(ignored(IgnoreReason::NotTheDefendedAircraft));
            return;
        }
        let identity = self
            .actor(receiver)
            .map(|a| (a.identity.side, a.identity.wing))
            .or_else(|| {
                self.humans
                    .iter()
                    .find(|h| h.id == receiver)
                    .map(|h| (h.side, h.wing))
            });
        let Some((side, wing)) = identity else {
            self.journal.push(ignored(IgnoreReason::UnknownReporter));
            return;
        };
        // Journal only evidence a recipient does not already hold or await;
        // a refresh of known evidence is not a new message.
        let mut receipts = Vec::new();
        for actor in &self.actors {
            if actor.alive()
                && actor.identity.side == side
                && (actor.id() == receiver
                    || actor.assignment.protected_ids.contains(&receiver)
                    || (actor.identity.is_leader() && actor.identity.wing == wing))
            {
                let key = (actor.id(), attack);
                if !self.pending_attack_reports.contains(&key) {
                    let new = !actor.holds_attack(&attack)
                        && !self.pending_attack_reports.iter().any(|(id, pending)| {
                            *id == actor.id()
                                && pending.report == attack.report
                                && pending.event_id == attack.event_id
                        });
                    self.pending_attack_reports.push(key);
                    if new {
                        receipts.push(Receipt {
                            actor: actor.id(),
                            outcome: Outcome::Queued,
                        });
                    }
                }
            }
        }
        if !receipts.is_empty() {
            self.journal.push(JournalEntry {
                tick: attack.observed_tick,
                sender: Some(receiver),
                message: Message::AttackEvidence(attack),
                receipts,
            });
        }
    }

    /// Complete current missile lifecycle snapshot, shared with the player
    /// receiver. The observation service, not the controller, reads this data.
    pub fn set_gun_rounds(&mut self, rounds: Vec<incoming_fire::Round>) {
        self.gun_rounds = rounds;
    }

    /// Sets the AI test settings; they apply from the next step.
    pub fn set_test_settings(&mut self, settings: AiTestSettings) {
        self.test = settings;
    }

    /// The AI test settings in force.
    pub fn test_settings(&self) -> AiTestSettings {
        self.test
    }

    /// Whether actor `index` observes at this tick of the mission: always at
    /// the default rate, otherwise every `divisor` ticks, the aircraft
    /// staggered by their place so each tick carries the same share.
    fn observation_due(&self, index: usize, tick: u64) -> bool {
        let divisor = u64::from(self.test.divisor());
        divisor <= 1 || (tick + index as u64).is_multiple_of(divisor)
    }

    /// The highest the ground the steps are given can reach over each part of
    /// the world, terrain and runways alike, or `None` when unknown.
    /// Incoming-fire sight lines wholly above it skip the terrain samples;
    /// every answer stays the same.
    pub fn set_ground_ceiling(
        &mut self,
        ceiling: Option<std::sync::Arc<crate::ground_ceiling::GroundCeiling>>,
    ) {
        self.ground_ceiling = ceiling;
    }

    /// Apply combat's explicit inert-store notices before stepping the mission.
    /// Only existing checkpointed observation and defense state is changed.
    pub fn end_missile_threats(&mut self, ids: &[u32]) {
        for actor in &mut self.actors {
            for &id in ids {
                actor.missile_threats.end_threat(id);
                actor.defense_state.end_threat(id);
            }
        }
    }

    pub fn set_missiles(&mut self, missiles: Vec<MissileSnapshot>) {
        self.missiles = missiles;
    }

    pub fn push(&mut self, actor: AiActor) {
        self.actors.push(actor);
    }

    /// Take actor `id` out of the mission, for a human taking its aircraft.
    /// The others keep their order. Nothing is announced to them: the
    /// aircraft still exists, and they see it in the world snapshot.
    pub fn remove_actor(&mut self, id: u32) -> Option<AiActor> {
        let index = self.actors.iter().position(|a| a.id() == id)?;
        Some(self.actors.remove(index))
    }

    /// Add an actor in id order, as it stands at mission start: neutral and
    /// with fresh awareness. It replaces nothing: the id must not be an actor
    /// already.
    pub fn insert_actor(&mut self, mut actor: AiActor) -> Result<()> {
        if self.actor(actor.id()).is_some() {
            return Err(super::AiError::InvalidInput("actor id is already flying"));
        }
        // What `start_in_formation` does for every actor at the start.
        actor.return_to_formation(self.tick);
        actor.formation_order_tick = None;
        // It joins its wing as it stands: leading if the wing's leader is its
        // id (a human leader handing its aircraft to the AI), else as a
        // wingman in the lowest formation slot no living AI wingman of the
        // wing holds (slice R1 of the lobby pass, agent decision): that is
        // its rank behind the leader in a wing whose slots have no gaps, it
        // never shares a slot, and it never passes the wing's living members,
        // where its member number alone (which grows with every revival and
        // respawn) could pass slot 9 and fail the step.
        let (side, wing) = (actor.identity.side, actor.identity.wing);
        let leader = self
            .leaders
            .iter()
            .find(|l| l.side == side && l.wing == wing)
            .map(|l| l.leader);
        if let Some(leader) = leader {
            actor.set_leads(leader == actor.id());
        }
        if leader != Some(actor.id()) {
            let held: Vec<u8> = self
                .actors
                .iter()
                .filter(|a| a.identity.side == side && a.identity.wing == wing)
                .filter(|a| Some(a.id()) != leader && a.alive())
                .map(|a| a.wing_slot)
                .collect();
            let free = (1..=super::wing::MAX_WINGMAN_SLOT)
                .find(|slot| !held.contains(slot))
                .unwrap_or(super::wing::MAX_WINGMAN_SLOT);
            actor.wing_slot = free;
        }
        let index = self.actors.partition_point(|a| a.id() < actor.id());
        self.actors.insert(index, actor);
        Ok(())
    }

    /// The human-flown aircraft in wings, replacing the last set. A human
    /// leader remains a world object, never an AI-controlled actor; the
    /// wing's current leader is worked out from these and the actors.
    pub fn set_humans(&mut self, humans: Vec<HumanMember>) {
        self.humans = humans;
    }

    /// One human as a wing's first member, the way single player and the
    /// mission fixtures name a human leader.
    pub fn set_external_leader(&mut self, side: super::targeting::Side, wing: u8, id: u32) {
        self.humans
            .retain(|h| h.id != id && (h.side != side || h.wing != wing || h.member != 0));
        self.humans.push(HumanMember {
            id,
            side,
            wing,
            member: 0,
            pilot_alive: false,
        });
    }

    /// Hand the mission what the flight data link holds for the next step:
    /// the humans' locked targets (slice G2), which join the engagement table
    /// at the start of the step. The step consumes it, so the host sets it
    /// before every step; a step without it sees no human lock.
    pub fn set_link(&mut self, input: super::link::LinkInput) {
        self.link = input;
    }

    pub fn humans(&self) -> &[HumanMember] {
        &self.humans
    }

    /// The aircraft that leads a wing now: an actor or a human.
    pub fn wing_leader(&self, side: super::targeting::Side, wing: u8) -> Option<u32> {
        self.leaders
            .iter()
            .find(|l| l.side == side && l.wing == wing)
            .map(|l| l.leader)
    }

    /// Hand the mission the lead hold's claims (slice R2 of the lobby pass),
    /// replacing the last set: one per wing whose lead a human owns. The
    /// host sets them before every step while the hold is on; with none the
    /// succession rule is today's.
    pub fn set_lead_claims(&mut self, claims: Vec<LeadClaim>) {
        self.lead_claims = claims;
    }

    /// The lead hold's claims, as last set.
    pub fn lead_claims(&self) -> &[LeadClaim] {
        &self.lead_claims
    }

    /// Whether a wing's current leader stands in for its owner.
    pub fn wing_acting(&self, side: super::targeting::Side, wing: u8) -> bool {
        self.leaders
            .iter()
            .any(|l| l.side == side && l.wing == wing && l.acting)
    }

    /// The wingmen waiting to re-form on their owner once their fight is
    /// over.
    pub fn reform_after(&self) -> &[u32] {
        &self.reform_after
    }

    /// The human-flown aircraft that leads a wing now, if a human does.
    fn external_leader(&self, side: super::targeting::Side, wing: u8) -> Option<u32> {
        self.wing_leader(side, wing)
            .filter(|id| self.actor(*id).is_none())
    }

    /// Opinionated (requested by John, 2026-09-29): once a side has seen
    /// hostile aircraft and none is left alive, its AI aircraft have no
    /// mission left and return to base: each airborne AI aircraft that is not
    /// already landing, recovering or bugged out is ordered to land at its
    /// home runway through the ordinary landing sequence. A wing with no
    /// home runway flies the B48 return-to-base path to its launch point and
    /// holds there, the wingmen in formation on the leader. Nothing happens while any hostile aircraft
    /// remains, or while any aircraft on the side is still departing. A wing
    /// led by the player stays with the player, whose own mission may still be
    /// open. CAP remains on station, and a wing with unfinished strike work
    /// waits for its strike controller to finish the ground assignment.
    fn return_when_done(&mut self, world: &[WorldObject]) {
        let alive_on = |side: super::targeting::Side| {
            world
                .iter()
                .any(|o| o.is_aircraft && o.alive && !o.destroyed && o.side == side)
        };
        let mut sides: Vec<super::targeting::Side> = Vec::new();
        for actor in &self.actors {
            if !sides.contains(&actor.identity.side) {
                sides.push(actor.identity.side);
            }
        }
        for side in sides {
            let hostile = |other: super::targeting::Side| other != side;
            let hostiles_alive = world
                .iter()
                .any(|o| o.is_aircraft && o.alive && !o.destroyed && hostile(o.side));
            if hostiles_alive {
                if !self.hostiles_seen.contains(&side) {
                    self.hostiles_seen.push(side);
                }
                continue;
            }
            // A side with aircraft still departing has only just begun.
            // A member parked for good because its wing went home is not.
            let departing = self.actors.iter().any(|a| {
                a.identity.side == side
                    && a.alive()
                    && a.airfield.as_ref().is_some_and(|s| s.is_departure())
                    && !self.stood_down(a)
            });
            if !self.hostiles_seen.contains(&side) || !alive_on(side) || departing {
                continue;
            }
            // A wing led by a human stays with its human; an AI-led wing goes
            // home whoever flies in it (a human wingman is never ordered).
            let human_led: Vec<u8> = self
                .actors
                .iter()
                .filter(|a| a.identity.side == side)
                .map(|a| a.identity.wing)
                .filter(|wing| self.external_leader(side, *wing).is_some())
                .collect();
            // Air superiority does not finish a ground assignment. Keep the
            // whole wing together until its strike controller finishes.
            let striking_wings: Vec<u8> = self
                .actors
                .iter()
                .filter(|a| a.identity.side == side && a.strikes() && !a.strike_home())
                .map(|a| a.identity.wing)
                .collect();
            self.send_home(|a| {
                a.identity.side == side
                    && !human_led.contains(&a.identity.wing)
                    && !striking_wings.contains(&a.identity.wing)
                    && a.assignment.role != engagement::Role::CombatAirPatrol
            });
        }
    }

    /// Send home every airborne AI aircraft `pick` names that is not already
    /// taking off, landing, recovering or bugged out: one with a home runway
    /// is ordered to land there through the ordinary landing sequence, and a
    /// leader with none flies the B48 return-to-base path to its launch point
    /// and holds there, its wingmen in formation on it. Cheap to repeat every
    /// tick: an aircraft already on its way is left alone.
    fn send_home(&mut self, pick: impl Fn(&AiActor) -> bool) {
        let mut ids: Vec<u32> = Vec::new();
        for a in self.actors.iter_mut().filter(|a| {
            a.alive()
                && !a.identity.human_controlled
                && a.airfield.is_none()
                && a.landing_order.is_none()
                && !a.bugged_out
                && !a.flight.research.as_ref().is_some_and(|r| r.on_ground)
                && pick(a)
        }) {
            // Helicopters land too since slice H2.
            if a.home_runway.is_some() {
                ids.push(a.id());
            } else if a.identity.is_leader() && !a.controller.mission_complete() {
                // No runway: the leader flies home and holds; its
                // wingmen stay in formation on it.
                a.controller.set_mission_complete(true);
            }
        }
        for id in ids {
            let Some(runway) = self.actor(id).and_then(|a| a.home_runway) else {
                continue;
            };
            let _ = self.order(
                id,
                super::wing::WingRequest::Land(super::airfield::LandingOrder {
                    runway,
                    reason: super::airfield::LandingReason::Ordered,
                }),
            );
        }
    }

    /// The mission of opportunity of a wing whose human leader was lost, if
    /// it flies one.
    pub fn opportunity(
        &self,
        side: super::targeting::Side,
        wing: u8,
    ) -> Option<&super::opportunity::Opportunity> {
        self.opportunities
            .iter()
            .find(|o| o.side == side && o.wing == wing)
    }

    /// A member still waiting to take off when its wing has given up its
    /// mission of opportunity and gone home: it stays parked (agent decision,
    /// 2026-09-30), since it would only take off to land again.
    fn stood_down(&self, actor: &AiActor) -> bool {
        actor
            .airfield
            .as_ref()
            .is_some_and(|s| s.phase() == super::airfield::Phase::Waiting)
            && self
                .opportunity(actor.identity.side, actor.identity.wing)
                .is_some_and(|o| o.home.is_some())
    }

    /// The waypoints a wing still has to fly, in order, in world feet: the
    /// human leader's remaining route, or an AI wing's own. A wing whose
    /// human leader is lost flies them after its search for the enemy
    /// ([`super::opportunity`]). An empty list clears the route. Hosts with
    /// no mission route (Quick Mission today) never call this.
    pub fn set_wing_route(&mut self, side: super::targeting::Side, wing: u8, route: Vec<[f64; 3]>) {
        self.routes.retain(|(s, w, _)| (*s, *w) != (side, wing));
        if !route.is_empty() {
            self.routes.push((side, wing, route));
        }
    }

    /// The waypoints a wing still has to fly, empty when it has none.
    pub fn wing_route(&self, side: super::targeting::Side, wing: u8) -> &[[f64; 3]] {
        self.routes
            .iter()
            .find(|(s, w, _)| (*s, *w) == (side, wing))
            .map_or(&[], |(_, _, route)| route.as_slice())
    }

    /// Every wing's mission of opportunity, in the order they began.
    pub fn opportunities(&self) -> &[super::opportunity::Opportunity] {
        &self.opportunities
    }

    /// Opinionated (John, 2026-09-30): an AI aircraft took the lead of a wing
    /// from a lost human, so the wing takes up a mission of opportunity
    /// ([`super::opportunity`]). A wing cleared to attack flies it under free
    /// engagement: the lost leader's task (a patrol, an escort of the human,
    /// an intercept) and any recall it gave end with it. A wing under weapons
    /// hold or self-defense keeps its rules and returns to base at once. Agent
    /// decisions, in docs/spec/ai.md.
    fn begin_opportunity(&mut self, side: super::targeting::Side, wing: u8, leader: u32) {
        if self.opportunity(side, wing).is_some() {
            return;
        }
        let mut opportunity = super::opportunity::Opportunity::new(
            side,
            wing,
            self.tick,
            self.wing_route(side, wing).to_vec(),
        );
        let cleared = self.actor(leader).is_some_and(|a| {
            !matches!(
                a.assignment.stance,
                engagement::Stance::WeaponsHold | engagement::Stance::SelfDefense
            )
        });
        if cleared {
            let free = engagement::Assignment::default();
            for actor in self
                .actors
                .iter_mut()
                .filter(|a| a.identity.side == side && a.identity.wing == wing)
            {
                if actor.assignment != free {
                    actor.set_assignment(free.clone());
                }
                actor.formation_order_tick = None;
            }
        } else {
            opportunity.go_home(self.tick, super::opportunity::HomeReason::NotCleared);
        }
        self.opportunities.push(opportunity);
    }

    /// One tick of every wing's mission of opportunity: pool what the wing's
    /// members know of hostile aircraft, hand the leader the point to search,
    /// and send the wing home once the search is over.
    fn fly_opportunities(&mut self, world: &[WorldObject]) {
        use super::opportunity::{Plan, Sighting};
        for index in 0..self.opportunities.len() {
            let (side, wing) = (
                self.opportunities[index].side,
                self.opportunities[index].wing,
            );
            let Some(leader) = self
                .wing_leader(side, wing)
                .and_then(|id| self.actor(id))
                .filter(|a| a.alive())
            else {
                continue;
            };
            let leader_id = leader.id();
            let leader_position = leader.flight.position;
            let leader_engaged = leader.controller.target().is_some();
            let mut sightings: Vec<Sighting> = Vec::new();
            for member in self
                .actors
                .iter()
                .filter(|a| a.alive() && a.identity.side == side && a.identity.wing == wing)
            {
                let current = member.awareness.current_observations().map(|s| (s, true));
                let remembered = member.awareness.remembered().map(|s| (s, false));
                for (snapshot, current) in current.chain(remembered) {
                    let target = &snapshot.target;
                    if target.side == side || !target.is_aircraft || !target.valid {
                        continue;
                    }
                    sightings.push(Sighting {
                        id: target.id,
                        position: target.position,
                        observed_tick: snapshot.last_observed_tick,
                        current,
                    });
                }
            }
            let alive = |id: u32| world.iter().any(|o| o.id == id && o.alive && !o.destroyed);
            let plan = self.opportunities[index].step(
                self.tick,
                leader_position,
                &sightings,
                leader_engaged,
                &alive,
            );
            let point = match plan {
                Plan::Search(point) => Some(SearchContact {
                    id: point.id,
                    position: point.position,
                    observed_tick: point.observed_tick,
                }),
                Plan::Engage | Plan::Route(_) | Plan::Home(_) => None,
            };
            let waypoint = match plan {
                Plan::Route(point) => Some(point),
                _ => None,
            };
            for actor in self
                .actors
                .iter_mut()
                .filter(|a| a.identity.side == side && a.identity.wing == wing)
            {
                let leads = actor.id() == leader_id;
                actor.controller.set_wing_search(point.filter(|_| leads));
                actor.controller.set_wing_route(waypoint.filter(|_| leads));
            }
            if matches!(plan, Plan::Home(_)) {
                self.send_home(|a| a.identity.side == side && a.identity.wing == wing);
            }
        }
    }

    pub fn actors(&self) -> &[AiActor] {
        &self.actors
    }

    pub fn actors_mut(&mut self) -> &mut [AiActor] {
        &mut self.actors
    }

    pub fn actor(&self, id: u32) -> Option<&AiActor> {
        self.actors.iter().find(|a| a.id() == id)
    }

    pub fn actor_mut(&mut self, id: u32) -> Option<&mut AiActor> {
        self.actors.iter_mut().find(|a| a.id() == id)
    }

    /// Each actor's place in [`Self::actors`] by id, for many lookups in a
    /// row while no actor joins or leaves: the place [`Self::actor`] finds.
    pub fn actor_places(&self) -> Places {
        Places::of(self.actors.iter().map(AiActor::id))
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn is_empty(&self) -> bool {
        self.actors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.actors.len()
    }

    /// Advance every AI aircraft by one 120 Hz tick.
    ///
    /// `world` is one snapshot of every participant including the player and
    /// including the actors themselves; an actor never sees itself as a
    /// target. `ground` is the terrain height query. The mission steps each
    /// actor's own sensors, runs its controller, converts the resulting motion
    /// through the steering adapter and steps that actor's own flight model.
    pub fn step(
        &mut self,
        world: &[WorldObject],
        ground: &(dyn Fn(f64, f64) -> f64 + Sync),
        now: TimeOfDay,
    ) -> Result<MissionOutput> {
        self.step_with_surface(world, ground, &|x, z| Surface::terrain(ground(x, z)), now)
    }

    /// [`Self::step`] with the host's full surface query, so runways are
    /// landable for aircraft on the researched flight model. `terrain` is the
    /// plain terrain height: aircraft on the legacy adapter keep using it for
    /// their height above the ground, terrain floor and escape, as before.
    pub fn step_with_surface(
        &mut self,
        world: &[WorldObject],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> Surface + Sync),
        now: TimeOfDay,
    ) -> Result<MissionOutput> {
        self.step_on_ground(world, terrain, surface, &|x, z| surface(x, z).height, now)
    }

    /// [`Self::step_with_surface`] with the host's height-only query,
    /// `height(x, z)` being exactly `surface(x, z).height`. The AI's ground
    /// samples and sight lines read only the height, so a host that can
    /// answer it without the wind, water and slope saves that work.
    pub fn step_on_ground(
        &mut self,
        world: &[WorldObject],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> Surface + Sync),
        height: &(dyn Fn(f64, f64) -> f64 + Sync),
        now: TimeOfDay,
    ) -> Result<MissionOutput> {
        self.step_ground_using(world, terrain, surface, height, now, tore_workers::shared())
    }

    #[cfg(test)]
    fn step_using(
        &mut self,
        world: &[WorldObject],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> Surface + Sync),
        now: TimeOfDay,
        workers: &tore_workers::Executor,
    ) -> Result<MissionOutput> {
        self.step_ground_using(
            world,
            terrain,
            surface,
            &|x, z| surface(x, z).height,
            now,
            workers,
        )
    }

    fn step_ground_using(
        &mut self,
        world: &[WorldObject],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> Surface + Sync),
        height: &(dyn Fn(f64, f64) -> f64 + Sync),
        now: TimeOfDay,
        workers: &tore_workers::Executor,
    ) -> Result<MissionOutput> {
        let mut output = MissionOutput::default();
        self.return_when_done(world);
        let mut delivered = Vec::new();
        for (receiver, report) in std::mem::take(&mut self.pending_attack_reports) {
            if let Some(actor) = self.actor_mut(receiver) {
                let new = !actor.holds_attack(&report);
                actor.remember_attack(report);
                if new {
                    let outcome = actor
                        .attack_ignore_reason(&report)
                        .map_or(Outcome::Delivered, Outcome::Ignored);
                    delivered.push((receiver, report, outcome));
                }
            } else {
                delivered.push((
                    receiver,
                    report,
                    Outcome::Ignored(IgnoreReason::RecipientGone),
                ));
            }
        }
        for (receiver, attack, outcome) in delivered {
            self.journal.push(JournalEntry {
                tick: self.tick,
                sender: Some(attack.report.defended_id),
                message: Message::AttackEvidence(attack),
                receipts: vec![Receipt {
                    actor: receiver,
                    outcome,
                }],
            });
        }
        let tick = self.tick;
        self.track_airborne(world);
        // Lead passes on this tick, before anyone decides who to follow.
        self.refresh_leaders(world, &mut output);
        self.fly_opportunities(world);

        // One table for the tick, shared by every controller.
        let traffic: std::sync::Arc<[super::formation::Traffic]> = world
            .iter()
            .filter(|o| o.alive && !o.destroyed && o.is_aircraft)
            .map(|o| {
                let trace = self
                    .actors
                    .iter()
                    .find(|a| a.id() == o.id)
                    .and_then(|a| a.controller().formation_trace());
                super::formation::Traffic {
                    id: o.id,
                    position: o.position,
                    velocity: o.velocity,
                    planned_velocity: trace.and_then(|t| t.planned_velocity),
                    phase: trace.map(|t| t.phase),
                }
            })
            .collect();
        let mut observations = self
            .prepare_observations(world, terrain, height, workers)
            .into_iter()
            .peekable();
        // Who attacks what, in decision order (slice G1): each actor's row is
        // rewritten right after it steps, so an actor reads earlier actors'
        // targets of this tick and later actors' of the last.
        let link = std::mem::take(&mut self.link);
        let mut engagements =
            super::link::Engagements::new(&self.actors).with_humans(&self.humans, &link);
        // The AI leads that took a new target this tick (slice G4).
        let mut commits = Vec::new();
        // The tick's gun rounds, indexed once for every actor's incoming
        // fire, then handed back as the host set them.
        let gun_rounds = std::mem::take(&mut self.gun_rounds);
        let rounds = incoming_fire::Rounds::new(&gun_rounds);
        let mut failed = None;
        for index in 0..self.actors.len() {
            let observed = if observations.peek().is_some_and(|(at, _)| *at == index) {
                observations.next().map(|(_, observed)| observed)
            } else {
                None
            };
            let before = self.actors[index].controller.target();
            if let Err(error) = self.step_actor(
                index,
                world,
                &traffic,
                terrain,
                surface,
                height,
                &rounds,
                now,
                tick,
                observed,
                &engagements,
                &link,
                &mut output,
            ) {
                failed = Some(error);
                break;
            }
            engagements.decided(index, &self.actors[index]);
            let after = self.actors[index].controller.target();
            if self.actors[index].identity.is_leader() && after.is_some() && after != before {
                commits.push(index);
            }
            self.journal_actor(index, tick);
        }
        drop(rounds);
        self.gun_rounds = gun_rounds;
        if let Some(error) = failed {
            return Err(error);
        }

        // Share only evidence produced this tick, after all controllers have
        // run. Recipients consume it next tick regardless of actor order.
        let fresh_reports: Vec<_> = self
            .actors
            .iter()
            .flat_map(|actor| {
                actor
                    .observed_attacks
                    .iter()
                    .filter(move |attack| {
                        attack.observed_tick == tick && attack.report.defended_id == actor.id()
                    })
                    .map(move |attack| (actor.id(), *attack))
            })
            .collect();
        for (receiver, attack) in fresh_reports {
            self.report_attack_evidence(
                receiver,
                attack.report,
                attack.bearing_world_deg,
                attack.event_id,
            );
        }

        // A neutral AI leader may respond to a perceived attack or a current
        // hostile contact permitted by its mission. Recall disables contact-only
        // release; player-led wings still require an accepted player order.
        let releases: Vec<_> = self
            .actors
            .iter()
            .filter(|leader| {
                leader.alive()
                    && leader.identity.is_leader()
                    && leader.neutral
                    && leader.assignment.stance != engagement::Stance::WeaponsHold
                    && self
                        .external_leader(leader.identity.side, leader.identity.wing)
                        .is_none()
            })
            .filter_map(|leader| {
                leader
                    .observed_attacks
                    .iter()
                    .find(|attack| {
                        leader.permits_attack_response(attack)
                            && (leader
                                .assignment
                                .protected_ids
                                .contains(&attack.report.defended_id)
                                || self.actors.iter().any(|member| {
                                    member.id() == attack.report.defended_id
                                        && member.identity.side == leader.identity.side
                                        && member.identity.wing == leader.identity.wing
                                }))
                    })
                    .map(|attack| (leader.id(), Message::FreeSelection { trigger: *attack }))
                    .or_else(|| {
                        if leader.formation_order_tick.is_some()
                            || leader.bugged_out
                            || leader.is_dummy()
                        {
                            return None;
                        }
                        let targets: Vec<_> = leader
                            .awareness
                            .current_observations()
                            .map(|s| s.target)
                            .collect();
                        let protected: Vec<_> = world
                            .iter()
                            .filter(|o| {
                                o.side == leader.identity.side
                                    && leader.assignment.protected_ids.contains(&o.id)
                            })
                            .map(|o| engagement::ProtectedView {
                                id: o.id,
                                position: o.position,
                                velocity: o.velocity,
                                alive: o.alive && !o.destroyed,
                            })
                            .collect();
                        engagement::Policy::default()
                            .select(
                                leader.id(),
                                leader.identity.side,
                                leader.flight.position,
                                &leader.assignment,
                                &targets,
                                &protected,
                                &[],
                                None,
                                2,
                            )
                            .map(|selection| {
                                (
                                    leader.id(),
                                    Message::ContactSelection {
                                        target: selection.id,
                                    },
                                )
                            })
                    })
            })
            .collect();
        let automatic_leaders: Vec<_> = releases.iter().map(|(id, _)| *id).collect();
        for (leader, message) in releases {
            let request =
                super::wing::WingRequest::TargetAssignment(super::wing::TargetOrder::FreeSelection);
            let outcome = self.order(leader, request).expect("live leader")?;
            output.wing.push((leader, request));
            self.journal.push(JournalEntry {
                tick,
                sender: Some(leader),
                message,
                receipts: vec![Receipt {
                    actor: leader,
                    outcome: Outcome::Order(outcome),
                }],
            });
        }

        // Deliver after all actors have decided, so iteration order cannot
        // change which wing members observe a new command this tick.
        for (sender, request) in &output.wing {
            if let Some(actor) = self.actor(*sender).filter(|a| a.alive()) {
                let identity = *actor.identity();
                if automatic_leaders.contains(sender)
                    && matches!(
                        request,
                        super::wing::WingRequest::TargetAssignment(
                            super::wing::TargetOrder::FreeSelection
                        )
                    )
                {
                    // A leader's automatic release must not cancel an explicit
                    // assignment already accepted by an individual wingman.
                    let recipients: Vec<_> = self
                        .actors
                        .iter()
                        .filter(|a| {
                            a.id() != *sender
                                && a.identity.side == identity.side
                                && a.identity.wing == identity.wing
                                && a.neutral
                        })
                        .map(AiActor::id)
                        .collect();
                    for id in recipients {
                        self.order_wing_report(
                            identity.side,
                            identity.wing,
                            Some(*sender),
                            Some(id),
                            *request,
                        )?;
                    }
                } else {
                    self.order_wing(identity.side, identity.wing, Some(*sender), *request)?;
                }
            }
        }
        // A lead that took a new target gives its wingmen targets (slice
        // G4), after the automatic releases above so a wing released this
        // tick can take them.
        self.lead_commits(&commits, &link, &engagements, world, &mut output);
        // Strikers with no target or store left go home (slice ST-T).
        self.strikers_home();
        // An aircraft lost or abandoned during the tick hands over lead on
        // the same tick.
        self.refresh_leaders(world, &mut output);
        self.tick += 1;
        Ok(output)
    }

    /// Whether wing member `id` is still flying: an actor is alive, and a
    /// human-flown aircraft is alive in the world snapshot. An aircraft that
    /// crashed, was destroyed or whose pilot ejected is not.
    fn member_flying(&self, id: u32, world: &[WorldObject]) -> bool {
        match self.actor(id) {
            Some(actor) => actor.alive(),
            None => world.iter().any(|o| o.id == id && o.alive && !o.destroyed),
        }
    }

    /// Give every wing its current leader and pass the lead on when it has
    /// been lost.
    ///
    /// At the start a wing's leader is its member 0. When the leader's
    /// aircraft is destroyed or its pilot ejects, lead passes to the
    /// lowest-numbered living human member of the wing, or, if there is none,
    /// to the lowest-numbered living AI member, and the flight re-forms on the
    /// new leader: its followers take formation slots 1, 2 and so on in
    /// member order. A wing with nobody left keeps its last leader.
    ///
    /// Under the lead hold (slice R2 of the lobby pass, John 2026-10-09) a
    /// wing with a claim ([`Self::set_lead_claims`]) also crowns its claimed
    /// plane whenever it flies and does not lead ([`Self::reclaim`]), and a
    /// leader other than the claimed plane is a stand-in (`acting`). With no
    /// claims this is today's rule exactly.
    fn refresh_leaders(&mut self, world: &[WorldObject], output: &mut MissionOutput) {
        self.humans_flying = self
            .humans
            .iter()
            .filter(|h| self.member_flying(h.id, world))
            .map(|h| h.id)
            .collect();
        self.reform_finished();
        let mut wings: Vec<(super::targeting::Side, u8)> = Vec::new();
        for (side, wing) in self
            .actors
            .iter()
            .map(|a| (a.identity.side, a.identity.wing))
            .chain(self.humans.iter().map(|h| (h.side, h.wing)))
        {
            if !wings.contains(&(side, wing)) {
                wings.push((side, wing));
            }
        }
        for (side, wing) in wings {
            // Every member of the wing as (member, id, human).
            let mut members: Vec<(u8, u32, bool)> = self
                .actors
                .iter()
                .filter(|a| a.identity.side == side && a.identity.wing == wing)
                .map(|a| (a.identity.member, a.id(), false))
                .chain(
                    self.humans
                        .iter()
                        .filter(|h| h.side == side && h.wing == wing)
                        .map(|h| (h.member, h.id, true)),
                )
                .collect();
            members.sort_unstable();
            let mut current = self.wing_leader(side, wing);
            if current.is_none() {
                // A wing starts under its first member. One without a member
                // 0 has no leader, as before.
                let Some(&(_, first, _)) = members.iter().find(|m| m.0 == 0) else {
                    continue;
                };
                self.leaders.push(WingLeader {
                    side,
                    wing,
                    leader: first,
                    reformed: false,
                    acting: false,
                });
                self.crown(side, wing, first, &members, world, false);
                current = Some(first);
            }
            let Some(previous) = current else { continue };
            let claim = self
                .lead_claims
                .iter()
                .find(|c| c.side == side && c.wing == wing)
                .copied();
            // The lead hold: the owner's plane flies again and does not
            // lead, so it takes the lead back.
            if let Some(claim) = claim
                && let Some(owner) = claim.plane
                && owner != previous
                && members.iter().any(|m| m.1 == owner)
                && self.member_flying(owner, world)
            {
                self.reclaim(
                    side,
                    wing,
                    owner,
                    previous,
                    claim.fresh,
                    &members,
                    world,
                    output,
                );
                continue;
            }
            if self.member_flying(previous, world) {
                // A leader other than the owner's plane stands in for it.
                let acting = claim.is_some_and(|c| c.plane != Some(previous));
                if let Some(entry) = self
                    .leaders
                    .iter_mut()
                    .find(|l| l.side == side && l.wing == wing)
                {
                    entry.acting = acting;
                }
                continue;
            }
            let flying = |&&(_, id, _): &&(u8, u32, bool)| self.member_flying(id, world);
            let successor = members
                .iter()
                .filter(flying)
                .find(|m| m.2)
                .or_else(|| members.iter().filter(flying).find(|m| !m.2))
                .map(|m| m.1);
            let Some(leader) = successor else { continue };
            let previous_pilot_alive = self.pilot_alive(previous);
            // A successor chosen while the owner's plane does not fly stands
            // in for the owner (the lead hold).
            let acting = claim.is_some_and(|c| c.plane != Some(leader));
            if let Some(entry) = self
                .leaders
                .iter_mut()
                .find(|l| l.side == side && l.wing == wing)
            {
                entry.leader = leader;
                entry.reformed = true;
                entry.acting = acting;
            }
            self.crown(side, wing, leader, &members, world, true);
            // A human's lead passed to an AI aircraft: the wing flies a
            // mission of opportunity (John, 2026-09-30).
            if self.actor(previous).is_none() && self.actor(leader).is_some() {
                self.begin_opportunity(side, wing, leader);
            }
            output.leadership.push(LeadershipChange {
                tick: self.tick,
                side,
                wing,
                leader,
                previous,
                previous_pilot_alive,
                acting,
                reclaimed: false,
            });
        }
    }

    /// Whether the pilot of the leader `previous` is alive as it stops
    /// leading: an AI pilot escaping alive, or a human's ejected unhurt.
    fn pilot_alive(&self, previous: u32) -> bool {
        match self.actor(previous) {
            Some(actor) => actor.flight.escape.is_some() && !actor.flight.systems.pilot.dead,
            None => self
                .humans
                .iter()
                .find(|h| h.id == previous)
                .is_some_and(|h| h.pilot_alive),
        }
    }

    /// The lead hold gives a wing's lead to its owner's flying plane `owner`
    /// (slice R2 of the lobby pass; John, 2026-10-09): the wing's mission of
    /// opportunity ends, the flight re-forms on the owner in member order,
    /// and each AI wingman flies back into formation, except one fighting,
    /// which re-forms once its fight is over ([`Self::reform_finished`]).
    /// A wingman landing, on the ground or bugged out is left to it. The
    /// change says whether the owner led the wing before (`!fresh`).
    #[allow(clippy::too_many_arguments)]
    fn reclaim(
        &mut self,
        side: super::targeting::Side,
        wing: u8,
        owner: u32,
        previous: u32,
        fresh: bool,
        members: &[(u8, u32, bool)],
        world: &[WorldObject],
        output: &mut MissionOutput,
    ) {
        let previous_pilot_alive = self.pilot_alive(previous);
        if let Some(entry) = self
            .leaders
            .iter_mut()
            .find(|l| l.side == side && l.wing == wing)
        {
            entry.leader = owner;
            entry.reformed = true;
            entry.acting = false;
        }
        self.crown(side, wing, owner, members, world, true);
        self.opportunities
            .retain(|o| o.side != side || o.wing != wing);
        let tick = self.tick;
        let mut fighting = Vec::new();
        for actor in self.actors.iter_mut().filter(|a| {
            a.identity.side == side && a.identity.wing == wing && a.id() != owner && a.alive()
        }) {
            if actor.bugged_out || actor.landing_order.is_some() || actor.airfield.is_some() {
                continue;
            }
            if actor.controller.target().is_some() {
                fighting.push(actor.id());
            } else {
                actor.return_to_formation(tick);
            }
        }
        for id in fighting {
            if !self.reform_after.contains(&id) {
                self.reform_after.push(id);
            }
        }
        output.leadership.push(LeadershipChange {
            tick,
            side,
            wing,
            leader: owner,
            previous,
            previous_pilot_alive,
            acting: false,
            reclaimed: !fresh,
        });
    }

    /// Each wingman that was fighting when its owner took the lead back
    /// re-forms once its fight is over, while the owner still leads; one
    /// lost, bugged out, or whose owner no longer leads is let go.
    fn reform_finished(&mut self) {
        if self.reform_after.is_empty() {
            return;
        }
        let tick = self.tick;
        for id in std::mem::take(&mut self.reform_after) {
            let Some(index) = self.actors.iter().position(|a| a.id() == id) else {
                continue;
            };
            let (side, wing) = (
                self.actors[index].identity.side,
                self.actors[index].identity.wing,
            );
            let owner_leads =
                self.leaders
                    .iter()
                    .find(|l| l.side == side && l.wing == wing)
                    .is_some_and(|l| {
                        !l.acting
                            && self.lead_claims.iter().any(|c| {
                                c.side == side && c.wing == wing && c.plane == Some(l.leader)
                            })
                    });
            let actor = &mut self.actors[index];
            if !owner_leads || !actor.alive() || actor.bugged_out {
                continue;
            }
            if actor.controller.target().is_some() {
                self.reform_after.push(id);
            } else {
                actor.return_to_formation(tick);
            }
        }
    }

    /// Mark `leader` as its wing's leader on every actor, and, when the wing
    /// re-forms, hand each flying follower its slot in member order.
    fn crown(
        &mut self,
        side: super::targeting::Side,
        wing: u8,
        leader: u32,
        members: &[(u8, u32, bool)],
        world: &[WorldObject],
        reform: bool,
    ) {
        let followers: Vec<u32> = members
            .iter()
            .filter(|m| m.1 != leader && self.member_flying(m.1, world))
            .map(|m| m.1)
            .collect();
        for actor in self
            .actors
            .iter_mut()
            .filter(|a| a.identity.side == side && a.identity.wing == wing)
        {
            actor.set_leads(actor.id() == leader);
            if reform && let Some(index) = followers.iter().position(|id| *id == actor.id()) {
                // Never past the last formation slot (slice R2: a wing that
                // respawns can hold more members than the slots).
                actor.wing_slot = (index.min(255) as u8)
                    .saturating_add(1)
                    .min(super::wing::MAX_WINGMAN_SLOT);
            }
            // A member that was following the lost leader in to land stops
            // (the bug bash's rule of 2026-09-29), as the wing abort does: the
            // new leader does not keep landing on the old one's order.
            if reform
                && actor.alive()
                && actor
                    .landing_order
                    .is_some_and(|o| o.reason == super::airfield::LandingReason::JoinLeader)
                && matches!(
                    actor.airfield_phase(),
                    Some(
                        super::airfield::Phase::Inbound
                            | super::airfield::Phase::Marshal
                            | super::airfield::Phase::Approach
                    )
                )
            {
                actor.landing_order = None;
                actor.leave_airfield();
            }
        }
    }

    fn prepare_observations(
        &self,
        world: &[WorldObject],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        height: &(dyn Fn(f64, f64) -> f64 + Sync),
        workers: &tore_workers::Executor,
    ) -> Vec<(usize, observation::Prepared)> {
        if !workers.should_dispatch(self.actors.len(), MIN_PARALLEL_OBSERVATIONS) {
            return Vec::new();
        }
        let test = observation::ObservationTest::of(self.test);
        // Only the actors whose turn it is observe this tick; the others hold
        // what they last observed (see `AiActor::hold_observation`).
        let inputs: Vec<_> = self
            .actors
            .iter()
            .enumerate()
            .filter(|(index, _)| self.observation_due(*index, self.tick))
            .filter_map(|(index, actor)| actor.observation_input(index))
            .map(|input| observation::Input {
                ceiling: self.ground_ceiling.as_deref(),
                context: observation::Context {
                    test,
                    ..input.context
                },
                ..input
            })
            .collect();
        if !workers.should_dispatch(inputs.len(), MIN_PARALLEL_OBSERVATIONS) {
            return Vec::new();
        }
        let tick = self.tick;
        workers.ordered_map(&inputs, MIN_PARALLEL_OBSERVATIONS, |_, input| {
            (input.index, input.prepare(tick, world, terrain, height))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn step_actor(
        &mut self,
        index: usize,
        world: &[WorldObject],
        traffic: &std::sync::Arc<[super::formation::Traffic]>,
        terrain: &dyn Fn(f64, f64) -> f64,
        surface: &dyn Fn(f64, f64) -> Surface,
        height: &dyn Fn(f64, f64) -> f64,
        rounds: &incoming_fire::Rounds<'_>,
        now: TimeOfDay,
        tick: u64,
        observed: Option<observation::Prepared>,
        engagements: &super::link::Engagements,
        link: &super::link::LinkInput,
        output: &mut MissionOutput,
    ) -> Result<()> {
        // Researched aircraft stand on runways, so their ground is the full
        // surface's height; legacy aircraft keep the terrain-only height.
        let ground: &dyn Fn(f64, f64) -> f64 = if self.actors[index].flight.research.is_some() {
            height
        } else {
            terrain
        };
        let actor_id = self.actors[index].id();
        let test = self.test;
        let observation_due = self.observation_due(index, tick);
        // The host's strike inputs are this step's only (slice ST-T).
        let strike_inputs = self.actors[index].take_strike_inputs();
        let strike_mates = self.strike_mates(index);
        let strike_wing = self.strike_wing(index);
        let mut leader = self.leader_view(index, world, engagements);
        let clearance = self.airfield_clearance(index, world);
        if leader.as_ref().is_none_or(|l| !l.recovering) {
            self.actors[index].join_cancelled = false;
        }
        let join = self.join_landing(index, leader.as_ref());
        // Fitted: a wingman does not formate on a human leader parked on the
        // ground; it flies free until the leader is airborne again.
        if leader.is_some_and(|l| l.on_ground) {
            leader = None;
        }

        let identity = self.actors[index].identity;
        // The other wing members' targets (B41), from the engagement table.
        let assignments = engagements.wing_targets(actor_id, identity.side, identity.wing);
        if super::link::audit::active() {
            let humans = engagements.human_wing_targets(identity.side, identity.wing);
            super::link::audit::wing(&self.actors, index, &assignments, &humans);
        }
        let actor = &mut self.actors[index];
        // Write-only records start afresh on every mission step.
        actor.trace = ActorTrace::begin(tick);
        actor.adapter.set_gun_aim(None);
        actor.adapter.set_defense(None);
        actor
            .adapter
            .set_strike_handling(super::strike::Handling::PLAIN);
        actor.trace.clearance = clearance;
        actor.route_random.clear_log();
        // Every controller shares the tick's one traffic list.
        actor.controller.set_formation_observation(traffic.clone());
        if !actor.dummy {
            if actor.flight.escape.is_some() {
                actor.flight.step_escape(ground);
                actor.trace.ejection = Some(EjectionTrace {
                    escape_running: true,
                    ..EjectionTrace::default()
                });
            } else {
                let mut assessment = super::damage::fire_assessment(
                    &actor.flight,
                    ground(actor.flight.position[0], actor.flight.position[2]),
                )
                .or_else(|| crate::ejection::assess(&actor.flight, ground));
                let mut ejection = EjectionTrace {
                    assessment,
                    ..EjectionTrace::default()
                };
                // Opinionated (John, 2026-09-23): taking off or landing, only
                // a catastrophe ejects; any other hazard aborts the landing.
                if let (Some(found), Some(sequence)) = (assessment, actor.airfield.as_mut())
                    && sequence.guards_ejection()
                {
                    ejection.guarded_phase = Some(sequence.phase());
                    let [x, y, z] = actor.flight.position;
                    let below = surface(x, z);
                    let landing = sequence.landing_point();
                    let field = crate::ejection::AirfieldContext {
                        agl_ft: y - below.height,
                        below,
                        landing_distance_ft: (landing[0] - x).hypot(landing[2] - z),
                    };
                    let catastrophic = crate::ejection::catastrophic(&actor.flight, found, field);
                    ejection.catastrophic = Some(catastrophic);
                    if !catastrophic {
                        // Normal landing geometry (a low, sinking final over
                        // the runway) needs nothing; a final that would touch
                        // down off the runway or gear up goes around; any
                        // hazard on the gates or at marshal aborts too. The
                        // takeoff carries on at full power.
                        let reach = found.impact_seconds.min(10.);
                        let touchdown = surface(
                            x + actor.flight.velocity[0] * reach,
                            z + actor.flight.velocity[2] * reach,
                        );
                        let abort = match sequence.phase() {
                            super::airfield::Phase::Marshal | super::airfield::Phase::Approach => {
                                true
                            }
                            super::airfield::Phase::Final => {
                                let descent = (-actor.flight.velocity[1])
                                    .atan2(actor.flight.velocity[0].hypot(actor.flight.velocity[2]))
                                    .to_degrees();
                                crate::touchdown::assess(&actor.flight, &touchdown).is_refused()
                                    || actor.flight.gear < 0.99
                                    || descent > super::airfield::GO_AROUND_DESCENT_DEG
                            }
                            _ => false,
                        };
                        ejection.go_around = Some(abort);
                        if abort {
                            sequence.request_go_around();
                        }
                        assessment = None;
                    }
                }
                if actor.escape_monitor.step(assessment).is_some() {
                    actor.flight.eject();
                    ejection.ejected = true;
                }
                actor.trace.ejection = Some(ejection);
            }
        }

        if !actor.alive() {
            actor.awareness.clear();
            actor.search_target = None;
            actor.controller.set_search_contact(None);
            actor.activity = Activity::Destroyed;
            output.activities.push((actor_id, Activity::Destroyed));
            actor.trace.path = ActorPath::Destroyed;
            return Ok(());
        }

        if actor.dummy {
            for i in 0..3 {
                actor.flight.position[i] += actor.flight.velocity[i] * flight::DT;
            }
            actor.flight.ticks += 1;
            actor.trace.dropped_warnings = actor
                .pending_threats
                .iter()
                .map(|report| (*report, DropReason::TrainingTarget))
                .collect();
            actor.pending_threats.clear();
            actor.pending_events.clear();
            actor.activity = Activity::Idle;
            output.activities.push((actor_id, Activity::Idle));
            actor.trace.path = ActorPath::Dummy;
            return Ok(());
        }

        let response =
            super::damage::Response::assess(&actor.flight.systems, actor.flight.damage_fraction);
        if actor.damage_return.is_none() && response.reason.is_some() {
            actor.damage_return = response.reason;
            actor.controller.return_to_formation();
            actor.search_target = None;
            actor.mission_policy.reset();
            if let Some(sensors) = &mut actor.sensors {
                sensors.clear_selection();
            }
        }
        actor
            .controller
            .set_damage_recovery(actor.damage_return.is_some());
        actor.trace.damage = super::damage::Trace {
            response,
            returning: actor.damage_return,
            home_known: actor.home().is_some() || actor.landing_order.is_some(),
            runway_known: actor.home_runway.is_some() || actor.landing_order.is_some(),
            ..Default::default()
        };
        if actor.damage_return.is_some() {
            if actor.landing_order.is_none()
                && let Some(runway) = actor.home_runway
            {
                actor.landing_order = Some(super::airfield::LandingOrder {
                    runway,
                    reason: super::airfield::LandingReason::Damage,
                });
            }
            let on_ground = actor.flight.research.as_ref().is_some_and(|r| r.on_ground);
            if on_ground
                && !matches!(
                    actor.airfield_phase(),
                    Some(
                        super::airfield::Phase::Final
                            | super::airfield::Phase::Rollout
                            | super::airfield::Phase::TaxiClear
                            | super::airfield::Phase::Parked
                            | super::airfield::Phase::VerticalDescent
                            | super::airfield::Phase::Settle
                            | super::airfield::Phase::AirTaxi
                    )
                )
            {
                actor.trace.damage.ground_hold = true;
                actor.fly_input(
                    PilotInput {
                        throttle: Some(0.),
                        commands: vec![
                            tore_input::PilotCommand::Set(tore_input::Switch::Burner, false),
                            tore_input::PilotCommand::Set(tore_input::Switch::Airbrake, true),
                        ],
                        ..Default::default()
                    },
                    ground,
                    surface,
                );
                actor.activity = Activity::Waiting;
                actor.trace.path = ActorPath::DamageHold;
                output.activities.push((actor_id, Activity::Waiting));
                return Ok(());
            }
        }

        // 1. The actor's own sensors, stepped with the actor as observer.
        // Observation and the decision frame below share one answer per
        // sight line from the actor this tick.
        let sight = observation::SightLines::new(
            actor.flight.position,
            ground,
            self.ground_ceiling.as_deref(),
        );
        let (mut targets, terrain_blocked) = if let Some(prepared) = observed {
            actor.sensors = prepared.sensors;
            actor.awareness = prepared.memory;
            actor.trace.visual = prepared.visual;
            (
                actor.apply_observation(prepared.observed),
                Some(prepared.terrain_blocked),
            )
        } else if observation_due {
            (actor.observe_sighted_with(tick, world, &sight, test), None)
        } else {
            (actor.hold_observation(tick, world), None)
        };
        if observation_due && test.divisor() > 1 {
            actor.last_visual.clone_from(&actor.trace.visual);
        }

        // 2. Own state from the actor's own flight model.
        let own = actor.own_state(ground);
        actor.update_defense(
            tick,
            &self.missiles,
            rounds,
            self.ground_ceiling.as_deref(),
            &own,
            ground,
        );

        // Takeoff and landing sequences replace combat and formation flying.
        if actor.landing_order.is_none()
            && let Some(order) = join
        {
            actor.landing_order = Some(order);
            actor.trace.join_landing = Some(order);
        }
        if actor.airfield_tick(&clearance, &own, tick, ground, surface, height, output)? {
            actor.trace.path = ActorPath::Airfield;
            // A damaged or bingo pilot flying a landing sheds its load on the
            // way home, never on the ground (docs/spec/jettison.md).
            if !actor.flight.research.as_ref().is_some_and(|r| r.on_ground)
                && let Some(drop) = super::jettison::decide(actor.load_situation())
            {
                output.jettison.push((actor_id, drop));
            }
            return Ok(());
        }
        actor.trace.path = ActorPath::Controller;

        // 3. The frame.
        let events = actor.drain_events(tick);
        for (index, target) in targets.iter_mut().enumerate() {
            target.wing_attackers =
                assignments.iter().filter(|id| **id == target.id).count() as u32;
            // OwnState copied the same pre-flight position as the worker's
            // observer. The preamble paths that move it returned before here.
            // Publish this only to the decision frame, never awareness memory.
            target.terrain_blocked = terrain_blocked.as_ref().map_or_else(
                || sight.blocked(own.position, target.position),
                |blocked| blocked[index],
            );
        }
        let mut expired = Vec::new();
        actor.observed_attacks.retain(|attack| {
            let keep = tick.saturating_sub(attack.observed_tick) < 240
                && attack
                    .report
                    .attacker_id
                    .is_none_or(|id| world.iter().any(|o| o.id == id && o.alive && !o.destroyed));
            if !keep {
                expired.push(*attack);
            }
            keep
        });
        actor.trace.expired_attacks = expired
            .into_iter()
            .map(|attack| {
                let age = tick.saturating_sub(attack.observed_tick);
                let reason = if age >= 240 {
                    ExpiryReason::Age { ticks: age }
                } else {
                    ExpiryReason::AttackerGone
                };
                (attack, reason)
            })
            .collect();
        // The ids of the emitters heard, asked for by the first warning that
        // needs them: the list of the last observation, or with lean sensors
        // one worked out now.
        let mut emitter_ids: Option<Vec<u32>> = None;
        let incoming: Vec<_> = actor
            .missile_threats
            .records()
            .filter(|r| !r.stale && r.targeting_receiver)
            .copied()
            .collect();
        for record in incoming {
            let attacker_id =
                if record.source == crate::combat::threats::EvidenceSource::ElectronicSupported {
                    record.radar_bearing_deg.and_then(|bearing| {
                        let heard = emitter_ids
                            .get_or_insert_with(|| actor.emitter_ids(world, &sight, test));
                        engagement::identify_supporting_attacker(
                            own.position,
                            own.heading_deg,
                            bearing,
                            &targets,
                            heard,
                            identity.side,
                        )
                    })
                } else {
                    None
                };
            actor.remember_attack(ObservedAttack {
                report: engagement::ThreatReport {
                    attacker_id,
                    defended_id: actor_id,
                },
                bearing_world_deg: Some(
                    (own.heading_deg + record.radar_bearing_deg.unwrap_or(record.bearing_deg))
                        .rem_euclid(360.),
                ),
                observed_tick: tick,
                event_id: Some(record.missile_id),
            });
        }
        // Slice G3b: an aircraft a lead assigned this actor, that its own
        // sensors do not hold, is flown toward on a flightmate's track. The
        // view joins the targets the engagement policy and the controller
        // choose from, flagged so the weapons never fire on it; everything
        // that reads the actor's own sensors keeps the observed targets.
        let observed_len = targets.len();
        if !actor.neutral
            && actor.damage_return.is_none()
            && let Some(pursuit) = link.pursuit_of(actor_id)
            && actor.controller.ordered_target() == Some(pursuit.target)
            && !targets.iter().any(|t| t.id == pursuit.target)
            && let Some(object) = world.iter().find(|o| o.id == pursuit.target)
            && let Some(view) = pursuit.view(
                tick,
                identity.side,
                object,
                actor.seeker_eligible(true),
                assignments
                    .iter()
                    .filter(|id| **id == pursuit.target)
                    .count() as u32,
            )
        {
            targets.push(view);
        }
        let reports: Vec<_> = actor
            .observed_attacks
            .iter()
            .filter(|attack| actor.permits_attack_response(attack))
            .map(|a| a.report)
            .collect();
        let protected: Vec<_> = world
            .iter()
            .filter(|object| {
                object.side == identity.side && actor.assignment.protected_ids.contains(&object.id)
            })
            .map(|object| engagement::ProtectedView {
                id: object.id,
                position: object.position,
                velocity: object.velocity,
                alive: object.alive && !object.destroyed,
            })
            .collect();
        let neutral_assignment = engagement::Assignment {
            role: engagement::Role::Disengage,
            stance: if actor.assignment.stance == engagement::Stance::WeaponsHold {
                engagement::Stance::WeaponsHold
            } else {
                engagement::Stance::SelfDefense
            },
            ..engagement::Assignment::default()
        };
        let damage_assignment = engagement::Assignment {
            role: engagement::Role::Disengage,
            stance: engagement::Stance::WeaponsHold,
            ..Default::default()
        };
        let permission = if actor.damage_return.is_some() {
            &damage_assignment
        } else if actor.neutral {
            &neutral_assignment
        } else {
            &actor.assignment
        };
        // Slice G4: a bandit this actor yielded to a flightmate is left out
        // of the choice while it has another to take.
        actor.yields.retain(|y| y.until > tick);
        for held in actor.yields.iter_mut().filter(|y| !y.announced) {
            held.announced = true;
            output.link.push(super::link::LinkEvent::Yield {
                actor: actor_id,
                target: held.target,
            });
        }
        let yielded: Vec<u32> = actor.yields.iter().map(|y| y.target).collect();
        let others: Vec<TargetView> = targets
            .iter()
            .filter(|t| !yielded.contains(&t.id))
            .copied()
            .collect();
        let current = actor.controller.target();
        // The selection and its explanation come from one pass over the
        // list the selection was made from (the explanation reads the leash
        // state that selection just updated).
        let mut chosen = None;
        if !yielded.is_empty() {
            let leaving = current.filter(|id| !yielded.contains(id));
            let (selection, explanation) = actor.mission_policy.select_explained(
                actor_id,
                identity.side,
                own.position,
                permission,
                &others,
                &protected,
                &reports,
                leaving,
                2,
            );
            if selection.is_some() {
                chosen = Some((selection, explanation));
            }
        }
        let (selection, explanation) = match chosen {
            Some(chosen) => chosen,
            None => actor.mission_policy.select_explained(
                actor_id,
                identity.side,
                own.position,
                permission,
                &targets,
                &protected,
                &reports,
                current,
                2,
            ),
        };
        let must_rejoin = actor.mission_policy.must_rejoin();
        actor.trace.engagement = Some(EngagementTrace {
            neutral: actor.neutral,
            role: permission.role,
            stance: permission.stance,
            reports: actor
                .observed_attacks
                .iter()
                .map(|attack| ReportTrace {
                    attack: *attack,
                    ignored: actor.attack_ignore_reason(attack),
                })
                .collect(),
            selection,
            must_rejoin,
            explanation,
        });
        actor.controller.set_mission_hold_fire(
            actor.damage_return.is_some()
                || actor.assignment.stance == engagement::Stance::WeaponsHold,
        );
        actor.controller.set_mission_target(selection.map(|s| s.id));
        let rejoin = if must_rejoin {
            protected
                .iter()
                .filter(|p| p.alive)
                .min_by(|a, b| {
                    distance(own.position, a.position)
                        .total_cmp(&distance(own.position, b.position))
                })
                .map(|p| p.position)
        } else if actor.assignment.role == engagement::Role::CombatAirPatrol
            && selection.is_none()
            && (!actor.neutral
                || (actor.identity.is_leader() && actor.formation_order_tick.is_none()))
        {
            actor
                .assignment
                .patrol
                .filter(|region| !region.contains(own.position))
                .map(|region| region.center_ft)
        } else {
            None
        };
        actor.controller.set_mission_rejoin(rejoin);
        actor.trace.rejoin = rejoin.map(|point| RejoinTrace {
            point,
            reason: if must_rejoin {
                RejoinReason::EscortLeash
            } else {
                RejoinReason::OutsidePatrol
            },
        });
        let cue = (rejoin.is_none()
            && selection.is_none()
            && !actor.neutral
            && actor.assignment.stance == engagement::Stance::ProtectAssigned)
            .then(|| {
                actor
                    .observed_attacks
                    .iter()
                    .filter(|attack| {
                        actor
                            .assignment
                            .protected_ids
                            .contains(&attack.report.defended_id)
                            && attack.report.attacker_id.is_none()
                    })
                    .max_by_key(|attack| attack.observed_tick)
                    .and_then(|attack| attack.bearing_world_deg)
            })
            .flatten();
        actor.controller.set_mission_search_bearing(cue);
        actor.trace.search_cue_deg = cue;
        actor.update_search_contact(&protected);
        let gun_target = selection
            .and_then(|s| {
                actor
                    .awareness
                    .current_observations()
                    .find(|o| o.target.id == s.id)
            })
            .map(|s| super::gunnery::Target {
                id: s.target.id,
                position: awareness::held_position(s, tick),
                velocity: s.velocity,
                basis: world
                    .iter()
                    .find(|o| o.id == s.target.id)
                    .and_then(|o| o.observable.as_ref())
                    .map(|o| o.basis)
                    .unwrap_or(crate::attitude::Basis::new(
                        s.target.heading_deg.to_radians(),
                        s.target.pitch_deg.to_radians(),
                        0.,
                    )),
            });
        actor.gun_target = gun_target;
        let launcher = crate::combat::live::Launcher {
            position: actor.flight.position,
            basis: crate::attitude::Basis::new(
                actor.flight.yaw,
                actor.flight.pitch,
                actor.flight.bank,
            ),
            speed_fps: actor.flight.speed,
            velocity: actor.flight.velocity,
            radar: actor.flight.radar,
            radar_power: actor.flight.radar,
            alive: actor.alive(),
            body_present: !actor.flight.wreck_gone(),
            bay_ready: true,
            jammer: false,
            controls: crate::sensors::Controls::default(),
        };
        let gun_views = actor
            .guns
            .iter()
            .filter_map(|(index, weapon)| {
                let station = actor.stations.iter().find(|s| s.station.0 == *index)?;
                Some(super::gunnery::View {
                    station: station.station,
                    ammunition: station.store.rounds,
                    target: gun_target.map(|t| t.id),
                    solution: gun_target
                        .and_then(|t| super::gunnery::solve(weapon, &launcher, station.mount, t)),
                    rounds: u64::from(weapon.burst.actual_rounds_per_game.max(1))
                        * u64::from(weapon.burst.game_rounds_in_burst.max(1)),
                    period_ticks: u64::from(weapon.burst.game_burst_t.max(1)) * 30,
                    target_speed: gun_target
                        .map_or(0., |t| crate::combat::missiles::length(t.velocity)),
                })
            })
            .collect();
        actor.controller.set_gun_views(gun_views);
        // A striker's pass, flown below a B47 defence (slice ST-T).
        let strike = actor.strike_step(
            tick,
            &own,
            strike_inputs,
            &strike_mates,
            strike_wing,
            terrain,
        );
        actor.controller.set_strike(strike);
        let stations = actor.station_views(&targets[..observed_len], &own);
        actor.trace.station_aim = actor
            .station_aim(&targets[..observed_len], &own)
            .map(|t| t.id);
        let wing = WingView {
            control: self.wing_control,
            formation: self.formation,
            horizontal_spacing_ft: self.horizontal_spacing_ft,
            vertical_spacing_ft: self.vertical_spacing_ft,
            slot: if !actor.neutral
                && actor.identity.is_leader()
                && actor.assignment.role == engagement::Role::Escort
            {
                1 + actor.identity.wing * 3
            } else {
                actor.wing_slot
            },
            leader,
            wingmen_in_formation: 0,
            wing_combat: observed_len > 0,
            wing_approach: false,
            // B12's wing-approach value has no recovered producer, so the
            // wing-split branch stays untried rather than being fed ordinary
            // target distance.
            wing_approach_value_ft: None,
        };
        let route = RouteView {
            home_airport: actor.home(),
            leader_is_ai: true,
        };
        let batch = {
            let frame = DecisionFrame {
                tick,
                own,
                targets: &targets,
                events: &events,
                stations: &stations,
                dispensers: &actor.dispensers,
                wing,
                route,
                now,
                flight_state: actor
                    .airfield
                    .as_ref()
                    .map(|s| s.phase().flight_state())
                    .unwrap_or(if own.on_ground {
                        FlightState::TakingOff
                    } else {
                        FlightState::Free
                    }),
            };
            actor.controller.step(&frame)?
        };
        // How the adapter flies a striker's attack steer (slice ST-T2).
        let handling = actor.controller.take_strike_handling();
        // The frame is gone; keep what the controller saw.
        actor.trace.targets = targets;
        actor.trace.stations = stations;

        // B48 bingo: with a known home runway the aircraft now lands there.
        // Leaders and singletons use the same fitted rule.
        if matches!(
            batch.fuel_state,
            Some(super::route::FuelState::Bingo | super::route::FuelState::Critical)
        ) && actor.landing_order.is_none()
            && let Some(runway) = actor.home_runway
        {
            let order = super::airfield::LandingOrder {
                runway,
                reason: super::airfield::LandingReason::Fuel,
            };
            actor.landing_order = Some(order);
            actor.trace.bingo_landing = Some(order);
        }

        // Choosing a currently visible target is the only way to fill or
        // replace the Novice's one remembered hostile. Losing contact leaves
        // that frozen record intact until its own deadline.
        actor.awareness.select_target(actor.controller.target());
        if actor.controller.target().is_some() {
            actor.search_target = None;
        }
        if let Some(sensors) = &mut actor.sensors {
            if batch.sensor.clear_designation {
                sensors.clear_selection();
            }
            if let Some(target) = batch.sensor.designate {
                sensors.designate(target);
            }
        }
        output
            .wing
            .extend(batch.wing.iter().map(|request| (actor_id, *request)));
        for fallback in &batch.fallbacks {
            output.fallbacks.push((actor_id, *fallback));
        }
        output.launch_calls.extend(
            batch
                .launch_calls
                .iter()
                .map(|launcher| (actor_id, *launcher)),
        );
        if let Some(mut activity) = batch.activity {
            // A neutral aircraft waits in formation, unless it leads its
            // wing's mission of opportunity search.
            let wing_search = activity == Activity::Searching
                && (actor.controller.wing_search().is_some()
                    || actor.controller.wing_route().is_some());
            if actor.neutral
                && matches!(activity, Activity::Idle | Activity::Searching)
                && !wing_search
            {
                activity = Activity::Formation;
            }
            actor.activity = activity;
            output.activities.push((actor_id, activity));
        }

        // Opinionated load shedding, from this tick's perceived contacts only.
        let range = |id| {
            actor
                .trace
                .targets
                .iter()
                .find(|t| t.id == id && t.valid && t.is_aircraft && t.side != identity.side)
                .map(|t| distance(own.position, t.position))
        };
        let attacker_ft = reports
            .iter()
            .filter(|r| r.defended_id == actor_id)
            .filter_map(|r| r.attacker_id.and_then(range))
            .min_by(f64::total_cmp);
        let engagement_ft = actor.controller.target().and_then(range);
        if let Some(drop) = super::jettison::decide(super::jettison::Situation {
            missile_defense: actor
                .last_defense
                .is_some_and(|d| d.motion.is_some() || d.burst.is_some()),
            defensive_break: actor.fire_defending
                || matches!(
                    actor.activity,
                    Activity::Defending | Activity::Evading | Activity::Breaking
                ),
            engagement_ft: if matches!(
                actor.activity,
                Activity::Pursuing | Activity::Attacking | Activity::Acquiring
            ) {
                engagement_ft
            } else {
                None
            },
            attacker_ft,
            ..actor.load_situation()
        }) {
            output.jettison.push((actor_id, drop));
        }

        // 4. Weapons: debit this actor's own store, then emit the event.
        for intent in &batch.weapons {
            if let Some(event) = actor.release(intent, &batch) {
                output.launches.push(event);
            }
        }

        // B47: debit each device only when its quarter-second release is due.
        if let Some(devices) = batch.devices {
            actor
                .device_schedule
                .push((tick, devices.class, devices.count, false));
        }
        if let Some(burst) = actor.last_defense.and_then(|d| d.burst) {
            let available = |class| {
                let stock: u32 = actor
                    .dispensers
                    .iter()
                    .filter(|d| d.class == class)
                    .map(|d| d.count)
                    .sum();
                let queued: u32 = actor
                    .device_schedule
                    .iter()
                    .filter(|(_, c, _, _)| *c == class)
                    .map(|(_, _, count, _)| u32::from(*count))
                    .sum();
                stock.saturating_sub(queued)
            };
            let burst = burst.with_inventory(
                available(SeekerClass::Radar),
                available(SeekerClass::Infrared),
                actor.last_defense.is_some_and(|d| d.reserve_allowed),
            );
            let mut classes = Vec::new();
            for index in 0..burst.chaff.max(burst.flares) {
                if index < burst.chaff {
                    classes.push(SeekerClass::Radar);
                }
                if index < burst.flares {
                    classes.push(SeekerClass::Infrared);
                }
            }
            classes.retain(|class| {
                actor
                    .dispensers
                    .iter()
                    .any(|d| d.class == *class && d.count > 0)
            });
            // Keep one schedule across simultaneous missiles. Each release
            // debits its actual dispenser when due; an empty class never
            // prevents the other class or a defensive maneuver.
            for (index, &class) in classes.iter().enumerate() {
                actor.device_schedule.push((
                    tick + index as u64 * super::QUARTER_SECOND_TICKS,
                    class,
                    1,
                    true,
                ));
            }
        }
        let schedule = std::mem::take(&mut actor.device_schedule);
        let current_threat = actor.last_defense.is_some_and(|d| !d.debug.passed);
        for (due, class, remaining, defensive) in schedule {
            if defensive && !current_threat {
                continue;
            }
            if tick < due {
                actor
                    .device_schedule
                    .push((due, class, remaining, defensive));
            } else if let Some(event) = actor.release_devices(class, 1) {
                output.devices.push(event);
                if remaining > 1 {
                    actor.device_schedule.push((
                        due + super::QUARTER_SECOND_TICKS,
                        class,
                        remaining - 1,
                        defensive,
                    ));
                }
            }
        }

        // 6. Motion through the adapter and this actor's own flight model.
        actor.adapter.set_gun_aim(batch.gun_aim);
        actor.adapter.set_strike_handling(handling);
        let defensive =
            actor.controller.defending() && !actor.fire_defending && !actor.is_rotorcraft();
        let reduce_heat = !actor.fire_defending
            && actor
                .last_defense
                .and_then(|d| d.motion)
                .is_some_and(|m| m.reduce_heat);
        actor
            .adapter
            .set_defense(defensive.then_some((actor.controller.experience().level, reduce_heat)));
        let motion = batch
            .motion
            .map(|intent| actor.avoid_traffic(tick, intent, world));
        actor.fly(motion.as_ref(), &own, ground, surface)?;
        Ok(())
    }

    /// Journal one actor's messages of this step from its records: launch
    /// warnings dropped, queued or received, remembered attacks forgotten,
    /// and a changed escort selection. Reads records only; decides nothing.
    fn journal_actor(&mut self, index: usize, tick: u64) {
        let actor = &mut self.actors[index];
        let id = actor.id();
        let receipt = |outcome| vec![Receipt { actor: id, outcome }];
        let mut entries = Vec::new();
        for (report, reason) in &actor.trace.dropped_warnings {
            entries.push(JournalEntry {
                tick,
                sender: Some(report.launcher_id),
                message: Message::MissileWarning(*report),
                receipts: receipt(Outcome::WarningDropped(*reason)),
            });
        }
        let decided = actor.controller.trace();
        if decided.tick == Some(tick) {
            for warning in &decided.events.warnings {
                let outcome = match warning.step {
                    WarningStep::NotWarned => Outcome::WarningDropped(DropReason::NotWarned),
                    WarningStep::Queued { already_queued } => Outcome::WarningDue {
                        due_tick: warning.due_tick.unwrap_or(tick),
                        already_queued,
                    },
                    WarningStep::Received {
                        outcome,
                        launch_call,
                        ..
                    } => Outcome::WarningReceived {
                        outcome,
                        launch_call,
                    },
                };
                entries.push(JournalEntry {
                    tick,
                    sender: Some(warning.report.launcher_id),
                    message: Message::MissileWarning(warning.report),
                    receipts: receipt(outcome),
                });
            }
        }
        for (attack, reason) in &actor.trace.expired_attacks {
            entries.push(JournalEntry {
                tick,
                sender: Some(attack.report.defended_id),
                message: Message::AttackEvidence(*attack),
                receipts: receipt(Outcome::Expired(*reason)),
            });
        }
        if !actor.assignment.protected_ids.is_empty()
            && let Some(engagement) = &actor.trace.engagement
            && actor.journal_memory.escort_selection != Some(engagement.selection)
        {
            actor.journal_memory.escort_selection = Some(engagement.selection);
            entries.push(JournalEntry {
                tick,
                sender: Some(id),
                message: Message::EscortPriority {
                    selection: engagement.selection,
                },
                receipts: Vec::new(),
            });
        }
        for entry in entries {
            self.journal.push(entry);
        }
    }

    /// The leader's pose for a wingman, taken from the leader actor itself,
    /// and the leader's target from the engagement table.
    fn leader_view(
        &self,
        index: usize,
        world: &[WorldObject],
        engagements: &super::link::Engagements,
    ) -> Option<LeaderView> {
        let actor = &self.actors[index];
        if !actor.neutral && actor.assignment.role == engagement::Role::Escort {
            let same_assignment_leader = self.actors.iter().any(|leader| {
                leader.id() != actor.id()
                    && leader.alive()
                    && leader.identity.side == actor.identity.side
                    && leader.identity.wing == actor.identity.wing
                    && leader.identity.is_leader()
                    && leader.assignment.role == engagement::Role::Escort
                    && leader.assignment.protected_ids == actor.assignment.protected_ids
            });
            if actor.identity.is_leader() || !same_assignment_leader {
                return world
                    .iter()
                    .filter(|o| {
                        o.id != actor.id()
                            && o.side == actor.identity.side
                            && o.alive
                            && !o.destroyed
                            && actor.assignment.protected_ids.contains(&o.id)
                    })
                    .min_by_key(|o| o.id)
                    .map(|leader| LeaderView {
                        position: leader.position,
                        heading_deg: leader.heading_deg,
                        velocity: leader.velocity,
                        speed: leader.speed,
                        target: None,
                        recovering: false,
                        on_ground: false,
                    });
            }
        }
        if actor.identity.is_leader() {
            return None;
        }
        if let Some(id) = self.external_leader(actor.identity.side, actor.identity.wing) {
            let leader = world
                .iter()
                .find(|o| o.id == id && o.alive && !o.destroyed)?;
            // A human leader that touches down after flying is landing.
            return Some(LeaderView {
                position: leader.position,
                velocity: leader.velocity,
                heading_deg: leader.heading_deg,
                speed: leader.speed,
                target: None,
                recovering: leader.on_ground && self.airborne_seen.contains(&leader.id),
                on_ground: leader.on_ground,
            });
        }
        let leader = self.actors.iter().find(|a| {
            a.identity.is_leader()
                && a.identity.side == actor.identity.side
                && a.identity.wing == actor.identity.wing
                && a.alive()
        })?;
        let pose = world
            .iter()
            .find(|o| o.id == leader.id() && o.alive && !o.destroyed)?;
        let target = engagements.target(leader.id());
        if super::link::audit::active() {
            super::link::audit::leader(leader, target);
        }
        Some(LeaderView {
            position: pose.position,
            velocity: pose.velocity,
            heading_deg: pose.heading_deg,
            speed: pose.speed,
            target,
            recovering: matches!(leader.activity, Activity::ReturningToBase)
                || leader.airfield.as_ref().is_some_and(|s| !s.is_departure()),
            on_ground: false,
        })
    }

    /// Record which aircraft have been airborne, so a human leader's later
    /// touchdown reads as a landing.
    fn track_airborne(&mut self, world: &[WorldObject]) {
        for object in world.iter().filter(|o| o.alive && !o.destroyed) {
            let on_ground = match self.actor(object.id) {
                Some(actor) => actor.flight.research.as_ref().is_some_and(|r| r.on_ground),
                None => object.on_ground,
            };
            if !on_ground && !self.airborne_seen.contains(&object.id) {
                self.airborne_seen.push(object.id);
            }
        }
    }

    /// The retail takeoff and landing gates for one actor (spec in
    /// docs/spec/ai-airfield.md): turn order, runway free, earlier wing
    /// members down, and a parking slot.
    fn airfield_clearance(&self, index: usize, world: &[WorldObject]) -> AirfieldClearance {
        use super::airfield::{PARKING_SLOTS, PLAYER_ROLLING_FPS, SPOT_OCCUPIED_FT};
        let actor = &self.actors[index];
        let (side, wing, member) = (
            actor.identity.side,
            actor.identity.wing,
            actor.identity.member,
        );
        let wingmates = || {
            self.actors.iter().filter(move |a| {
                a.id() != actor.id()
                    && a.alive()
                    && a.identity.side == side
                    && a.identity.wing == wing
            })
        };
        let external_leader = self
            .external_leader(side, wing)
            .and_then(|id| world.iter().find(|o| o.id == id && o.alive && !o.destroyed));
        // Wing abort (retail 0x4bc2a4): a joining wingman whose leader is
        // neither landing nor on the ground stops landing. A leader that has
        // been destroyed is neither, so its wingmen stop following it down.
        let leader_landing = if actor.identity.is_leader() {
            true
        } else if let Some(leader) = external_leader {
            leader.on_ground || self.priority_landing.contains_key(&leader.id)
        } else {
            wingmates()
                .find(|a| a.identity.is_leader())
                .is_some_and(|leader| {
                    leader.airfield.as_ref().is_some_and(|s| !s.is_departure())
                        || leader.landing_order.is_some()
                        || leader.flight.research.as_ref().is_some_and(|r| r.on_ground)
                })
        };
        let Some(sequence) = actor.airfield.as_ref() else {
            return AirfieldClearance {
                leader_landing,
                ..AirfieldClearance::default()
            };
        };
        let airport = sequence.runway().airport;
        let at_airport = || {
            self.actors
                .iter()
                .filter(move |a| a.id() != actor.id() && a.alive())
                .filter_map(move |a| {
                    a.airfield
                        .as_ref()
                        .filter(|s| s.runway().airport == airport)
                        .map(|s| (a, s))
                })
        };

        // Turn gate: every earlier wing member past its first taxiway leg;
        // a human leader must be airborne; a member stood down stays parked.
        let turn = !self.stood_down(actor)
            && wingmates().all(|a| {
                a.identity.member >= member
                    || a.airfield.as_ref().is_none_or(|s| !s.holds_followers())
            })
            && external_leader.is_none_or(|leader| actor.identity.is_leader() || !leader.on_ground);

        // Runway-free gate.
        let spot = sequence.takeoff_spot();
        let near_spot = |position: [f64; 3]| {
            spot.is_some_and(|spot| {
                (position[0] - spot[0]).hypot(position[2] - spot[2]) <= SPOT_OCCUPIED_FT
            })
        };
        let runway = sequence.runway();
        let busy_actor = at_airport().any(|(a, s)| {
            s.blocks_runway()
                || (a.flight.research.as_ref().is_some_and(|r| r.on_ground)
                    && s.phase() != super::airfield::Phase::Parked
                    && near_spot(a.flight.position))
        });
        let busy_human = world.iter().any(|o| {
            o.alive
                && !o.destroyed
                && o.human_controlled
                && self.actor(o.id).is_none()
                && o.on_ground
                && (near_spot(o.position)
                    || (o.velocity[0].hypot(o.velocity[2]) >= PLAYER_ROLLING_FPS
                        && (o.position[0] - runway.center[0])
                            .hypot(o.position[2] - runway.center[2])
                            <= super::airfield::LINEUP_RESET_FT))
        });
        let runway_free = !busy_actor
            && !busy_human
            && !self.priority_landing.values().any(|held| *held == airport);

        // Earlier wing members landing here must be down first. A helicopter
        // air taxiing to its slot is down (slice H2).
        let wing_landed = wingmates().all(|a| {
            a.identity.member >= member
                || !a.airfield.as_ref().is_some_and(|s| {
                    !s.is_departure()
                        && s.runway().airport == airport
                        && !s.landed()
                        && !a.flight.research.as_ref().is_some_and(|r| r.on_ground)
                })
        });

        // Parking slot: keep one already held, else the lowest free one.
        let free_slot = sequence.slot().or_else(|| {
            (0..PARKING_SLOTS).find(|slot| !at_airport().any(|(_, s)| s.slot() == Some(*slot)))
        });

        AirfieldClearance {
            turn,
            runway_free,
            wing_landed,
            free_slot,
            leader_landing,
        }
    }

    /// B48 join-landing: an AI wingman within 10000 ft of a landing leader
    /// and 40000 ft of its airport lands there too. A human leader's airport
    /// is taken to be the wingman's home runway (fitted).
    fn join_landing(
        &self,
        index: usize,
        leader: Option<&LeaderView>,
    ) -> Option<super::airfield::LandingOrder> {
        let actor = &self.actors[index];
        let leader = leader.filter(|l| l.recovering)?;
        if actor.identity.is_leader()
            || actor.landing_order.is_some()
            || actor.airfield.is_some()
            || actor.bugged_out
            || actor.join_cancelled
        {
            return None;
        }
        let runway = self
            .actors
            .iter()
            .find(|a| {
                a.identity.is_leader()
                    && a.identity.side == actor.identity.side
                    && a.identity.wing == actor.identity.wing
                    && a.alive()
            })
            .and_then(|a| {
                a.airfield
                    .as_ref()
                    .filter(|s| !s.is_departure())
                    .map(|s| *s.runway())
                    .or(a.landing_order.map(|o| o.runway))
                    .or(a.home_runway)
            })
            .or(actor.home_runway)?;
        let position = actor.flight.position;
        let join = super::route::join_leader_landing(&super::route::JoinLandingInputs {
            leader_recovering: true,
            distance_to_leader_ft: distance(position, leader.position),
            distance_to_leader_airport_ft: Some(
                (position[0] - runway.center[0]).hypot(position[2] - runway.center[2]),
            ),
        });
        join.then_some(super::airfield::LandingOrder {
            runway,
            reason: super::airfield::LandingReason::JoinLeader,
        })
    }

    /// Deliver one wing command to one actor (B46).
    pub fn order(
        &mut self,
        actor: u32,
        request: super::wing::WingRequest,
    ) -> Option<Result<super::wing::ReceiverOutcome>> {
        let tick = self.tick;
        let recalling = matches!(
            request,
            super::wing::WingRequest::FormationSelection(_)
                | super::wing::WingRequest::TargetAssignment(super::wing::TargetOrder::HoldFire)
        );
        // A report perceived before recall may still be in next-tick delivery.
        // Remember its identity too, so its subsequent refresh is not a new shot.
        let pending_ids: Vec<_> = self
            .pending_attack_reports
            .iter()
            .filter(|(receiver, _)| recalling && *receiver == actor)
            .filter_map(|(_, attack)| attack.event_id)
            .collect();
        self.actor_mut(actor).map(|a| {
            let outcome = a.order(request, tick);
            if recalling && matches!(outcome, Ok(super::wing::ReceiverOutcome::Applied(_))) {
                a.ignored_attack_ids.extend(pending_ids);
                a.ignored_attack_ids.sort_unstable();
                a.ignored_attack_ids.dedup();
            }
            outcome
        })
    }

    pub fn track_ordered_approach(&mut self, actor: u32, target: u32, heading: f64, pitch: f64) {
        if let Some(actor) = self.actor_mut(actor) {
            actor
                .controller
                .track_ordered_approach(target, heading, pitch);
        }
    }

    /// A command stays inside the identified side and wing.
    pub fn order_wing(
        &mut self,
        side: super::targeting::Side,
        wing: u8,
        sender: Option<u32>,
        request: super::wing::WingRequest,
    ) -> Result<usize> {
        Ok(self
            .order_wing_report(side, wing, sender, None, request)?
            .len())
    }

    /// Per-recipient outcomes, including no-motion and rejection. A directed
    /// recipient never bypasses the side/wing boundary.
    pub fn order_wing_report(
        &mut self,
        side: super::targeting::Side,
        wing: u8,
        sender: Option<u32>,
        recipient: Option<u32>,
        request: super::wing::WingRequest,
    ) -> Result<Vec<(u32, super::wing::ReceiverOutcome)>> {
        if sender.is_some_and(|id| {
            !self
                .actor(id)
                .is_some_and(|a| a.alive() && a.identity.side == side && a.identity.wing == wing)
        }) {
            return Err(super::AiError::InvalidInput(
                "sender does not belong to live wing",
            ));
        }
        let mut recipients = Vec::new();
        for actor in &self.actors {
            if actor.alive()
                && actor.identity.side == side
                && actor.identity.wing == wing
                && recipient.is_none_or(|id| id == actor.id())
                && (Some(actor.id()) != sender
                    || matches!(
                        request,
                        super::wing::WingRequest::Spacing { .. }
                            | super::wing::WingRequest::FormationSelection(_)
                            | super::wing::WingRequest::WingControl(_)
                    ))
            {
                recipients.push(actor.id());
            }
        }
        let mut outcomes = Vec::with_capacity(recipients.len());
        for id in recipients {
            outcomes.push((id, self.order(id, request).expect("validated recipient")?));
        }
        self.journal.push(JournalEntry {
            tick: self.tick,
            sender,
            message: Message::WingRequest(Box::new(request)),
            receipts: outcomes
                .iter()
                .map(|&(actor, outcome)| Receipt {
                    actor,
                    outcome: Outcome::Order(outcome),
                })
                .collect(),
        });
        Ok(outcomes)
    }

    pub fn set_formation(&mut self, formation: Formation) {
        self.formation = formation;
    }

    /// The formation wings fly until an order selects another.
    pub fn formation(&self) -> Formation {
        self.formation
    }

    pub fn set_wing_control(&mut self, control: WingControl) {
        self.wing_control = control;
    }

    pub fn set_spacing(&mut self, horizontal_ft: i32, vertical_ft: i32) {
        self.horizontal_spacing_ft = super::wing::request_spacing(i64::from(horizontal_ft));
        self.vertical_spacing_ft = vertical_ft;
    }
}

impl AiActor {
    fn observation_input(&self, index: usize) -> Option<observation::Input<'_>> {
        if !self.alive() || self.dummy || self.flight.escape.is_some() || self.sensors.is_none() {
            return None;
        }
        // A first damage-return response clears the controller target and
        // sensor selection before observing. Leave that tick inline. Later
        // recovery ticks may be prepared because the preamble no longer
        // changes observation inputs. A ground hold or an ejection returns
        // before observation, so its speculative result is simply dropped.
        if self.damage_return.is_none()
            && super::damage::Response::assess(&self.flight.systems, self.flight.damage_fraction)
                .reason
                .is_some()
        {
            return None;
        }
        Some(observation::Input {
            index,
            context: self.observation_context(),
            sensors: self.sensors.as_ref(),
            memory: &self.awareness,
            use_surface: self.flight.research.is_some(),
            ceiling: None,
        })
    }

    /// Capture only the actor-local values used by observation. No worker
    /// receives an actor, controller, or live peer state.
    fn observation_context(&self) -> observation::Context {
        observation::Context {
            actor: self.id(),
            side: self.identity.side,
            observer: Observer {
                position: self.flight.position,
                basis: crate::attitude::Basis::new(
                    self.flight.yaw,
                    self.flight.pitch,
                    self.flight.bank,
                ),
                radar_powered: self.flight.radar,
                radar_failed: self.equipment.radar,
                infrared_failed: self.equipment.infrared,
                visual_failed: self.equipment.visual,
            },
            selected: self.controller.target(),
            experience: self.controller.experience().level,
            receive_emitters: self.flight.systems.counts[32] <= 1,
            seeker_eligible: self.seeker_eligible(true),
            test: observation::ObservationTest::NORMAL,
        }
    }

    /// Collect fresh measurements, then update frozen aircraft memory. The
    /// inline path borrows existing state directly, without speculative clones.
    #[cfg(test)]
    fn observe(
        &mut self,
        tick: u64,
        world: &[WorldObject],
        ground: &dyn Fn(f64, f64) -> f64,
    ) -> Vec<TargetView> {
        let sight = observation::SightLines::new(self.flight.position, ground, None);
        self.observe_sighted(tick, world, &sight)
    }

    /// Observation with the tick's shared sight lines from this actor, with
    /// the full sensors (the reference the unsplit-observation tests hold).
    #[cfg(test)]
    fn observe_sighted(
        &mut self,
        tick: u64,
        world: &[WorldObject],
        sight: &observation::SightLines<'_>,
    ) -> Vec<TargetView> {
        self.observe_sighted_with(tick, world, sight, AiTestSettings::full())
    }

    /// [`Self::observe_sighted`] under the AI test settings.
    fn observe_sighted_with(
        &mut self,
        tick: u64,
        world: &[WorldObject],
        sight: &observation::SightLines<'_>,
        test: AiTestSettings,
    ) -> Vec<TargetView> {
        let context = observation::Context {
            test: observation::ObservationTest::of(test),
            ..self.observation_context()
        };
        let observed = observation::observe(
            context,
            &mut self.sensors,
            &mut self.awareness,
            &mut self.trace.visual,
            tick,
            world,
            sight,
        );
        self.apply_observation(observed)
    }

    /// A tick between two observations (AI observation rate below 120 Hz).
    /// The awareness memory, the sensors and the received emitters stay as
    /// the last observation left them, so the targets are the ones it
    /// admitted, less any that have been destroyed since. The lookout is
    /// refreshed (the aircraft's own pose and this tick's scan sector, with
    /// the last observation's attention point), because the missile threat
    /// and incoming fire services see through it every tick.
    fn hold_observation(&mut self, tick: u64, world: &[WorldObject]) -> Vec<TargetView> {
        let observer = self.observation_context().observer;
        let attention = self.lookout.and_then(|lookout| lookout.attention);
        let lookout = awareness::Lookout::new(tick, observer.position, observer.basis, attention);
        self.lookout = Some(lookout);
        self.trace.lookout = Some(lookout);
        self.trace.visual.clone_from(&self.last_visual);
        self.trace.observation_sources = self
            .awareness
            .current_observations()
            .take(32)
            .map(|s| (s.target.id, s.source_ticks))
            .collect();
        self.awareness
            .current_observations()
            .map(|snapshot| TargetView {
                position: awareness::held_position(snapshot, tick),
                ..snapshot.target
            })
            .filter(|target| {
                world
                    .iter()
                    .any(|o| o.id == target.id && o.alive && !o.destroyed)
            })
            .collect()
    }

    /// The ids of the emitters this aircraft hears, for naming the launcher
    /// of an electronically supported missile warning: the list the last
    /// observation built, or with lean sensors the list built now.
    fn emitter_ids(
        &self,
        world: &[WorldObject],
        sight: &observation::SightLines<'_>,
        test: AiTestSettings,
    ) -> Vec<u32> {
        if test.lean_sensors {
            observation::received_emitters_on_demand(
                self.observation_context(),
                &self.sensors,
                world,
                sight,
            )
            .iter()
            .map(|emitter| emitter.id)
            .collect()
        } else {
            self.received_emitters
                .iter()
                .map(|emitter| emitter.id)
                .collect()
        }
    }

    fn apply_observation(&mut self, observed: observation::Observed) -> Vec<TargetView> {
        self.lookout = Some(observed.lookout);
        self.trace.lookout = Some(observed.lookout);
        self.received_emitters = observed.emitters;
        self.trace.observation_sources = observed.sources;
        observed.targets
    }

    /// Choose a stable, lost hostile observation for investigation. Current
    /// observations stay in the ordinary selector; a frozen record never does.
    fn update_search_contact(&mut self, protected: &[engagement::ProtectedView]) {
        if self.neutral {
            self.search_target = None;
            self.controller.set_search_contact(None);
            return;
        }
        let lost = |snapshot: &&awareness::Snapshot| {
            snapshot.target.side != self.identity.side
                && self.mission_policy.allows_investigation(
                    &self.assignment,
                    &snapshot.target,
                    protected,
                    &self
                        .observed_attacks
                        .iter()
                        .map(|a| a.report)
                        .collect::<Vec<_>>(),
                    self.id(),
                )
                && !self
                    .awareness
                    .current_observations()
                    .any(|current| current.target.id == snapshot.target.id)
        };
        let preferred = self.controller.target().or(self.search_target);
        let snapshot = preferred
            .and_then(|id| self.awareness.snapshot(id))
            .filter(lost)
            .or_else(|| {
                self.awareness.remembered().filter(lost).min_by(|a, b| {
                    distance(self.flight.position, a.target.position)
                        .total_cmp(&distance(self.flight.position, b.target.position))
                        .then_with(|| a.target.id.cmp(&b.target.id))
                })
            });
        self.search_target = snapshot.map(|s| s.target.id);
        let contact = snapshot.map(|s| SearchContact {
            id: s.target.id,
            position: s.target.position,
            observed_tick: s.last_observed_tick,
        });
        self.controller.set_search_contact(contact);
        self.trace.search_contact = contact;
    }

    fn update_defense(
        &mut self,
        tick: u64,
        missiles: &[MissileSnapshot],
        rounds: &incoming_fire::Rounds<'_>,
        ground_ceiling: Option<&crate::ground_ceiling::GroundCeiling>,
        own: &OwnState,
        ground: &dyn Fn(f64, f64) -> f64,
    ) {
        let lookout = self.lookout.unwrap_or_else(|| {
            awareness::Lookout::new(
                tick,
                self.flight.position,
                crate::attitude::Basis::new(self.flight.yaw, self.flight.pitch, self.flight.bank),
                None,
            )
        });
        self.missile_threats.observe_with_visual(
            tick,
            Receiver {
                id: self.id(),
                position: own.position,
                velocity: self.flight.velocity,
                heading_deg: own.heading_deg,
                pitch_deg: own.body_pitch_deg(),
                skill: self.controller.experience().level,
                rwr_operating: self.flight.systems.counts[32] <= 1 && !self.equipment.rwr,
                visual_operating: !self.equipment.visual,
                visibility_limit_ft: None,
            },
            missiles,
            |point| {
                lookout.check(self.controller.experience().level, point, None, true)
                    == awareness::VisualResult::Visible
            },
            |from, to| {
                ground_ceiling.is_some_and(|ceiling| ceiling.clear_above(from, to))
                    || crate::combat::live::terrain_hit(from, to, &ground).is_none()
            },
        );
        let speed = own.speed.0.max(125.0);
        let g = own
            .g_limit
            .max(1.0)
            .min(1.0 / own.maximum_bank_deg.to_radians().cos().max(0.01));
        let bank = (1.0 / g).acos().to_degrees();
        let coordinated_rate = (32.174 * (g * g - 1.0).sqrt() / speed).to_degrees();
        let contacts: Vec<_> = self.missile_threats.records().copied().collect();
        let defense_own = defense::DefenseOwn {
            position: own.position,
            velocity: self.flight.velocity,
            heading_deg: own.heading_deg,
            flight_path_pitch_deg: own.flight_path_pitch_deg,
            speed_ft_s: own.speed.0,
            bank_deg: own.bank_deg,
            usable_turn_rate_deg_s: coordinated_rate * 0.5,
            usable_pitch_rate_deg_s: (32.174 * (g - 1.0) / speed).to_degrees() * 0.5,
            roll_in_time_s: (bank + own.bank_deg.abs()) / own.roll_limit_deg_per_s.max(1.0) + 0.7,
            dive_speed_safe: own.speed.0
                + 32.174 * (-defense::DIVE_PITCH_DEG).to_radians().sin() * 5.0
                <= own.limits.maximum.0,
        };
        self.last_defense = defense::decide(
            tick,
            self.controller.experience().level,
            defense_own,
            &contacts,
            &mut self.defense_state,
            |position| ground(position[0], position[2]),
        );
        // A sight line wholly above the highest ground under it cannot meet
        // it, so it is not sampled; the answer is the same either way.
        let clear = |point| {
            ground_ceiling.is_some_and(|ceiling| ceiling.clear_above(own.position, point))
                || crate::combat::live::terrain_hit(own.position, point, &ground).is_none()
        };
        // A round beyond the lookout's visual range is never seen.
        let sight = incoming_fire::SightReach {
            center: lookout.position,
            radius: awareness::visual_range_feet(self.controller.experience().level),
        };
        let visible = |point| {
            lookout.check(self.controller.experience().level, point, None, true)
                == awareness::VisualResult::Visible
                && clear(point)
        };
        // Every test that steps the AI checks the indexed visit against
        // visiting every round.
        #[cfg(test)]
        let mut every_round = self.incoming_fire.clone();
        #[cfg(test)]
        every_round.observe(tick, self.id(), defense_own, rounds.all(), visible, clear);
        let gunfire_own = defense::DefenseOwn {
            dive_speed_safe: own.speed.0 + 32.174 * 20_f64.to_radians().sin() * 5.0
                <= own.limits.maximum.0,
            ..defense_own
        };
        self.incoming_fire.observe_near(
            tick,
            self.id(),
            defense_own,
            rounds,
            sight,
            visible,
            clear,
        );
        #[cfg(test)]
        assert_eq!(
            format!("{every_round:?}"),
            format!("{:?}", self.incoming_fire),
            "the indexed rounds changed what the aircraft saw"
        );
        let fire_motion = self
            .incoming_fire
            .motion(tick, self.id(), gunfire_own, |p| ground(p[0], p[2]));
        let missile_motion = self.last_defense.and_then(|d| d.motion);
        let fire_selected = fire_motion.is_some()
            && (missile_motion.is_none()
                || self.incoming_fire.cue().is_some_and(|c| {
                    c.remaining_time(tick)
                        < self
                            .last_defense
                            .and_then(|d| d.debug.estimated_threat_time_s)
                            .unwrap_or(0.)
                }));
        self.fire_defending = fire_selected;
        self.trace.fire = incoming_fire::Trace {
            cue: self.incoming_fire.cue(),
            motion: fire_motion,
            selected: fire_selected,
        };
        if let Some(cue) = self.incoming_fire.cue().filter(|c| c.observed_tick == tick) {
            self.remember_attack(ObservedAttack {
                report: engagement::ThreatReport {
                    attacker_id: None,
                    defended_id: self.id(),
                },
                bearing_world_deg: cue.bearing_world_deg,
                observed_tick: tick,
                event_id: cue.round,
            });
        }
        if self.last_defense.is_none() {
            self.defense_state.clear_threat();
        }
        let source = if fire_selected {
            super::controller::DefenseSource::IncomingFire
        } else {
            super::controller::DefenseSource::Missile
        };
        // A helicopter does not fly the fighter's jink and dive (slice ST-H,
        // docs/spec/ai-strike.md, "Helicopter defence"): its own break, and
        // from the warning on, not at the fighter's last-moment timing (a
        // helicopter hides, it does not time a jink).
        let rotorcraft = self.is_rotorcraft();
        let suggestion = if fire_selected {
            fire_motion
        } else if rotorcraft && self.last_defense.is_some() {
            Some(missile_motion.unwrap_or(defense::MotionSuggestion {
                maneuver: defense::Maneuver::Jink,
                reduce_heat: false,
                heading_deg: own.heading_deg,
                flight_path_pitch_deg: 0.,
            }))
        } else {
            missile_motion
        };
        self.trace.rotor_break = None;
        let motion = suggestion.map(|motion| {
            if !rotorcraft {
                return (motion.heading_deg, motion.flight_path_pitch_deg);
            }
            let (threat, viewpoint) = if fire_selected {
                (
                    self.incoming_fire.cue().and_then(|c| c.bearing_world_deg),
                    self.defense_viewpoint(own, false),
                )
            } else {
                let contact = self
                    .last_defense
                    .and_then(|d| contacts.iter().find(|c| c.missile_id == d.threat_id));
                (
                    contact.map(|c| (own.heading_deg + c.bearing_deg).rem_euclid(360.)),
                    self.defense_viewpoint(own, true)
                        .or_else(|| contact.and_then(|c| c.position)),
                )
            };
            let tie = self.last_defense.map_or_else(|| self.id(), |d| d.threat_id);
            let answer = super::helicopter_defense::decide(
                super::helicopter_defense::Own {
                    position: own.position,
                    heading_deg: own.heading_deg,
                    speed_ft_s: own.speed.0,
                    bank_deg: own.bank_deg,
                    // The rotor adapter holds the speed on the cyclic
                    // whatever the path, so a descent does not speed it
                    // up: its few knots over the maximum are no reason to
                    // stop coming down (slice H1).
                    maximum_ft_s: own.limits.maximum.0
                        + if super::rotor_adapter::flies(&self.flight) {
                            super::rotor_adapter::SPEED_HOLD_MARGIN_KT
                                * crate::runway_wind::FEET_PER_SECOND_PER_KNOT
                        } else {
                            0.0
                        },
                },
                super::helicopter_defense::Threat {
                    bearing_deg: threat,
                    viewpoint,
                    tie,
                },
                ground,
            );
            self.trace.rotor_break = Some((answer, threat));
            (answer.heading_deg, answer.flight_path_pitch_deg)
        });
        // A striker inside its committed-to-release window flies its run on
        // and releases; the defence waits until it has (T.O.R.E, John,
        // 2026-10-28, docs/spec/ai-strike.md). Countermeasures and the
        // defence's own state go on.
        let held = self.strike.as_deref().is_some_and(|s| s.holds_run(tick));
        let motion = motion.filter(|_| !held);
        self.controller
            .set_defense_motion(motion.map(|(heading_deg, pitch_deg)| {
                super::controller::DefenseMotion {
                    source,
                    heading_deg,
                    pitch_deg,
                    rotorcraft,
                    extend: !fire_selected
                        && missile_motion.is_some_and(|m| m.maneuver == defense::Maneuver::Drag),
                }
            }));
    }

    /// The aircraft is a helicopter: its flight model is a rotorcraft's.
    pub fn is_rotorcraft(&self) -> bool {
        self.flight
            .model()
            .powered_lift()
            .is_some_and(|lift| lift.kind == crate::models::variety::LiftKind::Helicopter)
    }

    /// Where the defence seen by a helicopter is: the nearest hostile
    /// missile site (or gun site, for gunfire) the aircraft knows that
    /// reaches it. A CAP actor has no ground picture and so none.
    fn defense_viewpoint(&self, own: &OwnState, missiles: bool) -> Option<crate::attitude::Vector> {
        let picture = self.ground.as_deref()?;
        picture
            .threats()
            .hostile_defenses(self.identity.side)
            .filter(|d| {
                if missiles {
                    d.covers_with_missiles(own.position)
                } else {
                    d.covers_with_guns(own.position)
                }
            })
            .min_by(|a, b| {
                distance(a.position, own.position)
                    .total_cmp(&distance(b.position, own.position))
                    .then(a.id.cmp(&b.id))
            })
            .map(|d| d.position)
    }

    fn drain_events(&mut self, tick: u64) -> Vec<FrameEvent> {
        let mut events = std::mem::take(&mut self.pending_events);
        for report in self.pending_threats.drain(..) {
            events.push(FrameEvent::ThreatReported(report));
        }
        let _ = tick;
        events
    }

    /// The actor's own state, entirely from its own flight model.
    fn own_state(&self, ground: &dyn Fn(f64, f64) -> f64) -> OwnState {
        let terrain = ground(self.flight.position[0], self.flight.position[2]);
        let agl = self.flight.position[1] - terrain;
        // Terrain ahead of the aircraft, for the B44 floor: the highest
        // ground along the look-ahead (see [`terrain_lookahead_ft`]).
        let heading = self.flight.yaw;
        // A helicopter on its own control keeps a lower floor than the
        // fighters', so it can fly the 200 ft masking height (slice H1);
        // the V-22 keeps the fighters' (slice HF1).
        let minimum_altitude = if super::rotor_adapter::flies(&self.flight) {
            super::rotor_adapter::minimum_altitude_ft(&self.flight, MINIMUM_ALTITUDE_FT)
        } else {
            MINIMUM_ALTITUDE_FT
        };
        let limits = self.speed_limits();
        let (positive_g, _negative_g) = self.g_limits();
        // Above corner speed the envelope, not the pilot, limits the pull,
        // so the look-ahead also covers the time that G needs to raise the
        // flight path (see [`terrain_lookahead_ft`]). Rotorcraft keep theirs.
        let pull_excess_g = (!super::rotor_adapter::flies(&self.flight)
            && self.flight.speed > limits.corner.0)
            .then_some(positive_g - 1.0);
        let (terrain_ahead, terrain_climb) = terrain_ahead_ft(
            self.flight.position,
            heading,
            terrain_lookahead_ft(self.flight.speed, pull_excess_g),
            minimum_altitude,
            ground,
        );
        OwnState {
            position: self.flight.position,
            heading_deg: heading.to_degrees(),
            flight_path_pitch_deg: self.flight_path_pitch_deg(),
            body_pitch_offset_deg: self.flight.pitch.to_degrees() - self.flight_path_pitch_deg(),
            bank_deg: self.flight.bank.to_degrees(),
            speed: ScalarSpeed(self.flight.speed),
            limits,
            altitude_msl_ft: self.flight.position[1],
            agl_ft: agl,
            terrain_ahead_ft: terrain_ahead,
            terrain_climb_deg: terrain_climb,
            minimum_altitude_ft: minimum_altitude,
            at_ceiling: false,
            on_ground: agl <= GROUND_CONTACT_FT,
            g_limit: positive_g.min(
                super::damage::Response::assess(&self.flight.systems, self.flight.damage_fraction)
                    .g_cap
                    .unwrap_or(if self.damage_return.is_some() {
                        2.5
                    } else {
                        f64::INFINITY
                    }),
            ),
            roll_limit_deg_per_s: self
                .flight
                .model()
                .configuration()
                .aerodynamics
                .roll_limit_rad_per_second
                .to_degrees()
                * self.control_health(),
            maximum_bank_deg: if self.damage_return.is_some() {
                30.
            } else {
                MAXIMUM_BANK_DEG
            },
            alive: self.alive(),
            fuel_endurance_s: self.endurance_s(),
            // A helicopter or the V-22 has its own fuel schedule (slice H3,
            // see `route::rotorcraft_fuel_state`): its time home is at its
            // own cruise, not the fighters' slow fifth of the envelope.
            time_home_s: self.time_home_s(),
            internal_fuel_lbs: self.flight.fuel,
            radar_emitting: self.flight.radar,
            rotorcraft: super::rotor_adapter::flies(&self.flight),
        }
    }

    fn flight_path_pitch_deg(&self) -> f64 {
        let v = self.flight.velocity;
        let horizontal = (v[0] * v[0] + v[2] * v[2]).sqrt();
        if horizontal == 0.0 && v[1] == 0.0 {
            return self.flight.pitch.to_degrees();
        }
        v[1].atan2(horizontal).to_degrees()
    }

    /// The loaded envelope limits at the current altitude (B04, B15).
    ///
    /// Fitted host rule, agent decision 2026-09-17: the minimum and maximum
    /// come from the widest speed band any loaded envelope permits at this
    /// altitude, and corner speed is the slowest speed at which the highest-G
    /// envelope is still available, capped at the maximum. The spec calls
    /// these "the aircraft's loaded envelope limits at its current altitude"
    /// without naming the query, so this reads the same envelope block the
    /// flight model uses rather than inventing a table.
    pub fn speed_limits(&self) -> SpeedLimits {
        speed_limits_of(&self.flight)
    }

    /// The hybrid model's loaded low-speed positive G ceiling at `speed` with
    /// the current flaps, as the control adapter computes it, from the rows'
    /// bands at the current altitude; `None` above the ramp or for an
    /// aircraft without a 1 G envelope here.
    fn hybrid_ramp_g_with(&self, rows: &crate::flight::RowSpeeds, speed: f64) -> Option<f64> {
        let config = self.flight.model().configuration();
        let (clean_stall, _) = rows.one_g()?;
        let stall = clean_stall * (1.0 - 0.25 * self.flight.flaps);
        let loading = 1.0
            + (self.flight.fuel + self.flight.carried_lbs()) / config.mass.empty_lbs
                * config.aerodynamics.loaded_elevator_percent
                / 100.0;
        let next = crate::flight::low_speed_ramp_end(rows.bands(), stall)?;
        crate::flight::low_speed_ceiling_on(next, speed, stall, false)
            .map(|ceiling| ceiling.limit_g / loading)
    }

    /// Fitted (agent decision, 2026-09-29): the slowest speed at which the
    /// loaded aircraft, flaps down, still has [`APPROACH_G_MARGIN`] of lift,
    /// so a final can arrest a sink and follow its path. Zero for an
    /// aircraft without the hybrid model's low-speed ramp.
    fn approach_minimum_fps(&self) -> f64 {
        if self.flight.research.is_none() {
            return 0.0;
        }
        let config = self.flight.model().configuration();
        let altitude = self.flight.position[1];
        let Some((clean_stall, maximum)) = config
            .aerodynamics
            .envelopes
            .iter()
            .find(|e| e.g == 1)
            .and_then(|e| e.speeds(altitude))
        else {
            return 0.0;
        };
        let stall = clean_stall * 0.75;
        let loading = 1.0
            + (self.flight.fuel + self.flight.carried_lbs()) / config.mass.empty_lbs
                * config.aerodynamics.loaded_elevator_percent
                / 100.0;
        // Where the ramp ends does not depend on the speed: found once.
        let next = crate::flight::low_speed_ramp_end(
            config
                .aerodynamics
                .envelopes
                .iter()
                .filter(|envelope| envelope.g > 1)
                .map(|envelope| (envelope.g, envelope.speeds(altitude))),
            stall,
        );
        let mut speed = stall;
        while speed < maximum {
            match next
                .and_then(|next| crate::flight::low_speed_ceiling_on(next, speed, stall, false))
                .map(|ceiling| ceiling.limit_g)
            {
                Some(g) if g / loading < APPROACH_G_MARGIN => speed += 5.0,
                _ => return speed,
            }
        }
        maximum
    }

    /// The loaded G limits, already carrying the AI-only experience
    /// adjustment. A human-flown aircraft is exempt and never reaches here.
    fn g_limits(&self) -> (f64, f64) {
        let speed = self.flight.speed;
        // Every row's band once: the passes below all read them.
        let rows = crate::flight::RowSpeeds::of(&self.flight);
        let mut available = 1;
        for (envelope, speeds) in rows.rows() {
            if let Some((low, high)) = speeds
                && speed >= low
                && speed <= high
                && envelope.g > available
            {
                available = envelope.g;
            }
        }
        let config = self.flight.model().configuration();
        let loading = (self.flight.fuel + self.flight.payload_lbs) / config.mass.empty_lbs;
        let mut loaded = f64::from(available.max(1))
            / (1.0 + loading * config.aerodynamics.loaded_elevator_percent / 100.0);
        // The hybrid flight model keeps 1 G anywhere inside the 1 G envelope
        // and ramps from 1 G at the stall to the next envelope row
        // (docs/FLIGHT-MODEL.md, "Envelope limits and loading"), so the AI
        // may ask for what the model gives there, as its control adapter
        // already assumes. Before this a loaded aircraft slowing on final was
        // held below 1 G, could not hold its path and sank into the ground.
        if self.flight.research.is_some() {
            if let Some(ramp) = self.hybrid_ramp_g_with(&rows, speed) {
                loaded = loaded.max(ramp);
            }
            if rows.rows().any(|(e, speeds)| {
                e.g == 1 && speeds.is_some_and(|(low, high)| speed >= low && speed <= high)
            }) {
                loaded = loaded.max(1.0);
            }
        }
        let (positive, negative) = ai_g_limits(
            self.controller.experience().level,
            loaded,
            -loaded / 2.0,
            self.identity.human_controlled,
        );
        (
            health_scaled_positive_g(positive, self.control_health()),
            negative * self.control_health(),
        )
    }

    /// Fitted caution factor: remaining airframe health scales requested
    /// authority. Actual component and regional effects stay in flight physics.
    fn control_health(&self) -> f64 {
        (1.0 - self.flight.damage_fraction).clamp(0.0, 1.0)
    }

    /// B48 endurance at cruise, seconds.
    ///
    /// Fitted host rule: fuel flow is the model's military flow scaled by the
    /// current throttle, floored so a shut-down engine does not report an
    /// infinite endurance (a rotorcraft plans with its level cruise flow, see
    /// `State::endurance_flow_lbs_per_second`). The spec's own definition
    /// needs the throttle search that holds cruise speed, which the host does
    /// not yet perform.
    ///
    /// All remaining fuel counts, external tanks included: they burn first
    /// (`Fuel::consume`), so an AI aircraft with full tanks has their fuel to
    /// fly on before its internal fuel falls, as the player's fuel state
    /// counts it (docs/spec/cockpit-voice.md).
    fn endurance_s(&self) -> f64 {
        (self.flight.fuel + self.flight.systems.external_lbs())
            / self.flight.endurance_flow_lbs_per_second()
    }

    /// Seconds a full internal tank lasts at the flow `endurance_s` uses.
    fn full_endurance_s(&self) -> f64 {
        let c = self.flight.model().configuration();
        c.mass.internal_fuel_lbs / self.flight.full_tank_endurance_flow_lbs_per_second()
    }

    fn time_home_s(&self) -> Option<f64> {
        let home = self.home()?;
        let dx = home.x - self.flight.position[0];
        let dz = home.z - self.flight.position[2];
        let distance = (dx * dx + dz * dz).sqrt();
        let limits = self.speed_limits();
        let rotorcraft = super::rotor_adapter::flies(&self.flight);
        let cruise = if rotorcraft {
            super::route::rotorcraft_cruise_speed(&limits)
        } else {
            super::route::cruise_speed(&limits)
        };
        if cruise.0 <= 0.0 {
            return None;
        }
        let time = distance / cruise.0;
        // A home a full tank could not fly to and back from, with the
        // reserve, is none for a rotorcraft's fuel schedule (slice H3).
        if rotorcraft && !super::route::rotorcraft_home_in_range(time, self.full_endurance_s()) {
            return None;
        }
        Some(time)
    }

    /// Capture permitted classification/attitude metadata at the time of an
    /// observation. The measured position comes from that observation, not a
    /// later lookup of a remembered object ID in the world.
    #[cfg(test)]
    fn observed_target(&self, object: &WorldObject, position: [f64; 3]) -> TargetView {
        TargetView {
            id: object.id,
            side: object.side,
            position,
            heading_deg: object.heading_deg,
            pitch_deg: object.pitch_deg,
            speed: object.speed,
            maximum_speed: object.maximum_speed,
            is_aircraft: object.is_aircraft,
            is_fighter: object.is_fighter,
            infrared_strength: object.infrared_strength,
            human_controlled: object.human_controlled,
            valid: object.alive && !object.destroyed,
            type_allowed: true,
            seeker_eligible: self.seeker_eligible(object.is_aircraft),
            wing_attackers: 0,
            terrain_blocked: false,
            sensor_supported: self.sensors.as_ref().is_none_or(|s| s.supports(object.id)),
            link_track: false,
        }
    }

    /// Whether any carried store's envelope could engage this object (B45).
    fn seeker_eligible(&self, is_aircraft: bool) -> bool {
        let class = if is_aircraft {
            weapon_service::TargetClass::Air
        } else {
            weapon_service::TargetClass::Surface
        };
        self.stations.iter().any(|s| {
            !s.store.inhibited
                && !s.is_empty()
                && weapon_service::store_eligible(s.capability, class)
        })
    }

    /// The target the stores are pointed at when their envelopes are judged.
    fn station_aim<'a>(&self, targets: &'a [TargetView], own: &OwnState) -> Option<&'a TargetView> {
        // Point the stores at the target this actor is actually holding, not
        // at whichever object happens to come first in the world list. The
        // retained target is last tick's; the controller re-selects inside
        // `step`, and a one tick lag on a pointing error is immaterial next to
        // aiming at the wrong aircraft. With nothing retained, the nearest
        // permitted candidate is the best available guess.
        let retained = self.controller.target();
        retained
            .and_then(|id| targets.iter().find(|t| t.id == id))
            .or_else(|| {
                targets.iter().min_by(|a, b| {
                    distance(own.position, a.position)
                        .partial_cmp(&distance(own.position, b.position))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            })
    }

    fn station_views(&self, targets: &[TargetView], own: &OwnState) -> Vec<StationView> {
        let target = self.station_aim(targets, own);
        self.stations
            .iter()
            .map(|s| {
                let pointing = target
                    .map(|t| pointing_error_deg(own, t.position))
                    .unwrap_or(180.0);
                let mut view = StationView {
                    station: s.station,
                    guided: s.guided,
                    capability: s.capability,
                    inhibited: s.store.inhibited,
                    rounds: s.store.rounds,
                    pointing_error_deg: pointing,
                    employment_limit_deg: s.employment_limit_deg,
                    employment_fit: s
                        .employment_limit_deg
                        .is_none_or(|limit| pointing <= limit)
                        .then_some(pointing),
                    minimum_range_ft: s.minimum_range_ft,
                    maximum_range_ft: s.maximum_range_ft,
                    energy_target: s.energy_target,
                    infrared_envelope: s.infrared_envelope,
                    requires_radar: s.requires_radar,
                    requires_sensor: s.requires_sensor,
                    employment_zone: s.employment_zone,
                    mount: s.mount,
                    damage_vs_category: s.damage_vs_category,
                    store_speed: s.store_speed,
                    tracking_delay: s.tracking_delay,
                    pacing: s.pacing,
                };
                view.employment_fit = target.and_then(|target| view.employment_error(own, target));
                view
            })
            .collect()
    }

    /// Authorize a launch from this actor's own nonempty, uninhibited store.
    /// Live guns defer their per-round debit to physical emission in the host;
    /// other stores debit here through the shared B45 release service.
    fn release(
        &mut self,
        intent: &super::controller::WeaponIntent,
        _batch: &IntentBatch,
    ) -> Option<LaunchEvent> {
        let index = self
            .stations
            .iter()
            .position(|s| s.station == intent.request.station)?;
        let spec = &mut self.stations[index];
        if self.guns.contains_key(&spec.station.0) {
            if spec.store.inhibited || spec.is_empty() {
                return None;
            }
            // The host debits this single physical round only after allocation
            // and a final alignment check succeed.
            return Some(LaunchEvent {
                actor: self.id(),
                station: intent.request.station,
                target: intent.request.target.0,
                request_id: intent.request.request_id,
                projectiles: 1,
            });
        }
        let request = weapon_service::ReleaseRequest {
            debit: spec.debit.max(1),
            projectile_count: spec.projectile_count.max(1),
            allocation_succeeds: true,
            target: weapon_service::TargetRetention::Retained(intent.request.target),
            // No AI path is human controlled, so the global unlimited
            // ammunition bypass can never apply here (B45).
            human_controlled: false,
            global_unlimited: false,
            atomic_release: false,
        };
        let before = spec.rounds();
        let report = weapon_service::release(&mut spec.store, &request).ok()?;
        if let (Rounds::Finite(before), Rounds::Finite(after)) = (before, spec.rounds()) {
            self.flight.payload_lbs = (self.flight.payload_lbs
                - f64::from(before - after) * spec.external_round_lbs)
                .max(0.0);
        }
        let created = match report.projectiles {
            weapon_service::ProjectileCreation::Created { count } => count,
            weapon_service::ProjectileCreation::None => return None,
        };
        Some(LaunchEvent {
            actor: self.identity.actor.0,
            station: intent.request.station,
            target: intent.request.target.0,
            request_id: intent.request.request_id,
            projectiles: created,
        })
    }

    /// Debit this actor's own dispensers (B47).
    ///
    /// Release stops as soon as the matching dispenser is empty, and the other
    /// class is never substituted.
    fn release_devices(&mut self, class: SeekerClass, count: u8) -> Option<DeviceEvent> {
        let index = super::threat::select_dispenser(&self.dispensers, class)?;
        let mut released = 0;
        for _ in 0..count {
            match super::threat::debit_device(&mut self.dispensers[index], false) {
                Ok(_) => released += 1,
                Err(_) => break,
            }
        }
        if released == 0 {
            return None;
        }
        Some(DeviceEvent {
            actor: self.identity.actor.0,
            class,
            released,
        })
    }

    /// Convert this tick's maneuver into controls and step this actor's own
    /// flight model. Only that model may advance the aircraft state.
    fn fly(
        &mut self,
        intent: Option<&MotionIntent>,
        own: &OwnState,
        ground: &dyn Fn(f64, f64) -> f64,
        surface: &dyn Fn(f64, f64) -> Surface,
    ) -> Result<()> {
        let mut fly = FlyTrace {
            intent: intent.copied(),
            terrain_floor_deg: None,
            adapter: None,
            rotor: None,
            jet_lift: None,
            achieved: thought::Achieved::default(),
        };
        let input = match intent {
            Some(intent) => {
                // A striker's planned dive takes its own pull-out floor
                // (slice ST-T2); everything else the B44 floor.
                let general = self.controller.terrain_floor(&dummy_frame(own))?;
                let handling = self.adapter.strike_handling();
                let floor = if handling.planned_dive {
                    Some(super::strike::planned_pitch_floor_with(
                        own,
                        if handling.strafe {
                            super::strike::STRAFE_CLEARANCE_FT
                        } else {
                            super::strike::PULL_OUT_CLEARANCE_FT
                        },
                    ))
                } else {
                    general
                };
                fly.terrain_floor_deg = floor;
                if super::rotor_adapter::flies(&self.flight) {
                    // Helicopters and the V-22 (slice H1).
                    self.fly_rotor(
                        super::rotor_adapter::RotorRequest::Steer {
                            intent,
                            terrain_pitch_floor_deg: floor,
                        },
                        own,
                        surface,
                        &mut fly,
                    )
                } else {
                    let output = self.adapter.controls(
                        &self.flight,
                        intent,
                        &own.limits,
                        own.g_limit,
                        own.roll_limit_deg_per_s,
                        own.maximum_bank_deg,
                        floor,
                        flight::DT,
                    )?;
                    let input = output.input.clone();
                    fly.adapter = Some(output);
                    input
                }
            }
            None => PilotInput::default(),
        };
        self.fly_input(input, ground, surface);
        fly.achieved = self.achieved();
        self.trace.fly = Some(fly);
        Ok(())
    }

    /// One tick of the rotor adapter (slice H1): the controls, with what
    /// it commanded in the trace.
    fn fly_rotor(
        &mut self,
        request: super::rotor_adapter::RotorRequest<'_>,
        own: &OwnState,
        surface: &dyn Fn(f64, f64) -> Surface,
        fly: &mut FlyTrace,
    ) -> PilotInput {
        let [x, _, z] = self.flight.position;
        let here = surface(x, z);
        let (input, command) = self.rotor.controls(
            &self.flight,
            request,
            here.height,
            here.wind,
            own.maximum_bank_deg,
        );
        fly.adapter = Some(super::steering_adapter::AdapterOutput {
            input: input.clone(),
            fallbacks: Vec::new(),
            requested: super::steering::SteeringState {
                heading_deg: command.heading_deg,
                flight_path_pitch_deg: command.flight_path_pitch_deg,
                bank_deg: command.bank_deg,
                body_pitch_offset_deg: 0.0,
            },
        });
        fly.rotor = Some(command);
        input
    }

    /// A hover or slow-flight request (slice H1, for the vertical airfield
    /// sequences of slice H2): the rotor adapter's hover on a helicopter or
    /// the V-22, the jet-lift law on the AV-8 and Yak-141. An aircraft that
    /// cannot hover is steered toward the point, level, at its minimum
    /// speed.
    fn fly_hover(
        &mut self,
        guidance: &super::rotor_adapter::HoverGuidance,
        own: &OwnState,
        surface: &dyn Fn(f64, f64) -> Surface,
        height: &dyn Fn(f64, f64) -> f64,
        fly: &mut FlyTrace,
    ) -> PilotInput {
        if super::rotor_adapter::flies(&self.flight) {
            return self.fly_rotor(
                super::rotor_adapter::RotorRequest::Hover(*guidance),
                own,
                surface,
                fly,
            );
        }
        let [x, _, z] = self.flight.position;
        if super::jet_lift::flies(&self.flight) {
            let (input, command) = self.jet_lift.controls(&self.flight, guidance, height(x, z));
            fly.jet_lift = Some(command);
            return input;
        }
        let heading = (guidance.point[0] - x)
            .atan2(guidance.point[1] - z)
            .to_degrees()
            .rem_euclid(360.0);
        let bank = super::motion::Bank::Unconstrained;
        let duration = super::motion::Duration::Timed(1);
        let intent = MotionIntent {
            id: 0,
            request: super::motion::MotionRequest::new(
                heading.round() as i32,
                super::motion::PitchRequest::Explicit(0),
                bank,
                super::motion::SpeedRequest::Explicit(own.limits.minimum),
                duration,
            ),
            heading_deg: heading,
            flight_path_pitch_deg: 0.0,
            speed: own.limits.minimum,
            bank,
            completion: super::controller::Completion::Axis(super::motion::CompletionAxis::Heading),
            steering_point: None,
            mode: super::steering::CommandMode::OtherState,
            formation_flight: false,
            afterburner: false,
        };
        let output = self.adapter.controls(
            &self.flight,
            &intent,
            &own.limits,
            own.g_limit,
            own.roll_limit_deg_per_s,
            own.maximum_bank_deg,
            None,
            flight::DT,
        );
        fly.intent = Some(intent);
        match output {
            Ok(output) => {
                let input = output.input.clone();
                fly.adapter = Some(output);
                input
            }
            Err(_) => PilotInput::default(),
        }
    }

    fn fly_input(
        &mut self,
        mut input: PilotInput,
        ground: &dyn Fn(f64, f64) -> f64,
        surface: &dyn Fn(f64, f64) -> Surface,
    ) {
        let response =
            super::damage::Response::assess(&self.flight.systems, self.flight.damage_fraction);
        let safety = super::damage::safety_power(
            &self.flight,
            self.speed_limits().minimum.0,
            self.flight.position[1] - ground(self.flight.position[0], self.flight.position[2]),
            self.airfield_phase(),
        ) || self.airfield.as_ref().is_some_and(|s| s.climbing_away());
        let ground_hold = self.damage_return.is_some()
            && self.flight.research.as_ref().is_some_and(|r| r.on_ground)
            && !matches!(
                self.airfield_phase(),
                Some(
                    super::airfield::Phase::Final
                        | super::airfield::Phase::Rollout
                        | super::airfield::Phase::TaxiClear
                        | super::airfield::Phase::Parked
                        | super::airfield::Phase::VerticalDescent
                        | super::airfield::Phase::Settle
                        | super::airfield::Phase::AirTaxi
                )
            );
        // The engine-protection throttle cut is a jet's: on a helicopter the
        // throttle is the collective, and a quarter of it is a fall (a hit
        // AH-64 dived into the ground on it, slice ST-H). Opinionated (agent).
        if !ground_hold && !self.is_rotorcraft() {
            super::damage::controls(response, &self.flight, safety, &mut input);
        }
        self.trace.damage.safety_power = safety && response.power == super::damage::Power::Protect;
        self.trace.damage.throttle_requested = input.throttle;
        self.trace.damage.throttle_locked = self.flight.systems.controls.throttle_lock;
        // Airborne actors on the legacy adapter keep the terrain-only surface
        // they have always used; researched actors see runways and wind.
        if self.flight.research.is_some() {
            self.flight.step_surface(&input, surface);
        } else {
            self.flight
                .step_surface(&input, |x, z| Surface::terrain(ground(x, z)));
        }
        self.last_input = input;
    }
}

/// A frame carrying only what the terrain floor needs.
fn dummy_frame(own: &OwnState) -> DecisionFrame<'static> {
    DecisionFrame {
        tick: 0,
        own: *own,
        targets: &[],
        events: &[],
        stations: &[],
        dispensers: &[],
        wing: WingView {
            control: WingControl::Loose,
            formation: Formation::Echelon,
            horizontal_spacing_ft: 2048,
            vertical_spacing_ft: 512,
            slot: 1,
            leader: None,
            wingmen_in_formation: 0,
            wing_combat: false,
            wing_approach: false,
            wing_approach_value_ft: None,
        },
        route: RouteView {
            home_airport: None,
            leader_is_ai: true,
        },
        now: TimeOfDay(0),
        flight_state: FlightState::Free,
    }
}

/// The record's minimum-altitude value; 300 in every inspected record (B44).
/// Traffic avoidance (John, 2026-09-29; numbers are agent decisions): look
/// this far ahead for a closest approach, seconds.
const AVOID_HORIZON_S: f64 = 6.0;
/// Miss distance that counts as a conflict, feet, plus this many seconds of
/// the closing speed (a head-on pair at 1,000 kt closure needs about 1,150 ft).
const AVOID_SEPARATION_FT: f64 = 300.0;
const AVOID_CLOSURE_S: f64 = 0.5;
/// Miss distance that counts as a conflict with the aircraft's own target.
const AVOID_TARGET_FT: f64 = 150.0;
/// Heading change away from the conflict, degrees: modest, not a break.
const AVOID_TURN_DEG: f64 = 30.0;
/// Within this many degrees of the nose the other aircraft is head-on.
const AVOID_HEAD_ON_DEG: f64 = 20.0;
/// An avoidance holds its side at least this long, ticks (3 s).
const AVOID_HOLD_TICKS: u64 = 360;

/// B44's retail terrain look-ahead, feet; the floor of the speed-scaled one.
/// Fitted (agent decision, 2026-09-29): the loaded lift, in G, the AI keeps
/// in hand at its planned top speed.
pub const TOP_SPEED_G_MARGIN: f64 = 1.2;
/// Fitted (agent decision, 2026-09-29): the loaded lift, in G, a final
/// approach keeps in hand to arrest a sink and follow its path. 1.15 since
/// the weight-scaled stall speeds (1.3 before, which then asked for more
/// than the 174 kt final cap).
pub const APPROACH_G_MARGIN: f64 = 1.15;
pub const TERRAIN_LOOKAHEAD_MIN_FT: f64 = 1000.0;
/// Opinionated (requested by John, 2026-09-29; number an agent decision):
/// the terrain floor looks this many seconds of travel ahead, never less than
/// [`TERRAIN_LOOKAHEAD_MIN_FT`]. Derivation: an AI fighter may be banked
/// 90 degrees when the floor bites; rolling level at the 45 degree per second
/// cap takes 2 s, the pitch loop below the floor closes over 1 s, and at a
/// supersonic 1,800 ft/s with the 2.2 G a fighter has near the top of its
/// envelope, raising the flight path by the 14 degrees of the steepest
/// hillsides the battery met takes about 3 s more (angle times speed over
/// 32.174 times 1.2 G of excess). Six seconds therefore lets it clear a
/// rising hillside it would otherwise meet 0.6 s after seeing it; at 440 kt
/// it is 4,500 ft, and below 100 kt the retail 1,000 ft still applies.
/// Above corner speed [`terrain_lookahead_ft`] may look further, see there.
pub const TERRAIN_LOOKAHEAD_S: f64 = 6.0;
/// Fitted (agent decision, 2026-10-20, fuzz seed 43): the roll level and
/// the pitch loop's settling before the pull counts, seconds (the 2 s and
/// 1 s of [`TERRAIN_LOOKAHEAD_S`]).
pub const TERRAIN_SETTLE_S: f64 = 3.0;
/// See [`TERRAIN_SETTLE_S`]: the steepest hillside the look-ahead plans to
/// climb, degrees (as in [`TERRAIN_LOOKAHEAD_S`]).
pub const TERRAIN_HILLSIDE_DEG: f64 = 14.0;
/// See [`TERRAIN_SETTLE_S`]: the excess G assumed when the envelope leaves
/// less, so the look-ahead stays bounded.
pub const TERRAIN_EXCESS_G_MIN: f64 = 0.25;
/// See [`TERRAIN_SETTLE_S`]: the longest look-ahead, seconds of travel.
pub const TERRAIN_LOOKAHEAD_MAX_S: f64 = 20.0;
/// Ground samples along the look-ahead, at most one per this many feet.
const TERRAIN_SAMPLE_FT: f64 = 1000.0;
/// At most this many samples, so long look-aheads stay cheap.
const TERRAIN_SAMPLES_MAX: f64 = 12.0;

/// How far ahead the B44 terrain floor looks at this speed, feet.
///
/// `pull_excess_g` is the G above 1 the envelope leaves the aircraft, given
/// above corner speed. Fitted (agent decision, 2026-10-20, fuzz seed 43):
/// the look-ahead then covers [`TERRAIN_SETTLE_S`] plus the time that excess
/// takes to raise the flight path by [`TERRAIN_HILLSIDE_DEG`] (angle times
/// speed over 32.174 times the excess), between [`TERRAIN_LOOKAHEAD_S`] and
/// [`TERRAIN_LOOKAHEAD_MAX_S`]. Near its top speed a loaded F-22N keeps only
/// about 1.5 G; at 1,030 kt and 3,000 ft its six seconds (10,300 ft) saw a
/// 17 degree escarpment too late, and it flew into it pulling full stick.
pub fn terrain_lookahead_ft(speed_fps: f64, pull_excess_g: Option<f64>) -> f64 {
    let speed = speed_fps.max(0.0);
    let seconds = pull_excess_g.map_or(TERRAIN_LOOKAHEAD_S, |excess| {
        (TERRAIN_SETTLE_S
            + TERRAIN_HILLSIDE_DEG.to_radians() * speed
                / (32.174 * excess.max(TERRAIN_EXCESS_G_MIN)))
        .clamp(TERRAIN_LOOKAHEAD_S, TERRAIN_LOOKAHEAD_MAX_S)
    });
    (speed * seconds).max(TERRAIN_LOOKAHEAD_MIN_FT)
}

#[cfg(test)]
mod terrain_lookahead_tests {
    use super::*;

    #[test]
    fn above_corner_the_look_ahead_covers_the_pull_the_envelope_leaves() {
        // Fuzz seed 43: a loaded F-22N at 1,030 kt (1,740 ft/s) keeps about
        // 1.5 G. Six seconds of travel was too short to raise its path over
        // a 17 degree escarpment; the pull it has needs the full 20 s.
        assert_eq!(terrain_lookahead_ft(1740.0, Some(0.5)), 1740.0 * 20.0);
        // Plenty of G: never shorter than the six seconds.
        assert_eq!(terrain_lookahead_ft(800.0, Some(6.0)), 800.0 * 6.0);
        // In between: settle time plus the 14 degree pull.
        let expected = 1800.0 * (3.0 + 14.0_f64.to_radians() * 1800.0 / (32.174 * 1.2));
        assert!((terrain_lookahead_ft(1800.0, Some(1.2)) - expected).abs() < 1e-6);
        // No excess at all is bounded, and slow flight keeps 1,000 ft.
        assert_eq!(terrain_lookahead_ft(1740.0, Some(-0.3)), 1740.0 * 20.0);
        assert_eq!(terrain_lookahead_ft(100.0, Some(0.0)), 1000.0);
    }

    #[test]
    fn the_look_ahead_scales_with_speed_and_sees_ridges_inside_it() {
        // John, 2026-09-29: 1,000 ft at low speed, six seconds of travel fast.
        assert_eq!(terrain_lookahead_ft(100.0, None), 1000.0);
        assert_eq!(terrain_lookahead_ft(1500.0, None), 9000.0);
        // A 2,000 ft ridge 4,000 ft ahead of an aircraft at 1,000 ft, flying
        // north: seen, and the climb to clear it by 300 ft is the gradient.
        let ridge = |_: f64, z: f64| {
            if (3500.0..4500.0).contains(&z) {
                2000.0
            } else {
                0.0
            }
        };
        let (highest, climb) = terrain_ahead_ft([0.0, 1000.0, 0.0], 0.0, 9000.0, 300.0, &ridge);
        assert_eq!(highest, 2000.0);
        let expected = (1300.0_f64).atan2(4000.0).to_degrees();
        assert!((climb - expected).abs() < 0.5, "{climb} vs {expected}");
        // The retail single point 1,000 ft ahead would have seen flat ground.
        let (near, _) = terrain_ahead_ft([0.0, 1000.0, 0.0], 0.0, 1000.0, 300.0, &ridge);
        assert_eq!(near, 0.0);
    }
}

/// The highest ground along `reach` feet of the horizontal track, sampled
/// evenly and always including the end, so a ridge inside the look-ahead is
/// not skipped; and the flight-path climb, degrees, that clears every sample
/// by `clearance_ft` (-90 when none is needed).
pub fn terrain_ahead_ft(
    position: [f64; 3],
    heading_rad: f64,
    reach_ft: f64,
    clearance_ft: f64,
    ground: &dyn Fn(f64, f64) -> f64,
) -> (f64, f64) {
    let samples = (reach_ft / TERRAIN_SAMPLE_FT)
        .ceil()
        .clamp(1.0, TERRAIN_SAMPLES_MAX) as u32;
    let mut highest = f64::NEG_INFINITY;
    let mut climb = -90.0_f64;
    for k in 1..=samples {
        let d = reach_ft * f64::from(k) / f64::from(samples);
        let h = ground(
            position[0] + heading_rad.sin() * d,
            position[2] + heading_rad.cos() * d,
        );
        highest = highest.max(h);
        climb = climb.max((h + clearance_ft - position[1]).atan2(d).to_degrees());
    }
    (highest, climb)
}

pub const MINIMUM_ALTITUDE_FT: f64 = 300.0;
/// Fitted: AGL at or below this counts as ground contact for the B44 overrides.
pub const GROUND_CONTACT_FT: f64 = 5.0;
/// Fitted: the reference maximum bank for the B44 authority curve.
///
/// The spec says the reference bank is "the aircraft's own maximum bank" but
/// the imported records do not expose one, so the host uses a single value for
/// every ported fighter. Agent decision, 2026-09-17.
pub const MAXIMUM_BANK_DEG: f64 = 80.0;
/// Fitted fixture band used only when no envelope covers the altitude.
pub const FALLBACK_MINIMUM_FPS: f64 = 220.0;
/// See [`FALLBACK_MINIMUM_FPS`].
pub const FALLBACK_MAXIMUM_FPS: f64 = 1600.0;
/// See [`FALLBACK_MINIMUM_FPS`].
pub const FALLBACK_CORNER_FPS: f64 = 700.0;
/// Fitted floor on fuel flow so a shut-down engine cannot report an infinite
/// endurance.
pub const MINIMUM_FUEL_FLOW_LBS_PER_S: f64 = 0.01;

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Angle between the actor's nose and a point, in degrees.
fn pointing_error_deg(own: &OwnState, point: [f64; 3]) -> f64 {
    let dx = point[0] - own.position[0];
    let dy = point[1] - own.position[1];
    let dz = point[2] - own.position[2];
    let horizontal = (dx * dx + dz * dz).sqrt();
    if horizontal == 0.0 && dy == 0.0 {
        return 0.0;
    }
    let bearing = dx.atan2(dz).to_degrees();
    let elevation = dy.atan2(horizontal).to_degrees();
    let heading_error = wrap_signed(bearing - own.heading_deg).abs();
    let pitch_error = (elevation - own.flight_path_pitch_deg).abs();
    heading_error.max(pitch_error)
}

fn wrap_signed(value: f64) -> f64 {
    let mut v = value % 360.0;
    if v > 180.0 {
        v -= 360.0;
    }
    if v < -180.0 {
        v += 360.0;
    }
    v
}

/// Build one AI actor's stores from a simple description.
///
/// Convenience for hosts and fixtures that do not carry a full imported
/// loadout. It creates finite ammunition only; there is no unlimited path.
pub fn simple_stations(
    air_to_air_rounds: u32,
    gun_rounds: u32,
    store_speed: ScalarSpeed,
) -> Vec<StationSpec> {
    let mut stations = Vec::new();
    if air_to_air_rounds > 0 {
        stations.push(StationSpec {
            station: StationId(0),
            guided: true,
            capability: StoreCapability::AIR_TO_AIR_MISSILE,
            store: StoreState {
                inhibited: false,
                rounds: Rounds::Finite(air_to_air_rounds),
            },
            debit: 1,
            external_round_lbs: 0.0,
            projectile_count: 1,
            employment_limit_deg: Some(30.0),
            damage_vs_category: 100.0,
            store_speed,
            tracking_delay: Delay::seconds(1),
            pacing: ProjectilePacing {
                burst_count: 1,
                burst_interval: Delay::seconds(0),
                reload: Delay::seconds(2),
                startup: Delay::seconds(0),
            },
            minimum_range_ft: 0.0,
            infrared_envelope: None,
            requires_radar: false,
            requires_sensor: false,
            employment_zone: None,
            mount: [0.0; 3],
            maximum_range_ft: Some(40000.0),
            energy_target: None,
        });
    }
    if gun_rounds > 0 {
        stations.push(StationSpec {
            station: StationId(1),
            guided: false,
            capability: StoreCapability::GUN,
            store: StoreState {
                inhibited: false,
                rounds: Rounds::Finite(gun_rounds),
            },
            debit: 10,
            external_round_lbs: 0.0,
            projectile_count: 10,
            employment_limit_deg: Some(5.0),
            damage_vs_category: 10.0,
            store_speed: ScalarSpeed(3000.0),
            tracking_delay: Delay::quarters(1),
            pacing: ProjectilePacing {
                burst_count: 10,
                burst_interval: Delay::quarters(1),
                reload: Delay::quarters(2),
                startup: Delay::seconds(0),
            },
            minimum_range_ft: 0.0,
            infrared_envelope: None,
            requires_radar: false,
            requires_sensor: false,
            employment_zone: None,
            mount: [0.0; 3],
            maximum_range_ft: Some(6000.0),
            energy_target: None,
        });
    }
    stations
}

/// The standard two-class dispenser fit for an AI fighter.
pub fn simple_dispensers(each: u32) -> Vec<DispenserStore> {
    vec![
        DispenserStore {
            class: SeekerClass::Infrared,
            count: each,
        },
        DispenserStore {
            class: SeekerClass::Radar,
            count: each,
        },
    ]
}

/// Whether an aircraft has a reviewed retail donor.
pub fn is_ported(aircraft: AircraftId) -> bool {
    AircraftId::ALL.contains(&aircraft.source())
}

/// The health-scaled positive G request. The caution factor never takes the
/// request below [`HEALTH_SCALED_G_FLOOR`], or the healthy limit where that is
/// lower: a damaged aircraft that the flight model still lets pull 1 G must be
/// able to hold its flight path. Without the floor a hit but fault-free
/// aircraft flying a heavy final was held below 1 G and flew into the ground
/// short of the runway (review of branch ai-defense, 2026-10-25).
pub(crate) fn health_scaled_positive_g(positive: f64, health: f64) -> f64 {
    (positive * health).max(positive.min(HEALTH_SCALED_G_FLOOR))
}

/// Fitted (agent decision, 2026-10-25): the lowest positive G the health
/// caution factor asks for.
pub const HEALTH_SCALED_G_FLOOR: f64 = 1.25;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::controller::{BehaviorFamily, MissionRole};
    use crate::ai::experience::ExperienceOrigin;
    use crate::ai::targeting::Side;
    use crate::ai::{Experience, weapon_service::ActorId};

    fn aircraft_index(id: AircraftId) -> usize {
        AircraftId::SELECTABLE
            .iter()
            .position(|item| *item == id)
            .unwrap()
    }

    // Distinct synthetic capabilities, never presented as measured retail data.
    pub(super) fn synthetic_profile(id: AircraftId) -> tore_formats::aircraft::Aircraft {
        let mut profile = crate::flight::integration_tests::profile();
        let index = aircraft_index(id);
        profile.id = id;
        profile.name = match id {
            AircraftId::F18 => "F/A-18D",
            AircraftId::Rafale => "RAFALE",
            AircraftId::F14 => "F-14",
            AircraftId::A4E => "A-4E",
            AircraftId::X31 => "X-31",
            AircraftId::Mig29 => "MiG-29",
            AircraftId::Su27 => "Su-27",
            AircraftId::Mig21 => "MiG-21",
            AircraftId::Su25 => "Su-25",
            AircraftId::Mig23 => "MiG-23",
            AircraftId::Su35 => "Su-35",
            AircraftId::F22 | AircraftId::F22n | AircraftId::Faxx => "F-22",
            _ => id.reviewed_names().expect("reviewed variety identity").0,
        }
        .into();
        profile.shape = format!("{}.SH", id.stem());
        profile.fields.get_mut("aftThrust").unwrap().value =
            if matches!(id, AircraftId::A4E | AircraftId::Su25) {
                "0".into()
            } else {
                (400 + index * 10).to_string()
            };
        for envelope in &mut profile.envelopes {
            for point in &mut envelope.points {
                point[0] *= 0.85 + index as f64 * 0.025;
            }
        }
        profile
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
            origin: ExperienceOrigin::QuickMission { selected: level },
        }
    }

    fn flight_state(position: [f64; 3], yaw: f64) -> flight::State {
        let aircraft = crate::flight::integration_tests::profile();
        let mut state = flight::State::new(&aircraft, position).unwrap();
        state.yaw = yaw;
        let basis = crate::attitude::Basis::new(yaw, 0.0, 0.0);
        state.velocity = basis.forward.map(|v| v * state.speed);
        state
    }

    pub(super) fn setup(
        id: u32,
        side: u32,
        member: u8,
        position: [f64; 3],
        yaw: f64,
    ) -> ActorSetup {
        ActorSetup {
            identity: ActorIdentity {
                actor: ActorId(id),
                side: Side(side),
                wing: 0,
                member,
                leads: member == 0,
                aircraft: AircraftId::F18,
                human_controlled: false,
            },
            profile: profile(),
            experience: resolved(Experience::Experienced),
            seed: u64::from(id) * 31 + 7,
            flight: flight_state(position, yaw),
            sensors: None,
            stations: simple_stations(4, 500, ScalarSpeed(2000.0)),
            dispensers: simple_dispensers(30),
            wing_slot: member.max(1),
            home_airport: Some(crate::ai::route::Position { x: 0.0, z: 0.0 }),
        }
    }

    pub(super) fn object(actor: &AiActor, side: u32) -> WorldObject {
        WorldObject {
            infrared_strength: 1.,
            id: actor.id(),
            side: Side(side),
            position: actor.flight().position,
            velocity: actor.flight().velocity,
            heading_deg: actor.flight().yaw.to_degrees(),
            pitch_deg: actor.flight().pitch.to_degrees(),
            speed: ScalarSpeed(actor.flight().speed),
            maximum_speed: ScalarSpeed(1600.0),
            is_aircraft: true,
            is_fighter: true,
            human_controlled: false,
            alive: actor.alive(),
            destroyed: false,
            on_ground: false,
            observable: None,
        }
    }

    pub(super) fn flat(_x: f64, _z: f64) -> f64 {
        0.0
    }

    pub(super) fn perception_actor(level: Experience) -> AiActor {
        let mut setup = setup(1, 1, 0, [0., 20000., 0.], 0.);
        setup.experience = resolved(level);
        setup.sensors = Some(Sensors::new(sensors::SensorProfiles {
            aircraft: AircraftId::F18,
            radar: None,
            infrared: None,
            visual: None,
            jammer: None,
            signature: sensors::SignatureProfile::default(),
        }));
        AiActor::new(setup).unwrap()
    }

    pub(super) fn visible_object(actor: &AiActor, id: u32, position: [f64; 3]) -> WorldObject {
        let mut target = object(actor, 2);
        target.id = id;
        target.position = position;
        target.observable = Some(Observable {
            id,
            position,
            velocity: target.velocity,
            basis: crate::attitude::Basis::new(0., 0., 0.),
            configuration: sensors::Configuration::default(),
            signature: sensors::SignatureProfile::default(),
            jammer: None,
            jammer_active: false,
            radar_emitting: false,
            airborne: true,
            destroyed: false,
        });
        target
    }

    #[test]
    fn ejection_preempts_weapons_and_keeps_pilot_descent_running_without_ai_control() {
        let mut mission = one_v_one();
        let actor = &mut mission.actors[0];
        let mut source = crate::flight::integration_tests::profile();
        source.fields.get_mut("flags").unwrap().value = "16".into();
        actor.flight = flight::State::new(&source, [0., 10000., 0.]).unwrap();
        actor.flight.crashed = true;
        actor.escape_monitor = crate::ejection::Monitor::seeded(0);
        let id = actor.id();
        let out = run(&mut mission, 120);
        assert!(!out.launches.iter().any(|e| e.actor == id));
        let actor = mission.actor(id).unwrap();
        assert!(actor.flight.escape.is_some());
        let aircraft_position = actor.flight.position;
        let pilot_position = actor.flight.escape.as_ref().unwrap().position;
        run(&mut mission, 120);
        let actor = mission.actor(id).unwrap();
        assert_eq!(
            actor.flight.position, aircraft_position,
            "combat owns the wreck"
        );
        assert_ne!(
            actor.flight.escape.as_ref().unwrap().position,
            pilot_position
        );
        assert!(!actor.alive());
    }

    #[test]
    fn a_gun_break_changes_the_flown_path_and_increases_clearance() {
        fn encounter(defend: bool, hybrid: bool) -> (f64, f64) {
            let mut start = setup(1, 1, 0, [0., 20000., 0.], 0.);
            start.flight =
                flight::State::new(&synthetic_profile(AircraftId::Su27), start.flight.position)
                    .unwrap();
            start.flight.yaw = 0.;
            start.flight.velocity = [0., 0., start.flight.speed];
            start.home_airport = None;
            if hybrid {
                start.flight.enable_research(1).unwrap();
            }
            let mut actor = AiActor::new(start).unwrap();
            actor.set_assignment(engagement::Assignment {
                stance: engagement::Stance::WeaponsHold,
                ..Default::default()
            });
            let mut mission = AiMission::new();
            mission.push(actor);
            mission.start_in_formation();
            let mut nearest = f64::INFINITY;
            let mut peak_bank = 0.0_f64;
            for tick in 0..360 {
                let position = [0., 20000., 6000. - (tick as f64 + 1.) * 2000. / 120.];
                if defend {
                    mission.set_gun_rounds(vec![incoming_fire::Round {
                        id: 90,
                        owner: 99,
                        position,
                        previous: [0., 20000., position[2] + 2000. / 120.],
                        tracer: true,
                    }]);
                }
                let output = mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
                assert!(output.launches.is_empty() && output.devices.is_empty());
                let aircraft = mission.actor(1).unwrap().flight();
                nearest = nearest.min(distance(aircraft.position, position));
                peak_bank = peak_bank.max(aircraft.bank.abs().to_degrees());
            }
            (nearest, peak_bank)
        }
        for hybrid in [false, true] {
            let (undefended, _) = encounter(false, hybrid);
            let (defended, bank) = encounter(true, hybrid);
            assert!(
                bank > 10.,
                "hybrid={hybrid}: bank {bank}, clearance {defended} vs {undefended}"
            );
            assert!(
                defended > undefended + 25.,
                "hybrid={hybrid}: {defended} vs {undefended}"
            );
        }
    }

    #[test]
    fn anonymous_hit_defends_without_target_at_every_skill_and_in_both_flight_models() {
        // This is the established AI roster. The variety batch adds player
        // flight families without adding an AI calibration or acceptance claim.
        for aircraft in [
            AircraftId::F18,
            AircraftId::Rafale,
            AircraftId::F14,
            AircraftId::A4E,
            AircraftId::X31,
            AircraftId::Mig29,
            AircraftId::Su27,
            AircraftId::Mig21,
            AircraftId::Su25,
            AircraftId::Mig23,
            AircraftId::Su35,
            AircraftId::F22,
            AircraftId::F22n,
            AircraftId::Faxx,
        ] {
            for skill in Experience::ALL {
                for hybrid in [false, true] {
                    let mut start = setup(1, 1, 0, [0., 5000., 0.], 0.);
                    start.identity.aircraft = aircraft;
                    start.flight =
                        flight::State::new(&synthetic_profile(aircraft), start.flight.position)
                            .unwrap();
                    if hybrid {
                        start.flight.enable_research(1).unwrap();
                    }
                    start.home_airport = None;
                    start.experience = resolved(skill);
                    let mut actor = AiActor::new(start).unwrap();
                    actor.set_assignment(engagement::Assignment {
                        stance: engagement::Stance::WeaponsHold,
                        ..Default::default()
                    });
                    let mut mission = AiMission::new();
                    mission.push(actor);
                    mission.start_in_formation();
                    mission.actor_mut(1).unwrap().report_hit();
                    let first = mission.step(&[], &flat, TimeOfDay(0)).unwrap();
                    assert!(first.launches.is_empty() && first.devices.is_empty());
                    let actor = mission.actor(1).unwrap();
                    assert_eq!(actor.controller.target(), None);
                    assert_eq!(actor.activity(), Activity::Defending);
                    assert!(actor.trace.fire.selected);
                    let mut peak_bank = 0.0_f64;
                    for tick in 1..240 {
                        let output = mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
                        assert!(output.launches.is_empty() && output.devices.is_empty());
                        let actor = mission.actor(1).unwrap();
                        peak_bank = peak_bank.max(actor.flight.bank.abs());
                        assert!(actor.flight.position.iter().all(|v| v.is_finite()));
                        assert!(actor.flight.position[1] > 1000.);
                    }
                    assert!(
                        peak_bank.to_degrees() > 1.,
                        "{aircraft:?} {skill:?} hybrid={hybrid}: no actual bank"
                    );
                    mission.step(&[], &flat, TimeOfDay(240)).unwrap();
                    let actor = mission.actor(1).unwrap();
                    assert!(actor.incoming_fire_cue().is_none());
                    assert_ne!(actor.activity(), Activity::Defending);
                    assert!(actor.is_neutral());
                }
            }
        }
    }

    #[test]
    fn lookout_acquires_rear_contact_and_recall_does_not_release_again() {
        for skill in Experience::ALL {
            let mut mission = AiMission::new();
            mission.push(perception_actor(skill));
            mission.start_in_formation();
            let target = visible_object(mission.actor(1).unwrap(), 2, [0., 20000., -5000.]);
            let mut seen = None;
            for tick in 0..240 {
                mission
                    .step(std::slice::from_ref(&target), &flat, TimeOfDay(tick))
                    .unwrap();
                if mission
                    .actor(1)
                    .unwrap()
                    .awareness
                    .current_observations()
                    .any(|s| s.target.id == 2)
                {
                    seen = Some(tick);
                    break;
                }
            }
            assert!(seen.is_some(), "{skill:?}");
            assert!(!mission.actor(1).unwrap().is_neutral());
            mission
                .order(
                    1,
                    super::super::wing::WingRequest::TargetAssignment(
                        super::super::wing::TargetOrder::HoldFire,
                    ),
                )
                .unwrap()
                .unwrap();
            for tick in 0..480 {
                mission
                    .step(std::slice::from_ref(&target), &flat, TimeOfDay(tick))
                    .unwrap();
                assert!(mission.actor(1).unwrap().is_neutral());
                assert_eq!(mission.actor(1).unwrap().controller.target(), None);
            }
        }
    }

    #[test]
    fn contact_release_respects_hold_self_defense_and_assigned_target() {
        for (assignment, release) in [
            (engagement::Assignment::default(), true),
            (
                engagement::Assignment {
                    stance: engagement::Stance::WeaponsHold,
                    ..Default::default()
                },
                false,
            ),
            (
                engagement::Assignment {
                    stance: engagement::Stance::SelfDefense,
                    ..Default::default()
                },
                false,
            ),
            (
                engagement::Assignment {
                    role: engagement::Role::Intercept,
                    destroy_ids: vec![99],
                    ..Default::default()
                },
                false,
            ),
            (
                engagement::Assignment {
                    role: engagement::Role::Intercept,
                    destroy_ids: vec![2],
                    ..Default::default()
                },
                true,
            ),
        ] {
            let mut mission = AiMission::new();
            let mut actor = perception_actor(Experience::Average);
            actor.set_assignment(assignment);
            mission.push(actor);
            mission.start_in_formation();
            let target = visible_object(mission.actor(1).unwrap(), 2, [0., 20000., 5000.]);
            mission.step(&[target], &flat, TimeOfDay(0)).unwrap();
            assert_eq!(!mission.actor(1).unwrap().is_neutral(), release);
        }
    }

    #[test]
    fn skill_attention_is_independent_of_imported_visual_hardware_and_blocks_terrain() {
        for level in Experience::ALL {
            let mut actor = perception_actor(level);
            let range = awareness::visual_range_feet(level);
            let target = visible_object(&actor, 2, [0., 20000., range]);
            assert_eq!(
                actor.observe(0, std::slice::from_ref(&target), &flat).len(),
                1
            );
            let too_far = visible_object(&actor, 2, [0., 20000., range + 1.]);
            assert!(actor.observe(1, &[too_far], &flat).is_empty());
            let behind = visible_object(&actor, 2, [0., 20000., -1000.]);
            assert!(actor.observe(2, &[behind], &flat).is_empty());
            let ridge = |_: f64, z: f64| {
                if z > range / 4. && z < range * 0.75 {
                    21000.
                } else {
                    0.
                }
            };
            assert!(actor.observe(3, &[target], &ridge).is_empty());
        }
    }

    #[test]
    fn measured_pose_is_frozen_and_lifecycle_removes_hidden_memories() {
        let mut actor = perception_actor(Experience::Ace);
        let mut target = visible_object(&actor, 2, [0., 20000., 5000.]);
        // A sensor snapshot can differ from later host body metadata. The
        // measured position must be the one retained and used for geometry.
        target.position = [90000., 22000., 90000.];
        let views = actor.observe(10, std::slice::from_ref(&target), &flat);
        assert_eq!(views[0].position, [0., 20000., 5000.]);
        let frozen = *actor.awareness.snapshot(2).unwrap();
        target.observable = None;
        target.position = [-90000., 1000., -90000.];
        target.velocity = [100., -100., 500.];
        assert!(
            actor
                .observe(11, std::slice::from_ref(&target), &flat)
                .is_empty()
        );
        assert_eq!(actor.awareness.snapshot(2), Some(&frozen));
        actor.observe(1_000_000, std::slice::from_ref(&target), &flat);
        assert_eq!(actor.awareness.snapshot(2), Some(&frozen));
        target.destroyed = true;
        actor.observe(1_000_001, &[target], &flat);
        assert!(actor.awareness.snapshot(2).is_none());
    }

    #[test]
    fn novice_kill_does_not_recover_a_forgotten_hostile_without_a_new_observation() {
        let mut actor = perception_actor(Experience::Novice);
        let mut first = visible_object(&actor, 2, [0., 20000., 5000.]);
        let mut second = visible_object(&actor, 3, [1000., 20000., 5000.]);
        assert_eq!(
            actor
                .observe(0, &[first.clone(), second.clone()], &flat)
                .len(),
            2
        );
        actor.awareness.select_target(Some(2));
        assert!(actor.awareness.snapshot(3).is_none());
        first.destroyed = true;
        second.observable = None;
        assert!(actor.observe(1, &[first.clone(), second], &flat).is_empty());
        actor.update_search_contact(&[]);
        assert!(actor.awareness.remembered().next().is_none());
        assert_eq!(actor.search_target, None);
        let fresh = visible_object(&actor, 3, [1000., 20000., 5000.]);
        assert_eq!(actor.observe(2, &[first, fresh], &flat).len(), 1);
        actor.awareness.select_target(Some(3));
        assert!(actor.awareness.snapshot(3).is_some());
    }

    #[test]
    fn lost_contact_inside_retention_distance_searches_without_firing_then_reacquires() {
        let mut mission = AiMission::new();
        // Written for observation every tick.
        mission.set_test_settings(AiTestSettings::every_tick());
        mission.push(perception_actor(Experience::Novice));
        let target = visible_object(mission.actor(1).unwrap(), 2, [0., 20000., 5000.]);
        mission
            .step(std::slice::from_ref(&target), &flat, TimeOfDay(0))
            .unwrap();
        assert_eq!(mission.actor(1).unwrap().controller.target(), Some(2));
        let snapshot = *mission.actor(1).unwrap().awareness.snapshot(2).unwrap();
        let rounds = mission.actor(1).unwrap().rounds_remaining();
        let mut hidden = target.clone();
        hidden.observable = None;
        hidden.position = [3000., 20000., 4000.];
        for tick in 1..240 {
            let output = mission
                .step(std::slice::from_ref(&hidden), &flat, TimeOfDay(tick))
                .unwrap();
            let actor = mission.actor(1).unwrap();
            assert_eq!(actor.controller.target(), None);
            assert_eq!(actor.activity(), Activity::Searching);
            assert!(output.launches.is_empty());
            assert_eq!(actor.awareness.snapshot(2), Some(&snapshot));
        }
        assert_eq!(mission.actor(1).unwrap().rounds_remaining(), rounds);
        // Place a fresh observation in front of the actor after the search turn.
        let actor = mission.actor(1).unwrap();
        let forward =
            crate::attitude::Basis::new(actor.flight.yaw, actor.flight.pitch, actor.flight.bank)
                .forward;
        let position = std::array::from_fn(|i| actor.flight.position[i] + forward[i] * 5000.);
        let reacquired = visible_object(actor, 2, position);
        mission.step(&[reacquired], &flat, TimeOfDay(240)).unwrap();
        assert_eq!(mission.actor(1).unwrap().controller.target(), Some(2));
        assert_ne!(mission.actor(1).unwrap().activity(), Activity::Searching);
        assert_eq!(
            mission
                .actor(1)
                .unwrap()
                .awareness
                .snapshot(2)
                .unwrap()
                .last_observed_tick,
            240
        );
    }

    #[test]
    fn thirty_sensor_owned_actors_replay_observations_and_search_identically() {
        fn mission() -> AiMission {
            let mut mission = AiMission::new();
            for id in 1..=30 {
                let mut actor = perception_actor(Experience::ALL[(id as usize - 1) % 4]);
                // Each actor owns an independent sensor component and identity.
                let mut setup = setup(
                    id,
                    if id % 2 == 0 { 1 } else { 2 },
                    0,
                    [id as f64 * 250., 20000., id as f64 * 100.],
                    0.,
                );
                setup.experience = actor.controller.experience();
                setup.sensors = actor.sensors.take();
                mission.push(AiActor::new(setup).unwrap());
            }
            mission
        }
        fn advance(mission: &mut AiMission, tick: u64) -> MissionOutput {
            let world: Vec<_> = mission
                .actors()
                .iter()
                .map(|a| {
                    let mut o = visible_object(a, a.id(), a.flight.position);
                    o.side = a.identity.side;
                    o
                })
                .collect();
            mission.step(&world, &flat, TimeOfDay(tick)).unwrap()
        }
        let mut a = mission();
        let mut b = mission();
        for tick in 0..120 {
            assert_eq!(advance(&mut a, tick), advance(&mut b, tick));
            for (a, b) in a.actors().iter().zip(b.actors()) {
                assert_eq!(a.awareness(), b.awareness());
                assert_eq!(a.flight(), b.flight());
                assert_eq!(a.controller(), b.controller());
            }
        }
    }

    pub(super) fn enable_test_radar(actor: &mut AiActor) {
        let volume = sensors::Volume {
            azimuth_rad: 60_f64.to_radians(),
            elevation_rad: 60_f64.to_radians(),
            minimum_ft: 0.,
            maximum_ft: 20. * sensors::FEET_PER_NAUTICAL_MILE,
            minimum_relative_ft: f64::NEG_INFINITY,
            maximum_relative_ft: f64::INFINITY,
        };
        let preset = sensors::Preset::Advanced;
        actor.sensors.as_mut().unwrap().profiles.radar = Some(sensors::RadarProfile {
            record: "TEST.SEE".into(),
            search: volume,
            track: volume,
            look_down: 0.,
            preset,
            notch: preset.notch(),
            resistance: preset.resistance(),
            band: 0,
            source_flags: [0; 2],
            source_doppler: [0; 3],
        });
        actor.sensors.as_mut().unwrap().controls.range_index = 0;
    }

    #[test]
    fn radar_refreshes_memory_beyond_skill_visual_range_and_terrain_masks_both_channels() {
        for level in Experience::ALL {
            let mut actor = perception_actor(level);
            enable_test_radar(&mut actor);
            let target = visible_object(
                &actor,
                2,
                [0., 20000., 6. * sensors::FEET_PER_NAUTICAL_MILE],
            );
            assert_eq!(
                actor.observe(1, std::slice::from_ref(&target), &flat).len(),
                1
            );
            actor.awareness.select_target(Some(2));
            let snapshot = actor.awareness.snapshot(2).unwrap();
            assert_eq!(snapshot.source_ticks.radar, Some(1));
            assert_eq!(snapshot.source_ticks.visual, None);
            actor.observe(2, std::slice::from_ref(&target), &flat);
            assert_eq!(actor.awareness.snapshot(2).unwrap().last_observed_tick, 2);
            let ridge = |_: f64, z: f64| if z > 10000. && z < 20000. { 21000. } else { 0. };
            assert!(actor.observe(3, &[target], &ridge).is_empty());
            assert_eq!(actor.awareness.snapshot(2).unwrap().last_observed_tick, 2);
        }
    }

    #[test]
    fn hold_fire_clears_live_sensor_designation_without_erasing_memory() {
        let mut actor = perception_actor(Experience::Ace);
        enable_test_radar(&mut actor);
        let target = visible_object(&actor, 2, [0., 20000., 5000.]);
        let mut mission = AiMission::new();
        mission.push(actor);
        mission
            .step(std::slice::from_ref(&target), &flat, TimeOfDay(0))
            .unwrap();
        assert_eq!(
            mission.actor(1).unwrap().sensors().unwrap().selected(),
            Some(2)
        );
        mission
            .actor_mut(1)
            .unwrap()
            .order(
                super::super::wing::WingRequest::TargetAssignment(
                    super::super::wing::TargetOrder::HoldFire,
                ),
                1,
            )
            .unwrap();
        let output = mission.step(&[target], &flat, TimeOfDay(1)).unwrap();
        assert!(output.launches.is_empty());
        assert_eq!(
            mission.actor(1).unwrap().sensors().unwrap().selected(),
            None
        );
        assert!(mission.actor(1).unwrap().awareness.snapshot(2).is_some());
    }

    fn incoming_snapshot(
        guidance: crate::combat::missiles::Guidance,
        position: [f64; 3],
    ) -> MissileSnapshot {
        MissileSnapshot {
            id: 99,
            owner: 8,
            position,
            velocity: [0., 0., 2000.],
            guidance,
            target: Some(1),
            radar_active: false,
            radar_acquired: false,
            supported: guidance == crate::combat::missiles::Guidance::Supported,
            supporting_radar_position: Some([0., 20000., -60000.]),
            alive: true,
        }
    }

    #[test]
    fn lookout_sees_a_rear_passive_missile_without_revealing_the_launcher() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        mission.push(perception_actor(Experience::Novice));
        for tick in 0..83 {
            let mut missile = incoming_snapshot(
                Guidance::Infrared,
                [0., 20000., -10000. + tick as f64 * 2000. / 120.],
            );
            missile.target = Some(999);
            mission.set_missiles(vec![missile]);
            mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
            if tick < 80 {
                assert!(mission.actor(1).unwrap().defense_decision().is_none());
            }
        }
        let actor = mission.actor(1).unwrap();
        assert!(actor.defense_decision().unwrap().motion.is_none());
        assert!(
            actor
                .missile_threats()
                .any(|r| r.targeting_receiver && r.guidance_class.is_none())
        );
        assert!(actor.awareness().remembered().next().is_none());
        assert!(
            actor
                .perceived_attacks()
                .iter()
                .all(|r| r.report.attacker_id.is_none())
        );
    }

    #[test]
    fn ground_supported_missile_and_anonymous_hit_share_one_safe_response() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        let mut actor = perception_actor(Experience::Ace);
        actor.set_assignment(engagement::Assignment {
            stance: engagement::Stance::WeaponsHold,
            ..Default::default()
        });
        actor.report_hit();
        mission.push(actor);
        let mut missile = incoming_snapshot(Guidance::Supported, [0., 20000., -1000.]);
        missile.supporting_radar_position = Some([0., 10., -60000.]);
        mission.set_missiles(vec![missile]);
        let output = mission.step(&[], &flat, TimeOfDay(0)).unwrap();
        let actor = mission.actor(1).unwrap();
        assert_eq!(actor.controller.target(), None);
        assert_eq!(actor.activity(), Activity::Defending);
        assert_eq!(
            actor.trace.fire.cue.unwrap().evidence,
            incoming_fire::Evidence::Hit
        );
        assert!(
            actor
                .defense_decision()
                .unwrap()
                .debug
                .estimated_threat_time_s
                .is_some_and(|t| t > 0.)
        );
        assert!(
            actor.trace.fire.selected,
            "the hit is more urgent than the approaching missile"
        );
        assert!(matches!(
            actor.controller.trace().motion.branch,
            thought::MotionBranch::IncomingFire { .. }
        ));
        assert_eq!(output.devices.len(), 1);
        assert_eq!(output.devices[0].class, SeekerClass::Radar);
        mission.set_missiles(vec![]);
        mission.step(&[], &flat, TimeOfDay(1)).unwrap();
        let actor = mission.actor(1).unwrap();
        assert!(actor.trace.fire.selected);
        assert!(matches!(
            actor.controller.trace().motion.branch,
            thought::MotionBranch::IncomingFire { .. }
        ));
    }

    #[test]
    fn an_uncertain_pass_keeps_its_queued_pair_and_compatibility_releases() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        mission.push(perception_actor(Experience::Ace));
        let mut missile = incoming_snapshot(Guidance::Supported, [0.0, 20000.0, -1000.0]);
        mission.set_missiles(vec![missile]);
        let first = mission.step(&[], &flat, TimeOfDay(0)).unwrap();
        assert_eq!(first.devices.len(), 1);
        assert_eq!(first.devices[0].class, SeekerClass::Radar);
        assert!(!mission.actor(1).unwrap().device_schedule.is_empty());
        missile.velocity = [0.0, 0.0, -2000.0];
        mission.set_missiles(vec![missile]);
        mission
            .actor_mut(1)
            .unwrap()
            .device_schedule
            .push((30, SeekerClass::Infrared, 1, false));
        let mut later = Vec::new();
        for tick in 1..=31 {
            later.extend(mission.step(&[], &flat, TimeOfDay(tick)).unwrap().devices);
        }
        assert_eq!(later.len(), 2);
        assert!(later.iter().any(|d| d.class == SeekerClass::Radar));
        assert!(later.iter().any(|d| d.class == SeekerClass::Infrared));
        assert!(mission.actor(1).unwrap().device_schedule.is_empty());
    }

    #[test]
    fn novice_reaction_delays_defense_then_debits_two_chaff_on_schedule() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        let mut actor = perception_actor(Experience::Novice);
        actor
            .order(
                super::super::wing::WingRequest::TargetAssignment(
                    super::super::wing::TargetOrder::HoldFire,
                ),
                0,
            )
            .unwrap();
        let initial = actor.dispensers()[1].count;
        mission.push(actor);
        mission.set_missiles(vec![incoming_snapshot(
            Guidance::Supported,
            [0., 20000., -1000.],
        )]);
        let mut releases = Vec::new();
        for tick in 0..270 {
            let output = mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
            assert!(output.launches.is_empty());
            if tick == 0 {
                let actor = mission.actor(1).unwrap();
                assert!(actor.defense_decision().unwrap().motion.is_none());
                assert!(actor.missile_threats().any(|r| r.targeting_receiver));
            }
            for device in output.devices {
                releases.push((tick, device.class, device.released));
            }
        }
        assert_eq!(
            releases,
            [(180, SeekerClass::Radar, 1), (210, SeekerClass::Radar, 1)]
        );
        assert_eq!(mission.actor(1).unwrap().dispensers()[1].count, initial - 2);
    }

    #[test]
    fn silent_and_unseen_missiles_cannot_trigger_ai_defense() {
        use crate::combat::missiles::Guidance;
        for guidance in [Guidance::Active, Guidance::Infrared, Guidance::Emitter] {
            let mut mission = AiMission::new();
            mission.push(perception_actor(Experience::Novice));
            mission.set_missiles(vec![incoming_snapshot(guidance, [0., 20000., -1000.])]);
            for tick in 0..60 {
                let output = mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
                let actor = mission.actor(1).unwrap();
                assert!(actor.defense_decision().is_none(), "{guidance:?}");
                assert!(actor.missile_threats().next().is_none());
                assert!(output.devices.is_empty());
            }
        }
    }

    #[test]
    fn pitbull_acquisition_and_receiver_failure_gate_the_same_ai_service() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        mission.push(perception_actor(Experience::Ace));
        let mut missile = incoming_snapshot(Guidance::Active, [0., 20000., -1000.]);
        missile.radar_active = true;
        mission.set_missiles(vec![missile]);
        mission.step(&[], &flat, TimeOfDay(0)).unwrap();
        assert!(mission.actor(1).unwrap().defense_decision().is_none());
        missile.radar_acquired = true;
        mission.set_missiles(vec![missile]);
        mission.step(&[], &flat, TimeOfDay(1)).unwrap();
        assert_eq!(mission.actor(1).unwrap().activity(), Activity::Defending);
        let mut failed = AiMission::new();
        let mut actor = perception_actor(Experience::Ace);
        actor.flight_mut().systems.counts[32] = 2;
        failed.push(actor);
        failed.set_missiles(vec![missile]);
        failed.step(&[], &flat, TimeOfDay(0)).unwrap();
        assert!(failed.actor(1).unwrap().defense_decision().is_none());
    }

    #[test]
    fn a_visually_observed_ir_threat_requests_only_flares_without_hidden_classification() {
        use crate::combat::missiles::Guidance;
        let mut mission = AiMission::new();
        mission.push(perception_actor(Experience::Ace));
        let mut missile = incoming_snapshot(Guidance::Infrared, [0., 20000., 5000.]);
        missile.velocity = [0., 0., -3000.];
        mission.set_missiles(vec![missile]);
        let first = mission.step(&[], &flat, TimeOfDay(0)).unwrap();
        assert!(first.devices.is_empty());
        missile.position[2] -= 25.;
        mission.set_missiles(vec![missile]);
        let output = mission.step(&[], &flat, TimeOfDay(1)).unwrap();
        let actor = mission.actor(1).unwrap();
        assert_eq!(actor.activity(), Activity::Defending);
        assert_eq!(
            actor.defense_decision().unwrap().burst,
            Some(defense::BurstRequest::INFRARED)
        );
        assert!(actor.missile_threats().all(|r| r.guidance_class.is_none()));
        assert_eq!(output.devices.len(), 1);
    }

    #[test]
    fn a_distant_ace_drags_without_devices_while_a_novice_reacts_late() {
        use crate::combat::missiles::Guidance;
        for skill in [Experience::Novice, Experience::Ace] {
            let mut mission = AiMission::new();
            let mut actor = perception_actor(skill);
            actor.dispensers.clear();
            mission.push(actor);
            mission.set_missiles(vec![incoming_snapshot(
                Guidance::Supported,
                [0., 20000., -60000.],
            )]);
            let output = mission.step(&[], &flat, TimeOfDay(0)).unwrap();
            let actor = mission.actor(1).unwrap();
            let decision = actor.defense_decision().unwrap();
            assert_eq!(decision.motion.is_some(), skill == Experience::Ace);
            assert!(decision.burst.is_none());
            assert!(output.devices.is_empty());
            assert!(decision.debug.estimated_threat_time_s.unwrap() > 30.);
        }
    }

    #[test]
    fn dummy_holds_400_knots_without_decisions_and_stops_on_death() {
        let mut mission = one_v_one();
        for actor in mission.actors_mut() {
            actor.set_dummy();
        }
        let initial: Vec<_> = mission
            .actors()
            .iter()
            .map(|a| {
                (
                    a.flight.position,
                    a.flight.velocity,
                    a.flight.fuel,
                    a.rounds_remaining(),
                )
            })
            .collect();
        assert_eq!(
            mission
                .actor_mut(1)
                .unwrap()
                .order(
                    super::super::wing::WingRequest::Break {
                        heading_offset_deg: 90,
                        pitch_deg: 0
                    },
                    0
                )
                .unwrap(),
            super::super::wing::ReceiverOutcome::Rejected(super::super::wing::RejectReason::Dummy)
        );
        for tick in 0..1200 {
            mission.actor_mut(1).unwrap().report_hit();
            let world = world_of(&mission);
            let output = mission.step(&world, &|_, _| 0., TimeOfDay(tick)).unwrap();
            assert!(
                output.launches.is_empty() && output.devices.is_empty() && output.wing.is_empty()
            );
        }
        for (actor, (position, velocity, fuel, rounds)) in mission.actors().iter().zip(initial) {
            for i in 0..3 {
                assert!((actor.flight.position[i] - position[i] - velocity[i] * 10.).abs() < 1e-6);
            }
            assert_eq!(actor.flight.velocity, velocity);
            assert_eq!(actor.flight.speed, super::super::launch::DUMMY_SPEED_FPS);
            assert_eq!(actor.flight.fuel, fuel);
            assert_eq!(actor.rounds_remaining(), rounds);
            assert_eq!(actor.controller.target(), None);
            assert_eq!(actor.flight.ticks, 1200);
            assert_eq!(actor.flight.pitch, 0.);
            assert_eq!(actor.flight.bank, 0.);
        }
        let position = mission.actor(1).unwrap().flight.position;
        mission.actor_mut(1).unwrap().set_alive(false);
        mission
            .step(&world_of(&mission), &|_, _| 0., TimeOfDay(1200))
            .unwrap();
        assert_eq!(mission.actor(1).unwrap().flight.position, position);
        assert_eq!(mission.actor(1).unwrap().activity(), Activity::Destroyed);
    }

    /// Two aircraft, opposite sides, converging head on.
    fn one_v_one() -> AiMission {
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(
            AiActor::new(setup(
                2,
                2,
                0,
                [0.0, 20000.0, 40000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        mission
    }

    fn world_of(mission: &AiMission) -> Vec<WorldObject> {
        mission
            .actors()
            .iter()
            .map(|a| object(a, a.identity().side.0))
            .collect()
    }

    fn run(mission: &mut AiMission, ticks: u64) -> MissionOutput {
        let mut combined = MissionOutput::default();
        for _ in 0..ticks {
            let world = world_of(mission);
            let out = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
            combined.launches.extend(out.launches);
            combined.devices.extend(out.devices);
            combined.fallbacks.extend(out.fallbacks);
            combined.activities.extend(out.activities);
        }
        combined
    }

    #[test]
    fn the_ai_actually_launches_missiles_at_an_enemy() {
        let mut mission = one_v_one();
        let missiles_before: u32 = mission
            .actors()
            .iter()
            .map(|a| match a.stations()[0].store.rounds {
                Rounds::Finite(n) => n,
                Rounds::Unlimited => 0,
            })
            .sum();
        let out = run(&mut mission, 7200);
        assert!(
            !out.launches.is_empty(),
            "sixty seconds of a head-on 1v1 produced no launch at all"
        );
        let missiles_after: u32 = mission
            .actors()
            .iter()
            .map(|a| match a.stations()[0].store.rounds {
                Rounds::Finite(n) => n,
                Rounds::Unlimited => 0,
            })
            .sum();
        assert!(
            missiles_after < missiles_before,
            "the guided store was never chosen: {missiles_before} to {missiles_after}"
        );
        assert!(
            out.activities
                .iter()
                .any(|(_, a)| *a == Activity::Attacking)
        );
    }

    #[test]
    fn a_warned_ace_releases_countermeasures_and_defends() {
        let mut mission = one_v_one();
        run(&mut mission, 120);
        // Repeated warnings from a third aircraft, so B47's "launcher is the
        // current target" and same-side exemptions cannot apply.
        for tick in 0..40 {
            mission.actor_mut(1).unwrap().report_threat(ThreatReport {
                missile_id: 500 + tick,
                seeker: SeekerClass::Infrared,
                launcher_id: 999,
                launcher_same_side: false,
                distance_at_launch_ft: 3000.0,
                launch_tick: 0,
            });
            run(&mut mission, 120);
        }
        let dispensers = mission.actor(1).unwrap().dispensers();
        let infrared = dispensers
            .iter()
            .find(|d| d.class == SeekerClass::Infrared)
            .unwrap();
        assert!(
            infrared.count < 30,
            "an Ace under repeated infrared launches never released a flare"
        );
        // The matching class only: radar decoys are never substituted.
        let radar = dispensers
            .iter()
            .find(|d| d.class == SeekerClass::Radar)
            .unwrap();
        assert_eq!(
            radar.count, 30,
            "a radar decoy was released for an infrared launch"
        );
    }

    #[test]
    fn a_mission_steps_every_actor_and_advances_its_tick() {
        let mut mission = one_v_one();
        assert_eq!(mission.tick(), 0);
        run(&mut mission, 120);
        assert_eq!(mission.tick(), 120);
        assert_eq!(mission.len(), 2);
    }

    #[test]
    fn each_actor_flies_its_own_model_and_actually_moves() {
        let mut mission = one_v_one();
        let before: Vec<[f64; 3]> = mission
            .actors()
            .iter()
            .map(|a| a.flight().position)
            .collect();
        run(&mut mission, 240);
        for (index, actor) in mission.actors().iter().enumerate() {
            let moved = distance(before[index], actor.flight().position);
            assert!(moved > 1000.0, "actor {index} moved only {moved} ft");
        }
    }

    #[test]
    fn two_actors_hold_independent_state() {
        let mut mission = one_v_one();
        run(&mut mission, 240);
        let a = mission.actor(1).unwrap();
        let b = mission.actor(2).unwrap();
        assert_ne!(a.flight().position, b.flight().position);
        assert_ne!(a.flight().yaw, b.flight().yaw);
        // Neither actor's stores are the other's.
        assert_eq!(a.stations().len(), b.stations().len());
        assert!(!std::ptr::eq(a.stations(), b.stations()));
    }

    #[test]
    fn repeated_runs_with_the_same_seed_are_identical() {
        let mut first = one_v_one();
        let mut second = one_v_one();
        let a = run(&mut first, 600);
        let b = run(&mut second, 600);
        assert_eq!(a.launches, b.launches);
        assert_eq!(a.devices, b.devices);
        assert_eq!(a.activities, b.activities);
        for (x, y) in first.actors().iter().zip(second.actors()) {
            assert_eq!(x.flight().position, y.flight().position);
            assert_eq!(x.flight().yaw, y.flight().yaw);
            assert_eq!(x.rounds_remaining(), y.rounds_remaining());
        }
    }

    #[test]
    fn an_actor_never_targets_itself() {
        let mut mission = one_v_one();
        run(&mut mission, 240);
        for actor in mission.actors() {
            assert_ne!(actor.controller().target(), Some(actor.id()));
        }
    }

    #[test]
    fn an_actor_never_targets_its_own_side() {
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(AiActor::new(setup(2, 1, 1, [0.0, 20000.0, 10000.0], 0.0)).unwrap());
        run(&mut mission, 600);
        for actor in mission.actors() {
            assert_eq!(actor.controller().target(), None, "engaged a friendly");
        }
    }

    #[test]
    fn ammunition_is_debited_and_never_free() {
        let mut mission = one_v_one();
        let start: u32 = mission.actors().iter().map(|a| a.rounds_remaining()).sum();
        let out = run(&mut mission, 3600);
        let end: u32 = mission.actors().iter().map(|a| a.rounds_remaining()).sum();
        if out.launches.is_empty() {
            // Nothing fired, so nothing may have been spent either.
            assert_eq!(start, end);
        } else {
            assert!(
                end < start,
                "rounds were not debited by {} launches",
                out.launches.len()
            );
        }
    }

    #[test]
    fn an_empty_store_stops_producing_launches() {
        let mut mission = AiMission::new();
        let mut a = setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0);
        // One missile only, and no gun.
        a.stations = simple_stations(1, 0, ScalarSpeed(2000.0));
        mission.push(AiActor::new(a).unwrap());
        mission.push(
            AiActor::new(setup(
                2,
                2,
                0,
                [0.0, 20000.0, 20000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        let out = run(&mut mission, 7200);
        let fired = out.launches.iter().filter(|l| l.actor == 1).count();
        assert!(fired <= 1, "one missile produced {fired} launches");
        assert_eq!(
            mission.actor(1).unwrap().rounds_remaining(),
            1 - fired as u32
        );
    }

    #[test]
    fn an_inhibited_store_never_launches() {
        let mut mission = AiMission::new();
        let mut a = setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0);
        for station in &mut a.stations {
            station.store.inhibited = true;
        }
        mission.push(AiActor::new(a).unwrap());
        mission.push(
            AiActor::new(setup(
                2,
                2,
                0,
                [0.0, 20000.0, 20000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        let out = run(&mut mission, 3600);
        assert!(out.launches.iter().all(|l| l.actor != 1));
    }

    #[test]
    fn a_launch_event_carries_a_unique_request_identity() {
        let mut mission = one_v_one();
        let out = run(&mut mission, 7200);
        let mut seen = Vec::new();
        for launch in &out.launches {
            let key = (launch.actor, launch.request_id);
            assert!(!seen.contains(&key), "duplicate launch {key:?}");
            seen.push(key);
        }
    }

    #[test]
    fn a_threat_report_reaches_only_the_aircraft_it_names() {
        let mut mission = one_v_one();
        run(&mut mission, 120);
        mission.actor_mut(1).unwrap().report_threat(ThreatReport {
            missile_id: 500,
            seeker: SeekerClass::Infrared,
            launcher_id: 999,
            launcher_same_side: false,
            distance_at_launch_ft: 4000.0,
            launch_tick: 0,
        });
        let out = run(&mut mission, 1200);
        // Only actor 1 can have released devices from that warning.
        assert!(out.devices.iter().all(|d| d.actor == 1));
    }

    #[test]
    fn countermeasures_come_out_of_the_actors_own_dispensers() {
        let mut mission = one_v_one();
        let before = mission.actor(1).unwrap().dispensers()[0].count;
        mission.actor_mut(1).unwrap().report_threat(ThreatReport {
            missile_id: 500,
            seeker: SeekerClass::Infrared,
            launcher_id: 999,
            launcher_same_side: false,
            distance_at_launch_ft: 1000.0,
            launch_tick: 0,
        });
        let out = run(&mut mission, 2400);
        if out.devices.iter().any(|d| d.actor == 1) {
            let after = mission.actor(1).unwrap().dispensers()[0].count;
            assert!(after < before, "devices were released without a debit");
        }
    }

    #[test]
    fn a_destroyed_actor_stops_flying_and_reports_destroyed() {
        let mut mission = one_v_one();
        run(&mut mission, 60);
        mission.actor_mut(1).unwrap().set_alive(false);
        let frozen = mission.actor(1).unwrap().flight().position;
        let out = run(&mut mission, 120);
        assert_eq!(mission.actor(1).unwrap().flight().position, frozen);
        assert!(out.activities.contains(&(1, Activity::Destroyed)));
    }

    #[test]
    fn every_ported_aircraft_runs_a_one_v_one_at_every_level() {
        for aircraft in AircraftId::ALL {
            for level in Experience::ALL {
                let mut mission = AiMission::new();
                for (index, (id, side, yaw, z)) in [
                    (1u32, 1u32, 0.0, 0.0),
                    (2u32, 2u32, std::f64::consts::PI, 30000.0),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut s = setup(id, side, 0, [0.0, 20000.0, z], yaw);
                    s.identity.aircraft = aircraft;
                    let profile = synthetic_profile(aircraft);
                    s.flight = flight::State::new(&profile, s.flight.position).unwrap();
                    s.flight.yaw = yaw;
                    s.flight.velocity = crate::attitude::Basis::new(yaw, 0.0, 0.0)
                        .forward
                        .map(|v| v * s.flight.speed);
                    s.stations = simple_stations(
                        2 + index as u32,
                        200 + 10 * aircraft_index(aircraft) as u32,
                        ScalarSpeed(1500.0 + aircraft_index(aircraft) as f64 * 50.0),
                    );
                    s.experience = resolved(level);
                    s.seed = 101 + index as u64;
                    mission.push(AiActor::new(s).unwrap());
                }
                run(&mut mission, 600);
                for actor in mission.actors() {
                    assert!(
                        actor.flight().position.iter().all(|v| v.is_finite()),
                        "{aircraft:?} {level:?} produced a non-finite position"
                    );
                }
            }
        }
    }

    #[test]
    fn a_two_v_two_runs_with_leaders_and_wingmen() {
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(AiActor::new(setup(2, 1, 1, [2000.0, 20000.0, -2000.0], 0.0)).unwrap());
        mission.push(
            AiActor::new(setup(
                3,
                2,
                0,
                [0.0, 20000.0, 40000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        mission.push(
            AiActor::new(setup(
                4,
                2,
                1,
                [2000.0, 20000.0, 42000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        run(&mut mission, 1200);
        assert_eq!(mission.len(), 4);
        for actor in mission.actors() {
            assert!(actor.flight().position.iter().all(|v| v.is_finite()));
        }
        // The wingmen are not leaders and the leaders are.
        assert!(mission.actor(1).unwrap().identity().is_leader());
        assert!(!mission.actor(2).unwrap().identity().is_leader());
    }

    #[test]
    fn each_wing_follows_only_its_own_leader_including_the_human() {
        let mut mission = AiMission::new();
        mission.set_external_leader(Side(1), 0, 0);
        for (id, side, wing, member) in [
            (1, 1, 0, 1),
            (2, 1, 1, 0),
            (3, 1, 1, 1),
            (4, 2, 1, 0),
            (5, 2, 1, 1),
        ] {
            let mut actor = setup(id, side, member, [id as f64 * 1000., 20000., 0.], 0.);
            actor.identity.wing = wing;
            mission.push(AiActor::new(actor).unwrap());
        }
        let mut world = world_of(&mission);
        let mut player = world[0].clone();
        player.id = 0;
        player.position = [90000., 20000., 0.];
        player.human_controlled = true;
        world.push(player);
        mission.refresh_leaders(&world, &mut MissionOutput::default());
        let table = super::super::link::Engagements::new(&mission.actors);
        assert_eq!(
            mission.leader_view(0, &world, &table).unwrap().position[0],
            90000.
        );
        assert!(mission.leader_view(1, &world, &table).is_none());
        assert_eq!(
            mission.leader_view(2, &world, &table).unwrap().position[0],
            2000.
        );
        assert_eq!(
            mission.leader_view(4, &world, &table).unwrap().position[0],
            4000.
        );
        world.last_mut().unwrap().alive = false;
        assert!(mission.leader_view(0, &world, &table).is_none());
    }

    #[test]
    fn formation_recovers_from_ahead_or_behind_and_holds_through_turns() {
        for (turn, initial_offset) in [0.0_f64, 1.5, -1.5]
            .into_iter()
            .flat_map(|turn| [-800., 0., 800.].map(|offset| (turn, offset)))
        {
            let mut mission = AiMission::new();
            mission.set_spacing(512, 0);
            mission.set_external_leader(Side(1), 0, 0);
            let mut start = setup(1, 1, 1, [512., 20000., -512. + initial_offset], 0.);
            // Isolate formation from the synthetic model's bingo return.
            start.home_airport = None;
            start.flight.speed = 800.;
            start.flight.velocity = [0., 0., 800.];
            mission.push(AiActor::new(start).unwrap());
            let mut leader = object(mission.actor(1).unwrap(), 1);
            leader.id = 0;
            leader.human_controlled = true;
            leader.position = [0., 20000., 0.];
            let mut last_error = 0.;
            let mut maximum_altitude_error = 0.0_f64;
            for tick in 0..14400 {
                let heading = (turn * (tick as f64 / 120. - 30.).max(0.)).to_radians();
                leader.heading_deg = heading.to_degrees();
                leader.velocity = [800. * heading.sin(), 0., 800. * heading.cos()];
                let mut world = world_of(&mission);
                world.push(leader.clone());
                mission.step(&world, &flat, TimeOfDay(0)).unwrap();
                for i in 0..3 {
                    leader.position[i] += leader.velocity[i] / 120.;
                }
                let slot = [
                    leader.position[0] + 512. * heading.cos() - 512. * heading.sin(),
                    20000.,
                    leader.position[2] - 512. * heading.sin() - 512. * heading.cos(),
                ];
                last_error = distance(mission.actor(1).unwrap().flight().position, slot);
                if tick > 3600 && turn == 0. {
                    maximum_altitude_error = maximum_altitude_error.max(
                        (mission.actor(1).unwrap().flight().position[1] - leader.position[1]).abs(),
                    );
                }
                if tick > 7200 {
                    assert!(
                        last_error < 150.,
                        "turn {turn}, start {initial_offset}, tick {tick}: slot error {last_error}"
                    );
                }
            }
            assert!(last_error < 150.);
            if turn == 0. {
                eprintln!(
                    "straight formation offset {initial_offset}: maximum altitude error {maximum_altitude_error:.2} ft"
                );
                assert!(maximum_altitude_error < 10.);
            }
        }
    }

    #[test]
    fn formation_decisions_do_not_depend_on_actor_iteration_order() {
        fn scene(reverse: bool) -> AiMission {
            let mut m = AiMission::new();
            m.set_external_leader(Side(1), 0, 0);
            m.set_spacing(512, 0);
            for member in 1..=4 {
                let offset =
                    super::super::wing::formation_slot_point(Formation::Echelon, member, 512, 0)
                        .unwrap();
                let mut start = setup(
                    u32::from(member),
                    1,
                    member,
                    [offset[0], 20000., offset[2]],
                    0.,
                );
                start.home_airport = None;
                start.flight.speed = 800.;
                start.flight.velocity = [0., 0., 800.];
                m.push(AiActor::new(start).unwrap());
            }
            if reverse {
                m.actors.reverse();
            }
            m
        }
        for initial_heading in [0., 180.] {
            let mut a = scene(false);
            let mut b = scene(true);
            let mut leader = object(a.actor(1).unwrap(), 1);
            leader.id = 0;
            leader.position = [0., 20000., 0.];
            leader.heading_deg = initial_heading;
            leader.velocity = super::super::formation::velocity(initial_heading, 0., 800.);
            for tick in 0..1800 {
                for mission in [&mut a, &mut b] {
                    if initial_heading == 0. && matches!(tick, 300 | 900) {
                        mission
                            .order_wing(
                                Side(1),
                                0,
                                None,
                                super::super::wing::WingRequest::FormationSelection(
                                    if tick == 300 {
                                        Formation::LineAbreast
                                    } else {
                                        Formation::LineAstern
                                    },
                                ),
                            )
                            .unwrap();
                    }
                    let mut world = world_of(mission);
                    world.push(leader.clone());
                    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
                }
                for id in 1..=4 {
                    assert_eq!(a.actor(id).unwrap().flight(), b.actor(id).unwrap().flight());
                    assert_eq!(
                        a.actor(id).unwrap().controller().formation_trace(),
                        b.actor(id).unwrap().controller().formation_trace()
                    );
                }
                leader.position[2] += leader.velocity[2] / 120.;
            }
        }
    }

    #[test]
    fn formation_diving_reversal_uses_separate_physical_approaches() {
        use super::super::formation::Phase;
        for (turn, climb, repeat) in [
            (-1.0, -1.0, false),
            (1.0, -1.0, false),
            (1.0, 1.0, false),
            (-1.0, -1.0, true),
        ] {
            let mut mission = AiMission::new();
            mission.set_spacing(512, 0);
            mission.set_external_leader(Side(1), 0, 0);
            for member in 1..=4 {
                let offset =
                    super::super::wing::formation_slot_point(Formation::Echelon, member, 512, 0)
                        .unwrap();
                let mut start = setup(
                    u32::from(member),
                    1,
                    member,
                    [offset[0], 20000., offset[2]],
                    0.,
                );
                start.home_airport = None;
                start.flight.speed = 800.;
                start.flight.velocity = [0., 0., 800.];
                mission.push(AiActor::new(start).unwrap());
            }
            let mut leader = object(mission.actor(1).unwrap(), 1);
            leader.id = 0;
            leader.position = [0., 20000., 0.];
            leader.human_controlled = true;
            let mut minimum = f64::INFINITY;
            let mut phases = [[false; 7]; 4];
            let mut last = [Phase::Close; 4];
            for tick in 0..36000 {
                let t = tick as f64 / 120.;
                let progress = ((t - 10.) / 20.).clamp(0., 1.);
                let reversal = if repeat {
                    ((t - 70.) / 20.).clamp(0., 1.)
                } else {
                    0.
                };
                leader.heading_deg = turn * 180. * (progress - reversal);
                leader.pitch_deg = climb
                    * 25.
                    * ((progress * std::f64::consts::PI).sin()
                        - (reversal * std::f64::consts::PI).sin());
                leader.velocity =
                    super::super::formation::velocity(leader.heading_deg, leader.pitch_deg, 800.);
                let mut world = world_of(&mission);
                world.push(leader.clone());
                let before: Vec<_> = mission
                    .actors()
                    .iter()
                    .map(|a| a.flight().clone())
                    .collect();
                mission.step(&world, &flat, TimeOfDay(0)).unwrap();
                for i in 0..3 {
                    leader.position[i] += leader.velocity[i] / 120.;
                }
                let mut achieved = world_of(&mission);
                achieved.push(leader.clone());
                for (i, actor) in mission.actors().iter().enumerate() {
                    let mut replay = before[i].clone();
                    replay.step_surface(actor.last_input(), |_, _| Surface::terrain(0.));
                    assert_eq!(replay, *actor.flight());
                    let trace = actor.controller().formation_trace().unwrap();
                    phases[i][trace.phase as usize] = true;
                    if trace.phase != last[i] {
                        eprintln!(
                            "turn {turn} t {t:.1} actor {} {:?} distance {:.0} closure {:.0} yielding {:?}",
                            actor.id(),
                            trace.phase,
                            trace.slot_distance_ft,
                            trace.closure_fps,
                            trace.yielding_to
                        );
                        last[i] = trace.phase;
                    }
                    assert!(!actor.flight().crashed);
                    for other in achieved.iter().filter(|o| o.id != actor.id()) {
                        minimum = minimum.min(distance(actor.flight().position, other.position));
                    }
                }
            }
            eprintln!(
                "turn {turn}, climb {climb}, repeat {repeat}: minimum separation {minimum:.1}"
            );
            for (i, actor) in mission.actors().iter().enumerate() {
                let trace = actor.controller().formation_trace().unwrap();
                eprintln!(
                    "actor {} final {:?} slot distance {:.0}",
                    actor.id(),
                    trace.phase,
                    trace.slot_distance_ft
                );
                assert!(phases[i][Phase::Intercept as usize]);
                assert!(phases[i][Phase::Capture as usize]);
                assert_eq!(trace.phase, Phase::Close);
                assert!(trace.slot_distance_ft < 250.);
            }
            assert!(minimum > 250., "unsafe separation {minimum}");
        }
    }

    #[test]
    fn an_idle_wingman_flies_toward_its_own_delta_slot() {
        let mut mission = AiMission::new();
        mission.set_spacing(512, 0);
        mission.push(AiActor::new(setup(1, 1, 0, [0., 20000., 0.], 0.)).unwrap());
        mission.push(AiActor::new(setup(2, 1, 1, [-3000., 20000., -512.], 0.)).unwrap());
        let initial_error = 3512.;
        run(&mut mission, 1200);
        let leader = mission.actor(1).unwrap().flight();
        let wingman = mission.actor(2).unwrap().flight();
        let slot = [
            leader.position[0] + 512.,
            leader.position[1],
            leader.position[2] - 512.,
        ];
        assert!(
            distance(wingman.position, slot) < initial_error,
            "wingman did not close on its slot: {:?} vs {:?}",
            wingman.position,
            slot
        );
        assert_eq!(mission.actor(2).unwrap().activity(), Activity::Formation);
    }

    #[test]
    fn a_wingman_obeys_break_formation_and_engage_orders() {
        use crate::ai::wing::{
            AppliedSetting, Formation as F, PlayerBreak, ReceiverOutcome, TargetId, TargetOrder,
            WingRequest,
        };
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(AiActor::new(setup(2, 1, 1, [2000.0, 20000.0, -2000.0], 0.0)).unwrap());
        mission.push(
            AiActor::new(setup(
                3,
                2,
                0,
                [0.0, 20000.0, 30000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        run(&mut mission, 240);

        // Break: motion is installed and the wingman's heading actually moves.
        let heading_before = mission.actor(2).unwrap().flight().yaw;
        let outcome = mission
            .order(2, PlayerBreak::Right.request())
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, ReceiverOutcome::MotionInstalled(_)));
        // Allow physical roll reversal and lift response to develop.
        run(&mut mission, 720);
        let heading_after = mission.actor(2).unwrap().flight().yaw;
        assert!(
            (heading_after - heading_before).abs() > 0.1,
            "the wingman did not turn on a break order"
        );

        // Formation: a setting, not a relocation.
        let outcome = mission
            .order(2, WingRequest::FormationSelection(F::LineAstern))
            .unwrap()
            .unwrap();
        assert!(matches!(
            outcome,
            ReceiverOutcome::Applied(AppliedSetting::FormationSelection { .. })
        ));

        // Engage: the named target is taken.
        let outcome = mission
            .order(
                2,
                WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(3))),
            )
            .unwrap()
            .unwrap();
        assert!(matches!(
            outcome,
            ReceiverOutcome::Applied(AppliedSetting::TargetOrder { .. })
        ));
        assert_eq!(mission.actor(2).unwrap().controller().target(), Some(3));
    }

    #[test]
    fn an_actor_on_bingo_fuel_turns_for_home() {
        let mut mission = one_v_one();
        // Put actor 1 a long way from home and nearly dry, so endurance falls
        // under time-to-home plus five minutes (B48 bingo).
        {
            let actor = mission.actor_mut(1).unwrap();
            actor.flight_mut().position = [0.0, 20000.0, 400_000.0];
            actor.set_internal_fuel(120.0);
        }
        let out = run(&mut mission, 240);
        assert!(
            out.activities
                .iter()
                .any(|(id, a)| *id == 1 && *a == Activity::ReturningToBase),
            "a bingo actor never reported returning to base"
        );
        // The leader and singleton route home is fitted; it must be recorded.
        assert!(
            out.fallbacks
                .iter()
                .any(|(id, f)| *id == 1 && *f == Fallback::LeaderReturnToBase)
        );
    }

    #[test]
    fn external_tank_fuel_counts_toward_the_actors_endurance() {
        // The bingo actor above, with a full tank: its fuel burns first, so
        // the actor has it to fly on and its endurance counts it.
        let mut mission = one_v_one();
        let actor = mission.actor_mut(1).unwrap();
        actor.flight_mut().position = [0.0, 20000.0, 400_000.0];
        actor.set_internal_fuel(120.0);
        let dry = actor.endurance_s();
        actor.flight_mut().systems = crate::aircraft_systems::Systems::new(1, [10_000.0; 9]);
        let wet = actor.endurance_s();
        assert!(dry > 0.0);
        assert!(
            (wet / dry - (120.0 + 90_000.0) / 120.0).abs() < 1e-9,
            "{dry} {wet}"
        );
        let out = run(&mut mission, 240);
        assert!(
            !out.activities
                .iter()
                .any(|(id, a)| *id == 1 && *a == Activity::ReturningToBase),
            "an actor with full tanks went home on bingo"
        );
    }

    #[test]
    fn an_actor_out_of_fuel_reports_it() {
        let mut mission = one_v_one();
        mission.actor_mut(1).unwrap().set_internal_fuel(0.0);
        let out = run(&mut mission, 120);
        assert!(
            out.activities
                .iter()
                .any(|(id, a)| *id == 1 && *a == Activity::OutOfFuel)
        );
    }

    #[test]
    fn stores_point_at_the_retained_target_not_the_first_world_object() {
        // Two enemies: a far one listed first and a near one listed second.
        // The station view must describe the actor's own retained target.
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(
            AiActor::new(setup(
                2,
                2,
                0,
                [0.0, 20000.0, 90000.0],
                std::f64::consts::PI,
            ))
            .unwrap(),
        );
        mission.push(
            AiActor::new(setup(3, 2, 0, [0.0, 20000.0, 9000.0], std::f64::consts::PI)).unwrap(),
        );
        run(&mut mission, 240);
        let actor = mission.actor(1).unwrap();
        let retained = actor.controller().target().expect("no target retained");
        let world = world_of(&mission);
        let own = actor.own_state(&flat);
        let targets: Vec<_> = world
            .iter()
            .filter(|o| o.id != actor.id())
            .map(|o| actor.observed_target(o, o.position))
            .collect();
        let views = actor.station_views(&targets, &own);
        let expected = targets.iter().find(|t| t.id == retained).unwrap();
        let expected_error = pointing_error_deg(&own, expected.position);
        assert!(
            (views[0].pointing_error_deg - expected_error).abs() < 1e-9,
            "stores aimed at the wrong aircraft"
        );
    }

    #[test]
    fn fitted_fallbacks_are_reported_per_actor() {
        let mut mission = one_v_one();
        let out = run(&mut mission, 600);
        assert!(!out.fallbacks.is_empty());
        for (actor, _) in &out.fallbacks {
            assert!(mission.actor(*actor).is_some());
        }
        // The engagement pitch is unavoidable once an actor has a target.
        assert!(
            out.fallbacks
                .iter()
                .any(|(_, f)| *f == Fallback::EngagementPitch)
        );
    }

    #[test]
    fn speed_limits_come_from_the_loaded_envelopes() {
        let mission = one_v_one();
        let actor = mission.actor(1).unwrap();
        let limits = actor.speed_limits();
        assert!(limits.minimum.0 > 0.0);
        assert!(limits.maximum.0 > limits.minimum.0);
        assert!(limits.corner.0 <= limits.maximum.0);
        assert!(limits.corner.0 >= limits.minimum.0);
    }

    #[test]
    fn the_experience_g_adjustment_reaches_the_actor() {
        let novice = {
            let mut s = setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0);
            s.experience = resolved(Experience::Novice);
            AiActor::new(s).unwrap()
        };
        let ace = {
            let mut s = setup(2, 1, 0, [0.0, 20000.0, 0.0], 0.0);
            s.experience = resolved(Experience::Ace);
            AiActor::new(s).unwrap()
        };
        let (novice_g, _) = novice.g_limits();
        let (ace_g, _) = ace.g_limits();
        assert!(
            novice_g < ace_g,
            "Novice {novice_g} should lose a G against Ace {ace_g}"
        );
        assert!(novice_g >= 2.0, "the floor is 2 G, got {novice_g}");
    }

    #[test]
    fn simple_stations_and_dispensers_are_finite() {
        let stations = simple_stations(4, 500, ScalarSpeed(2000.0));
        assert_eq!(stations.len(), 2);
        assert!(
            stations
                .iter()
                .all(|s| !matches!(s.store.rounds, Rounds::Unlimited))
        );
        assert_eq!(simple_dispensers(30).len(), 2);
        assert!(simple_stations(0, 0, ScalarSpeed(2000.0)).is_empty());
    }

    #[test]
    fn every_ported_aircraft_is_recognised() {
        for aircraft in AircraftId::ALL {
            assert!(is_ported(aircraft));
        }
    }

    #[test]
    fn pointing_error_takes_the_larger_axis() {
        let own = OwnState {
            position: [0.0, 0.0, 0.0],
            heading_deg: 0.0,
            flight_path_pitch_deg: 0.0,
            body_pitch_offset_deg: 0.0,
            bank_deg: 0.0,
            speed: ScalarSpeed(800.0),
            limits: SpeedLimits {
                minimum: ScalarSpeed(220.0),
                maximum: ScalarSpeed(1600.0),
                corner: ScalarSpeed(700.0),
            },
            altitude_msl_ft: 0.0,
            agl_ft: 0.0,
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
            time_home_s: None,
            rotorcraft: false,
            internal_fuel_lbs: 8000.0,
            radar_emitting: true,
        };
        // Straight ahead is zero error.
        assert!(pointing_error_deg(&own, [0.0, 0.0, 1000.0]).abs() < 1e-9);
        // Ninety degrees right.
        assert!((pointing_error_deg(&own, [1000.0, 0.0, 0.0]) - 90.0).abs() < 1e-9);
        // Directly above is ninety in the pitch axis.
        assert!((pointing_error_deg(&own, [0.0, 1000.0, 0.0]) - 90.0).abs() < 1e-9);
    }
    #[test]
    fn scheduled_devices_are_single_quarter_second_releases_and_stop_when_empty() {
        let mut mission = AiMission::new();
        let mut actor = AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap();
        actor.dispensers[0].count = 2;
        actor
            .device_schedule
            .push((0, SeekerClass::Infrared, 3, false));
        mission.push(actor);
        let mut releases = Vec::new();
        for tick in 0..100 {
            let output = mission.step(&[], &flat, TimeOfDay(tick)).unwrap();
            for device in output.devices {
                releases.push((tick, device.released));
            }
        }
        assert_eq!(releases, [(0, 1), (30, 1)]);
        assert_eq!(mission.actor(1).unwrap().dispensers[0].count, 0);
        assert_eq!(mission.actor(1).unwrap().dispensers[1].count, 30);
    }

    #[test]
    fn releases_remove_only_the_mass_of_external_rounds_actually_debited() {
        let mut actor = AiActor::new(setup(1, 1, 0, [0., 20000., 0.], 0.)).unwrap();
        actor.stations = simple_stations(2, 10, ScalarSpeed(2000.));
        actor.stations[0].external_round_lbs = 250.;
        actor.flight.set_payload(600.).unwrap(); // 100 lb fixed equipment remains.
        let mut intent = super::super::controller::WeaponIntent {
            request: weapon_service::FireRequest {
                actor: ActorId(1),
                station: StationId(0),
                target: super::super::weapon_service::TargetId(2),
                request_id: super::super::weapon_service::RequestId(1),
            },
        };
        for expected in [350., 100.] {
            assert!(actor.release(&intent, &IntentBatch::default()).is_some());
            assert_eq!(actor.flight.payload_lbs, expected);
        }
        assert!(actor.release(&intent, &IntentBatch::default()).is_none());
        assert_eq!(actor.flight.payload_lbs, 100.);
        intent.request.station = StationId(1);
        assert!(actor.release(&intent, &IntentBatch::default()).is_some());
        assert_eq!(
            actor.flight.payload_lbs, 100.,
            "internal gun uses the shared zero external-mass convention"
        );
    }

    #[test]
    fn station_scoring_receives_error_and_failed_envelopes() {
        let actor = AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap();
        let own = actor.own_state(&flat);
        let mut target = object(&actor, 2);
        target.id = 2;
        target.position = [0.0, 20000.0, 1000.0];
        let views = [actor.observed_target(&target, target.position)];
        assert_eq!(
            actor.station_views(&views, &own)[0].employment_fit,
            Some(0.0)
        );
        target.position = [2000.0, 20000.0, 1000.0];
        let views = [actor.observed_target(&target, target.position)];
        assert_eq!(actor.station_views(&views, &own)[0].employment_fit, None);
    }

    #[test]
    fn wing_orders_do_not_cross_side_or_wing_boundaries() {
        use super::super::wing::{PlayerBreak, TargetOrder, WingRequest};
        let mut mission = AiMission::new();
        for (id, side, wing) in [(1, 1, 0), (2, 1, 1), (3, 2, 0)] {
            let mut s = setup(id, side, 1, [0.0, 20000.0, 0.0], 0.0);
            s.identity.wing = wing;
            mission.push(AiActor::new(s).unwrap());
        }
        run(&mut mission, 1);
        assert_eq!(
            mission
                .order_wing(
                    super::super::targeting::Side(1),
                    0,
                    None,
                    PlayerBreak::Right.request()
                )
                .unwrap(),
            1
        );
        mission
            .order_wing(
                super::super::targeting::Side(1),
                0,
                None,
                WingRequest::TargetAssignment(TargetOrder::HoldFire),
            )
            .unwrap();
        run(&mut mission, 2);
        assert_eq!(mission.actor(1).unwrap().controller.target(), None);
        assert!(mission.actor(2).unwrap().controller.target().is_some());
        assert!(mission.actor(3).unwrap().controller.target().is_some());
    }

    #[test]
    fn health_caution_never_holds_a_damaged_aircraft_below_its_flight_path_load() {
        // The ai-long-5v5 case: a heavy final, 24 percent airframe damage.
        let healthy = 1.16;
        assert_eq!(
            health_scaled_positive_g(healthy, 0.759),
            HEALTH_SCALED_G_FLOOR.min(healthy)
        );
        // Maneuvering authority is still reduced, never raised.
        assert!((health_scaled_positive_g(7.0, 0.5) - 3.5).abs() < 1e-12);
        assert_eq!(health_scaled_positive_g(7.0, 1.0), 7.0);
        assert_eq!(health_scaled_positive_g(7.0, 0.0), HEALTH_SCALED_G_FLOOR);
        // A loaded limit already below the floor is kept, not raised.
        assert_eq!(health_scaled_positive_g(0.9, 0.5), 0.9);
        assert_eq!(health_scaled_positive_g(1.1, 1.0), 1.1);
    }

    #[test]
    fn all_models_move_only_through_recorded_inputs_including_reversal_and_damage() {
        use super::super::{
            controller::Completion,
            motion::{Bank, Duration, MotionRequest, PitchRequest, SpeedRequest},
            steering::CommandMode,
        };
        for aircraft in AircraftId::ALL {
            for (health, hybrid) in [(1.0, false), (0.25, false), (1.0, true), (0.25, true)] {
                let mut setup = setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0);
                setup.identity.aircraft = aircraft;
                setup.flight =
                    flight::State::new(&synthetic_profile(aircraft), setup.flight.position)
                        .unwrap();
                if hybrid {
                    setup.flight.enable_research(1).unwrap();
                }
                setup.flight.damage_fraction = 1.0 - health;
                setup.flight.set_payload(500.0).unwrap();
                let mut actor = AiActor::new(setup).unwrap();
                for tick in 0..600 {
                    if tick == 450 {
                        actor.set_internal_fuel(0.0);
                    }
                    let heading = if tick < 300 { 90 } else { 270 };
                    let own = actor.own_state(&flat);
                    let before = actor.flight.clone();
                    let intent = MotionIntent {
                        formation_flight: false,
                        afterburner: tick < 300,
                        id: 1,
                        request: MotionRequest::new(
                            heading,
                            PitchRequest::Explicit(0),
                            Bank::Unconstrained,
                            SpeedRequest::Corner,
                            Duration::Timed(5),
                        ),
                        heading_deg: f64::from(heading),
                        flight_path_pitch_deg: 0.0,
                        speed: own.limits.corner,
                        bank: Bank::Unconstrained,
                        completion: Completion::Deadline(super::super::motion::Deadline(300)),
                        steering_point: None,
                        mode: CommandMode::OtherState,
                    };
                    actor
                        .fly(Some(&intent), &own, &flat, &|x, z| {
                            Surface::terrain(flat(x, z))
                        })
                        .unwrap();
                    let mut replay = before;
                    replay.step_surface(actor.last_input(), |x, z| Surface::terrain(flat(x, z)));
                    assert_eq!(
                        actor.flight, replay,
                        "{aircraft:?}: AI motion differs from input replay"
                    );
                    assert!(actor.flight.position.iter().all(|v| v.is_finite()));
                    if tick >= 450 {
                        assert!(!actor.flight.engine);
                    }
                }
            }
        }
    }

    #[test]
    fn wing_assignments_reach_live_ranking_without_counting_other_wings() {
        use super::super::wing::{TargetId, TargetOrder, WingRequest};
        let mut mission = AiMission::new();
        for (id, side, wing, z) in [
            (1, 1, 0, 0.0),
            (2, 1, 0, 0.0),
            (3, 1, 0, 0.0),
            (4, 1, 1, 0.0),
            (10, 2, 0, 9000.0),
            (11, 2, 0, 12000.0),
        ] {
            let mut setup = setup(id, side, 0, [0.0, 20000.0, z], 0.0);
            setup.identity.wing = wing;
            mission.push(AiActor::new(setup).unwrap());
        }
        for (actor, target) in [(2, 10), (3, 10), (4, 11)] {
            mission
                .order(
                    actor,
                    WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(target))),
                )
                .unwrap()
                .unwrap();
        }
        run(&mut mission, 1);
        assert_eq!(mission.actor(1).unwrap().controller.target(), Some(11));
    }

    #[test]
    fn equal_aligned_stores_keep_the_first_station_instead_of_rewarding_narrow_cones() {
        let mut setup = setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0);
        setup.stations.truncate(1);
        let mut narrow = setup.stations[0].clone();
        narrow.station = StationId(7);
        narrow.employment_limit_deg = Some(10.0);
        setup.stations.push(narrow);
        let mut actor = AiActor::new(setup).unwrap();
        let own = actor.own_state(&flat);
        let mut target = object(&actor, 2);
        target.id = 2;
        target.position[2] = 2000.0;
        let targets = [actor.observed_target(&target, target.position)];
        let stations = actor.station_views(&targets, &own);
        let mut fired = None;
        for tick in 0..2400 {
            let mut frame = dummy_frame(&own);
            frame.tick = tick;
            frame.targets = &targets;
            frame.stations = &stations;
            let output = actor.controller.step(&frame).unwrap();
            if let Some(weapon) = output.weapons.first() {
                fired = Some(weapon.request.station);
                break;
            }
        }
        assert_eq!(fired, Some(StationId(0)));
    }
    #[test]
    fn terrain_between_live_actors_blocks_release_without_spending_ammunition() {
        let mut mission = AiMission::new();
        mission.push(AiActor::new(setup(1, 1, 0, [0.0, 20000.0, 0.0], 0.0)).unwrap());
        mission.push(
            AiActor::new(setup(2, 2, 0, [0.0, 20000.0, 5000.0], std::f64::consts::PI)).unwrap(),
        );
        let initial: Vec<_> = mission.actors.iter().map(|a| a.flight.clone()).collect();
        let ammunition: Vec<_> = mission
            .actors
            .iter()
            .map(AiActor::rounds_remaining)
            .collect();
        let ridge = |_x: f64, z: f64| {
            if (2000.0..3000.0).contains(&z) {
                21000.0
            } else {
                0.0
            }
        };
        for tick in 0..2400 {
            for (actor, flight) in mission.actors.iter_mut().zip(&initial) {
                actor.flight = flight.clone();
            }
            let world = world_of(&mission);
            assert!(
                mission
                    .step(&world, &ridge, TimeOfDay(tick))
                    .unwrap()
                    .launches
                    .is_empty()
            );
        }
        assert_eq!(
            mission
                .actors
                .iter()
                .map(AiActor::rounds_remaining)
                .collect::<Vec<_>>(),
            ammunition
        );
    }
    #[test]
    fn directed_delivery_checks_sender_recipient_and_initial_physical_heading() {
        use super::super::targeting::Side;
        use super::super::wing::{PlayerBreak, ReceiverOutcome};
        let mut mission = AiMission::new();
        for (id, side) in [(1, 1), (2, 1), (3, 2)] {
            mission.push(AiActor::new(setup(id, side, 1, [0., 20000., 0.], 90.)).unwrap());
        }
        let report = mission
            .order_wing_report(Side(1), 0, Some(1), Some(3), PlayerBreak::Left.request())
            .unwrap();
        assert!(report.is_empty());
        assert!(
            mission
                .order_wing_report(Side(1), 0, Some(3), Some(2), PlayerBreak::Left.request())
                .is_err()
        );
        let heading = mission.actor(2).unwrap().flight().yaw.to_degrees().round() as i32;
        let report = mission
            .order_wing_report(Side(1), 0, Some(1), Some(2), PlayerBreak::Right.request())
            .unwrap();
        assert_eq!(report.len(), 1);
        let ReceiverOutcome::MotionInstalled(motion) = report[0].1 else {
            panic!("no motion");
        };
        assert_eq!(motion.heading_deg, (heading + 170).rem_euclid(360));
        assert!(motion.speed.0 > 0., "first-tick order needs real limits");
    }
    #[test]
    fn routine_formation_changes_use_local_physical_paths() {
        use super::super::{
            formation::Phase,
            wing::{WingRequest, formation_slot_point},
        };
        let mut cases = Vec::new();
        for from in Formation::ALL {
            for to in Formation::ALL {
                if from != to {
                    cases.push((from, to, 512, 512, 0, 0.0_f64));
                }
            }
        }
        cases.extend([
            (Formation::Echelon, Formation::Echelon, 512, 2048, 0, 0.),
            (Formation::Echelon, Formation::Echelon, 2048, 512, 0, 0.),
            (
                Formation::Echelon,
                Formation::LineAbreast,
                512,
                512,
                512,
                0.,
            ),
            (
                Formation::LineAbreast,
                Formation::Echelon,
                512,
                512,
                -512,
                0.75,
            ),
            (
                Formation::LineAbreast,
                Formation::Echelon,
                512,
                512,
                -512,
                1.5,
            ),
        ]);
        for (from, to, old_spacing, new_spacing, stacking, turn) in cases {
            let mut mission = AiMission::new();
            mission.set_spacing(old_spacing, 0);
            mission.set_formation(from);
            mission.set_external_leader(Side(1), 0, 0);
            for member in 1..=4 {
                let offset = formation_slot_point(from, member, old_spacing, 0).unwrap();
                let mut start = setup(
                    u32::from(member),
                    1,
                    member,
                    [offset[0], 20000., offset[2]],
                    0.,
                );
                start.home_airport = None;
                start.flight.speed = 800.;
                start.flight.velocity = [0., 0., 800.];
                mission.push(AiActor::new(start).unwrap());
            }
            let mut leader = object(mission.actor(1).unwrap(), 1);
            leader.id = 0;
            leader.position = [0., 20000., 0.];
            leader.human_controlled = true;
            let mut minimum = f64::INFINITY;
            let mut phases = [Phase::Close; 4];
            let mut entered = [false; 4];
            for tick in 0..36000 {
                if tick == 1200 {
                    mission
                        .order_wing(Side(1), 0, None, WingRequest::FormationSelection(to))
                        .unwrap();
                    mission.set_spacing(new_spacing, stacking);
                }
                if turn > 0. {
                    if tick == 3600 {
                        mission
                            .order_wing(
                                Side(1),
                                0,
                                None,
                                WingRequest::FormationSelection(Formation::LineAstern),
                            )
                            .unwrap();
                        mission.set_spacing(512, 0);
                    }
                    let heading = (turn * (tick as f64 / 120. - 10.).clamp(0., 60.)).to_radians();
                    leader.heading_deg = heading.to_degrees();
                    leader.velocity = [800. * heading.sin(), 0., 800. * heading.cos()];
                }
                let mut world = world_of(&mission);
                world.push(leader.clone());
                let before: Vec<_> = mission
                    .actors()
                    .iter()
                    .map(|a| a.flight().clone())
                    .collect();
                mission.step(&world, &flat, TimeOfDay(0)).unwrap();
                for (position, velocity) in leader.position.iter_mut().zip(leader.velocity) {
                    *position += velocity / 120.;
                }
                let mut achieved = world_of(&mission);
                achieved.push(leader.clone());
                for (i, actor) in mission.actors().iter().enumerate() {
                    let mut replay = before[i].clone();
                    replay.step_surface(actor.last_input(), |_, _| Surface::terrain(0.));
                    assert_eq!(replay, *actor.flight());
                    let trace = actor.controller().formation_trace().unwrap();
                    entered[i] |= trace.phase == Phase::Reposition;
                    if trace.phase != phases[i] {
                        eprintln!(
                            "{from:?}->{to:?} {old_spacing}->{new_spacing} V{stacking} turn={turn} t={:.1} actor={} phase={:?} error={:.0}",
                            tick as f64 / 120.,
                            actor.id(),
                            trace.phase,
                            trace.slot_distance_ft
                        );
                        phases[i] = trace.phase;
                    }
                    for other in achieved.iter().filter(|o| o.id != actor.id()) {
                        minimum = minimum.min(distance(actor.flight().position, other.position));
                    }
                    assert!(
                        turn > 0.75 || !matches!(trace.phase, Phase::Breakout | Phase::Intercept),
                        "routine change entered recovery: {trace:?}"
                    );
                }
            }
            eprintln!(
                "{from:?}->{to:?} {old_spacing}->{new_spacing} V{stacking} turn={turn}: minimum={minimum:.1}"
            );
            assert!(minimum > 250.);
            for (i, actor) in mission.actors().iter().enumerate() {
                let t = actor.controller().formation_trace().unwrap();
                assert!(entered[i]);
                assert_eq!(t.phase, Phase::Close, "unfinished: {t:?}");
                assert!(t.slot_distance_ft < 100.);
            }
        }
    }
}

#[cfg(test)]
#[path = "engagement_integration_tests.rs"]
mod engagement_integration_tests;

#[cfg(test)]
#[path = "airfield_integration_tests.rs"]
mod airfield_integration_tests;
#[cfg(test)]
#[path = "envelope_rows_tests.rs"]
mod envelope_rows_tests;

#[cfg(test)]
#[path = "vertical_field_tests.rs"]
mod vertical_field_tests;

#[cfg(test)]
#[path = "thought_tests.rs"]
mod thought_tests;

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "mission_checkpoint.rs"]
mod checkpoint;

/// [`AiActor::speed_limits`] of any flight state: the minimum, maximum and
/// corner speed the AI reads from the loaded envelopes at the aircraft's
/// altitude. The AC-130's orbit hold plans with the same numbers as the AI
/// gunship (docs/spec/ac130-linked-guns.md, "The orbit").
pub fn speed_limits_of(flight: &crate::flight::State) -> SpeedLimits {
    // Every row's band once: the passes below all read them.
    let rows = crate::flight::RowSpeeds::of(flight);
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut corner = f64::NAN;
    let mut best_g = i32::MIN;
    for (envelope, speeds) in rows.rows() {
        let Some((low, high)) = speeds else {
            continue;
        };
        minimum = minimum.min(low);
        maximum = maximum.max(high);
        if envelope.g > best_g {
            best_g = envelope.g;
            corner = low;
        }
    }
    if !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        // No envelope covers this altitude. Fall back to the documented
        // fixture band rather than producing a non-finite limit.
        return SpeedLimits {
            minimum: ScalarSpeed(FALLBACK_MINIMUM_FPS),
            maximum: ScalarSpeed(FALLBACK_MAXIMUM_FPS),
            corner: ScalarSpeed(FALLBACK_CORNER_FPS),
        };
    }
    // Fitted (agent decision, 2026-09-29, after the weight-scaled stall
    // speeds): the lowest slow edge over every row, 0 G and negative G
    // included, lies well under the speed at which the loaded aircraft
    // can hold 1 G. The AI never plans to fly slower than that, in its
    // current flap setting (`flight::State::minimum_level_speed`); on the
    // F/A-18D the rows alone gave 114 kt against the 140 kt it needs, and
    // finals built on it sank into the ground.
    let level = flight.minimum_level_speed_with(&rows, flight.flaps);
    if level < maximum {
        minimum = minimum.max(level);
    }
    // Fitted (agent decision, 2026-09-29): nor faster than the fastest
    // row that still leaves the loaded aircraft TOP_SPEED_G_MARGIN of
    // lift. Near its top speed only the 1 G row covers the aircraft, and
    // the legacy flight model divides that 1 G by the loading, so a
    // loaded fighter at full power there sank steadily into flat ground
    // at 1,100 kt without being able to pull up (fuzz seeds 43, 183,
    // 266).
    let config = flight.model().configuration();
    let load = 1.0
        + (flight.fuel + flight.payload_lbs) / config.mass.empty_lbs
            * config.aerodynamics.loaded_elevator_percent
            / 100.0;
    let manoeuvring = rows
        .rows()
        .filter(|(e, _)| f64::from(e.g) / load >= TOP_SPEED_G_MARGIN)
        .filter_map(|(_, speeds)| speeds.map(|(_, high)| high))
        .fold(f64::NEG_INFINITY, f64::max);
    if manoeuvring > minimum {
        maximum = maximum.min(manoeuvring);
    }
    if !corner.is_finite() {
        corner = minimum;
    }
    SpeedLimits {
        minimum: ScalarSpeed(minimum),
        maximum: ScalarSpeed(maximum),
        corner: ScalarSpeed(corner.min(maximum)),
    }
}
