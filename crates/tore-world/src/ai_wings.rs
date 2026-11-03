//! M1e AI-6: the live hookup between `tore_sim::ai` and the running mission.
//!
//! This module is a bridge and nothing else. Every decision, every store debit
//! and every motion belongs to [`tore_sim::ai::mission`]; everything here only
//! moves data between that mission and the host's existing combat world:
//!
//! - one [`WorldObject`] snapshot per tick, built from the player's
//!   authoritative [`flight::State`] plus each AI actor's own flight state,
//! - each actor's pose mirrored into the matching `live::Target` so the
//!   existing renderer, sensors, missile collision and damage keep working off
//!   one world,
//! - damage mirrored back so a dead target kills its actor,
//! - each [`LaunchEvent`] realised as a real `live::Projectile` flown by the
//!   existing missile code, never by a second physics path,
//! - each missile turned into a [`ThreatReport`] delivered only to the aircraft
//!   it is aimed at (B47: no broadcast).
//!
//! The creator uses this bridge by default. `--fixture-wings` keeps the old
//! straight-flight launch and integration; ordinary free flight has no bridge.
//!
//! Projectiles carry actor-owned weapon and fire-control observations into
//! the shared seeker lifecycle. AI defense receives the same bounded missile
//! information service as the player RWR. Compatibility steering remains an
//! explicit weapon-rules option.

mod chatter;
pub use chatter::{Chatter, Contact, ContactView, FuelLevel, Member};
mod engagement;
pub use engagement::Preset;
mod ground;
mod gunship;
mod jettison;
mod orders;
pub(crate) use orders::{GroundAim, GroundFact, OrderReport, order_label};
pub mod outcome;
mod record;
pub use record::DecoyRoll;
mod release;
mod reports;
mod strike;
mod strike_reach;
mod takeover;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tore_formats::aircraft::{Aircraft, AircraftId};
use tore_sim::{
    ai::{
        ScalarSpeed,
        airfield::{GroundStart, RunwayView},
        controller::{
            Activity, ActorIdentity, BehaviorFamily, BehaviorProfile, MissionRole, ThreatReport,
        },
        launch::{self, WingLaunch},
        mission::{
            ActorSetup, AiActor, AiMission, EquipmentFaults, HumanMember, LaunchEvent, Places,
            WorldObject, simple_dispensers, simple_stations,
        },
        route,
        targeting::Side,
        threat::{SeekerClass, TimeOfDay},
        weapon_service::{ActorId, RequestId},
    },
    airport::ApproachEnd,
    attitude::{Basis, Vector, unit},
    combat::{
        FallState, launch_speed,
        live::{self, MAX_PROJECTILES},
        missiles::{self, Flight, LaunchMode, Motion, Rules, seeker},
    },
    sensors::{self, Observable, Sensors},
};

use crate::{WorldResult, mission::Situation, resources::ResourceSource, terrain::Terrain};
use tore_sim::flight;

fn terrain_visible(from: Vector, to: Vector, ground: &dyn Fn(f64, f64) -> f64) -> bool {
    (1..=8).all(|step| {
        let t = step as f64 / 8.;
        let point = std::array::from_fn::<_, 3, _>(|i| from[i] + (to[i] - from[i]) * t);
        point[1] > ground(point[0], point[2])
    })
}

/// `fitted`: the AI side numbers. `tore_sim::ai::targeting::Side` is an opaque
/// identity with no recovered numbering, so the host picks one. The player and
/// every friendly wing are side 1, every enemy wing is side 2. Rule: the
/// selector rejects same-side candidates and nothing else reads the value, so
/// any two distinct numbers are equivalent; these are chosen for readability.
pub const FRIENDLY_SIDE: Side = Side(1);
/// See [`FRIENDLY_SIDE`].
pub const ENEMY_SIDE: Side = Side(2);

/// The aircraft single player flies. Single player's dummy target ids start
/// at 1 after it, and an open mission's at 0, with plane 0 an AI aircraft, so
/// actor ids and target ids are always the same number. The AI no longer reads
/// this: every human-flown aircraft reaches it as a [`HumanAircraft`] with its
/// own id, and this is only the id single player's entry carries.
pub const PLAYER_ID: u32 = 0;

/// Fitted Quick Mission placement, agent choice: use B43 echelon slots at
/// 512 ft spacing, level with the player. Friendly wing 1 occupies slots
/// behind the player. Wings 2 and 3 start 4096 ft behind and respectively
/// 4096 ft left and right. Enemy leaders start at the selected separation,
/// with wings 2 and 3 offset 4096 ft left and right. The creator's situation
/// ([`Situation`]) decides where and which way: ahead facing the player
/// (neutral), ahead flying away from it (advantage) or behind it flying the
/// player's heading (disadvantage).
/// Original Quick Mission spawn geometry is unknown. These offsets make the
/// selected allies nearby instead of placing them in the enemy group.
///
/// `turn` rotates the spawn about the reference point, so an enemy group can
/// be aimed back onto the map (John's request, 2026-09-23;
/// `docs/spec/quick-mission-menu.md#mission-wings`). `runway_order` marks a
/// member of the player's wing that starts parked on the runway; its offset
/// is the runway slot relative to the player's slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MissionSpawn {
    pub offset: Vector,
    /// The member flies for the enemy side.
    pub opposing: bool,
    /// The member starts facing back toward the player's group: its heading
    /// is the player's turned half a circle (an enemy in a neutral start).
    pub head_on: bool,
    /// Clockwise rotation about the reference point, radians.
    pub turn: f64,
    /// Departure order when this member starts on the runway.
    pub runway_order: Option<u8>,
    /// The member flies in the player's own wing, so it starts where the
    /// player does, whatever the aircraft.
    pub with_player: bool,
}

impl MissionSpawn {
    pub fn pose(self, position: Vector, basis: Basis) -> (Vector, Basis) {
        let [yaw, pitch, bank] = basis.angles();
        let frame = if self.turn == 0.0 {
            basis
        } else {
            Basis::new(yaw + self.turn, pitch, bank)
        };
        let position = std::array::from_fn(|i| {
            position[i] + frame.right[i] * self.offset[0] + frame.forward[i] * self.offset[2]
        });
        let heading = yaw
            + self.turn
            + if self.head_on {
                std::f64::consts::PI
            } else {
                0.0
            };
        (position, Basis::new(heading, 0.0, 0.0))
    }
}

/// How the creator's choices place the wings around the player.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnPlan {
    /// Distance from the player to the enemy group's placement point, feet.
    pub separation_ft: f64,
    /// Rotation of the whole enemy group about the player, radians clockwise.
    pub enemy_turn: f64,
    /// Runway slots for the player's wing as [right, forward] offsets from
    /// the player's slot, leader first. `None` is an airborne start.
    pub runway_slots: Option<Vec<[f64; 2]>>,
    /// Who is on whose tail: where the airborne enemy wings start and which
    /// way they face. Always neutral when the layout placed the enemy by a
    /// ground target or the player starts on the ground.
    pub situation: Situation,
}

impl SpawnPlan {
    /// Everyone airborne, enemies straight ahead.
    pub fn airborne(separation_ft: f64) -> Self {
        Self {
            separation_ft,
            enemy_turn: 0.0,
            runway_slots: None,
            situation: Situation::Neutral,
        }
    }
}

/// Where each wing member starts, in payload order, with the player leading
/// friendly wing 1 ahead of its members.
pub fn mission_spawns(wings: &[WingLaunch], plan: &SpawnPlan) -> Vec<MissionSpawn> {
    mission_spawns_for(wings, plan, true)
}

/// [`mission_spawns`], where `player` says whether the player flies the lead
/// of friendly wing 1 outside its members (single player). Without, as in an
/// open mission, the wing's member 0 is its lead and starts on the player's
/// spot, plane 0's.
pub fn mission_spawns_for(
    wings: &[WingLaunch],
    plan: &SpawnPlan,
    player: bool,
) -> Vec<MissionSpawn> {
    use tore_sim::ai::wing::{Formation, formation_slot_point};
    wings
        .iter()
        .flat_map(|wing| {
            wing.members.iter().map(move |member| {
                let opposing = wing.wing.side.is_enemy();
                let player_wing = !opposing && wing.wing.index == 0;
                let slot = member.member + u8::from(player && player_wing);
                if player_wing
                    && let Some(runway) = plan.runway_slots.as_ref()
                    && let Some([right, forward]) = runway.get(usize::from(slot))
                {
                    return MissionSpawn {
                        offset: [*right, 0.0, *forward],
                        opposing,
                        head_on: false,
                        turn: 0.0,
                        runway_order: Some(slot),
                        with_player: true,
                    };
                }
                let offset = if slot == 0 {
                    [0.0; 3]
                } else {
                    formation_slot_point(Formation::Echelon, slot, 512, 0)
                        .expect("validated Quick Mission member fits the formation table")
                };
                let lateral = match wing.wing.index {
                    1 => -4096.0,
                    2 => 4096.0,
                    _ => 0.0,
                };
                // An enemy facing the player mirrors its formation; one
                // flying the player's heading keeps it, its wingmen trailing
                // toward the player (advantage) or away (disadvantage).
                let (offset, head_on) = match (opposing, plan.situation) {
                    (false, _) => (
                        [
                            lateral + offset[0],
                            0.0,
                            offset[2] - if wing.wing.index == 0 { 0.0 } else { 4096.0 },
                        ],
                        false,
                    ),
                    (true, Situation::Neutral) => (
                        [lateral - offset[0], 0.0, plan.separation_ft - offset[2]],
                        true,
                    ),
                    (true, Situation::Advantage) => (
                        [lateral + offset[0], 0.0, plan.separation_ft + offset[2]],
                        false,
                    ),
                    (true, Situation::Disadvantage) => (
                        [lateral + offset[0], 0.0, offset[2] - plan.separation_ft],
                        false,
                    ),
                };
                MissionSpawn {
                    offset,
                    opposing,
                    head_on,
                    turn: if opposing { plan.enemy_turn } else { 0.0 },
                    runway_order: None,
                    with_player: player_wing,
                }
            })
        })
        .collect()
}

/// Every enemy aircraft's offset from the enemy group's placement point, as
/// [right, forward] in the frame whose forward axis points from the player to
/// that point: straight ahead of the player, or straight behind it in a
/// disadvantage. The placement point itself is always included, so the map
/// check never passes an empty group.
pub fn enemy_group_offsets(wings: &[WingLaunch], situation: Situation) -> Vec<[f64; 2]> {
    let plan = SpawnPlan {
        situation,
        ..SpawnPlan::airborne(0.0)
    };
    // Behind the player the frame is the player's turned half a circle.
    let sign = if situation == Situation::Disadvantage {
        -1.0
    } else {
        1.0
    };
    let mut offsets: Vec<[f64; 2]> = mission_spawns(wings, &plan)
        .into_iter()
        .filter(|spawn| spawn.opposing)
        .map(|spawn| [sign * spawn.offset[0], sign * spawn.offset[2]])
        .collect();
    if !offsets.is_empty() && !offsets.contains(&[0.0, 0.0]) {
        offsets.push([0.0, 0.0]);
    }
    offsets
}

/// `opinionated` (agent decision, 2026-09-17): AI projectiles take ids from
/// their own high range so they can never be confused with the player's shots,
/// whose ids are `live::State::shots`. Rule: the player fires far fewer than
/// sixteen million rounds in a session, so the two ranges cannot meet, and the
/// player's shot counter is left untouched by AI fire.
pub const AI_PROJECTILE_ID_BASE: u32 = 1 << 24;

/// Fitted synthetic fixture inventory. Imported missions replace it with each
/// aircraft's reviewed default weapon and dispenser records before stepping.
pub const AI_MISSILES: u32 = 4;
/// See [`AI_MISSILES`].
pub const AI_GUN_ROUNDS: u32 = 500;
/// See [`AI_MISSILES`]. Feet per second, the store speed the weapon service
/// uses for its own range and time-of-flight estimates.
pub const AI_STORE_SPEED: ScalarSpeed = ScalarSpeed(2000.0);
/// See [`AI_MISSILES`]: countermeasures per dispenser class.
pub const AI_DISPENSER_COUNT: u32 = 30;

/// `opinionated` (agent decision, 2026-09-17): the activity bar holds one line
/// for four seconds, so the bridge posts at most one AI line every this many
/// ticks. Rule: 240 ticks is two seconds at the fixed 120 Hz rate, which lets a
/// second line replace the first only after it has been readable for half its
/// life, instead of a new line every tick.
pub const MESSAGE_INTERVAL_TICKS: u64 = 240;

/// The message channel is opinionated too: only a change *into* one of these
/// activities is worth interrupting the player for. Formation, idle and pursuit
/// changes happen constantly and say nothing a player would act on.
fn worth_announcing(activity: Activity) -> bool {
    matches!(
        activity,
        Activity::Attacking | Activity::Defending | Activity::Breaking | Activity::Destroyed
    )
}

/// One AI aircraft's host-side identity, kept so the bridge can name it in a
/// message and find its target row without searching the launch payload again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Actor id, which is also the `live::Target` id.
    pub id: u32,
    pub side: launch::Side,
    /// Wing number as the setup screen shows it, 1 through 3.
    pub wing_number: u8,
    /// Member number inside the wing, 1 based for display.
    pub member_number: u8,
    pub aircraft: AircraftId,
}

impl Slot {
    /// "Enemy 2-1", the wing and member a player would recognise.
    pub fn label(&self) -> String {
        let side = match self.side {
            launch::Side::Friendly => "Friendly",
            launch::Side::Enemy => "Enemy",
        };
        format!("{side} {}-{}", self.wing_number, self.member_number)
    }
}

/// Where one human-flown aircraft sits in the mission: its id and its place in
/// a wing. The wing is the setup screen's index (0 through 2) and the member
/// is 0 for the wing's first aircraft, as in [`ActorIdentity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HumanSlot {
    pub id: u32,
    pub side: launch::Side,
    pub wing: u8,
    pub member: u8,
}

impl HumanSlot {
    /// Single player: the leader of Friendly wing 1 flies aircraft 0.
    pub const SINGLE_PLAYER: Self = Self {
        id: PLAYER_ID,
        side: launch::Side::Friendly,
        wing: 0,
        member: 0,
    };
}

/// One human-flown aircraft as the AI sees it this tick. The host hands the AI
/// one entry per such aircraft, in id order. The AI builds a world object with
/// `human_controlled` set from each, where it used to build one for "the
/// player".
pub struct HumanAircraft<'a> {
    pub slot: HumanSlot,
    /// The aircraft's authoritative flight state.
    pub flight: &'a flight::State,
    /// Its combat hit points; zero or less means destroyed.
    pub hit_points: i32,
    /// What the aircraft shows to other aircraft's sensors (from its combat
    /// configuration).
    pub signature: sensors::SignatureProfile,
    pub jammer: Option<sensors::JammerProfile>,
    /// The external stores it carries now, which raise its radar signature.
    pub stores: sensors::Stores,
}

impl<'a> HumanAircraft<'a> {
    /// The aircraft as its combat ownship `own` holds it: hit points,
    /// signature, jammer and live external stores.
    pub fn new(slot: HumanSlot, flight: &'a flight::State, own: &live::Ownship) -> Self {
        Self::from_parts(
            slot,
            flight,
            own.hp,
            own.configuration(),
            own.external_stores(),
        )
    }

    pub(crate) fn from_parts(
        slot: HumanSlot,
        flight: &'a flight::State,
        hit_points: i32,
        config: &live::Configuration,
        stores: sensors::Stores,
    ) -> Self {
        Self {
            slot,
            flight,
            hit_points,
            signature: config.sensors.signature,
            jammer: config.sensors.jammer.clone(),
            stores,
        }
    }

    fn alive(&self) -> bool {
        !self.flight.crashed && self.hit_points > 0
    }
}

/// An AI aircraft taken out of the AI, for a human to fly. Its flight state,
/// stores, dispensers, sensors and warnings are the aircraft's; the AI's
/// awareness, orders and decisions are gone.
pub struct RemovedActor {
    pub slot: Slot,
    pub parts: tore_sim::ai::mission::ActorParts,
    /// The AI's skill, kept for putting the aircraft back.
    pub experience: tore_sim::ai::experience::ResolvedExperience,
    /// Its combat configuration, when the bridge had one.
    pub config: Option<live::Configuration>,
}

/// What an aircraft needs to join the AI: the same as at mission start (its
/// place in the roster, the seed rule, fresh awareness, neutral) from the
/// state it is in.
pub struct ActorInsert {
    /// The aircraft id, also its combat target row's id.
    pub id: u32,
    pub side: launch::Side,
    pub wing: u8,
    /// Its member number in the wing, from 0.
    pub member: u8,
    pub aircraft: AircraftId,
    pub experience: tore_sim::ai::experience::ResolvedExperience,
    pub flight: flight::State,
    pub sensors: Option<Sensors>,
    pub stations: Vec<tore_sim::ai::mission::StationSpec>,
    pub dispensers: Vec<tore_sim::ai::threat::DispenserStore>,
    /// Missile warnings to keep; `None` starts a fresh record.
    pub warnings: Option<tore_sim::combat::threats::ThreatService>,
    pub equipment: EquipmentFaults,
    /// Its combat configuration: the weapon records its stations release, its
    /// guns and its countermeasure effectiveness.
    pub config: Option<live::Configuration>,
}

impl ActorInsert {
    /// Put a removed actor back the way it left.
    pub fn from_removed(removed: RemovedActor) -> Self {
        let identity = removed.parts.identity;
        Self {
            id: removed.slot.id,
            side: removed.slot.side,
            wing: identity.wing,
            member: identity.member,
            aircraft: removed.slot.aircraft,
            experience: removed.experience,
            flight: removed.parts.flight,
            sensors: removed.parts.sensors,
            stations: removed.parts.stations,
            dispensers: removed.parts.dispensers,
            warnings: Some(removed.parts.warnings),
            equipment: removed.parts.equipment,
            config: removed.config,
        }
    }
}

/// Which flight model the AI's aircraft fly, a mission setting.
///
/// *Agent decision:* `AllHybrid` seeds each aircraft as the AI probe's
/// `--probe-flight-model researched` does (seed `1 + aircraft id`), and leaves
/// training targets, which only drift on a straight line, as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiFlightModel {
    /// Single player as it is: legacy, except wingmen that start on the ground,
    /// aircraft that begin a landing, and the helicopters and the V-22 (slice
    /// H1), which fly the hybrid model.
    #[default]
    Standard,
    /// Every AI aircraft flies the hybrid model from mission start, so a human
    /// taking one over never feels its handling change.
    AllHybrid,
}

impl AiFlightModel {
    pub const ALL: [Self; 2] = [Self::Standard, Self::AllHybrid];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::AllHybrid => "all-hybrid",
        }
    }
}

impl std::str::FromStr for AiFlightModel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "standard" => Ok(Self::Standard),
            "all-hybrid" | "all_hybrid" | "allhybrid" => Ok(Self::AllHybrid),
            _ => Err(format!(
                "unknown AI flight model {value:?}; expected {}",
                Self::ALL.map(Self::name).join(", ")
            )),
        }
    }
}

/// The formation trace samples every 12 simulation ticks (10 Hz).
const FORMATION_TRACE_EVERY: u64 = 12;
/// The file is flushed on the first sample of each second (every 120 ticks).
const FORMATION_TRACE_FLUSH_EVERY: u64 = 120;
/// The most rows held between two drains. The app drains every tick, so this is
/// only a bound: a wing of 29 aircraft adds 29 rows per sample, and rows past
/// the limit are dropped.
const FORMATION_TRACE_MAX_ROWS: usize = 4096;

/// One sampled row of the formation trace: what one aircraft's formation logic
/// decided this tick and where the aircraft is. Decision quantities precede
/// that tick's physics; the achieved ones follow it. Plain numbers only: the
/// app turns rows into the CSV file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormationRow {
    pub tick: u64,
    pub actor: u32,
    pub trace: tore_sim::ai::formation::Trace,
    pub position: [f64; 3],
    pub speed: f64,
    pub bank: f64,
    pub g: f64,
    pub pitch_input: f64,
    pub roll_input: f64,
    pub yaw_input: f64,
    pub throttle: f64,
    pub afterburner: bool,
}

/// What [`AiWings::take_formation_trace`] hands over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormationBatch {
    pub rows: Vec<FormationRow>,
    /// True when a sample fell on a whole second since the last drain, the
    /// point where the file is flushed.
    pub flush: bool,
}

/// The AI's weapon stations for an aircraft built from `config`: one per
/// configuration station, in station order, with the configuration's full
/// counts. With `guns_only` every station but the gun starts empty.
pub(crate) fn station_specs(
    config: &live::Configuration,
    guns_only: bool,
) -> Vec<tore_sim::ai::mission::StationSpec> {
    let counts: Vec<u16> = config.stations.iter().map(|s| s.count).collect();
    station_specs_loaded(config, &counts, guns_only)
}

/// [`station_specs`] with each station carrying `quantities` (a player's
/// loadout) instead of its full count.
pub(crate) fn station_specs_loaded(
    config: &live::Configuration,
    quantities: &[u16],
    guns_only: bool,
) -> Vec<tore_sim::ai::mission::StationSpec> {
    let mut stores = Vec::new();
    for (index, station) in config.stations.iter().enumerate() {
        let w = &station.weapon;
        let gun = config.aircraft.mounts_gun(&w.source);
        let units = u32::from(quantities.get(index).copied().unwrap_or(station.count));
        // Source bookkeeping must use the same ammo units for a human taking
        // this depot's aircraft. This changes no targeting or steering rules.
        // A rocket pod's rockets are counted the same way, `projsInPod` a pod
        // (retail's `HARDPodHack` multiplies every station's count at mission
        // start, the AI's and a wing member's included).
        let carried = units
            * config
                .gun_pods
                .iter()
                .find(|pod| pod.station == index)
                .filter(|_| w.source == "SUU16.JT")
                .map_or_else(|| live::pod_rounds(w), |pod| u32::from(pod.rounds_per_pod));
        // An empty station is the same record with no rounds.
        let mut spec = if gun {
            simple_stations(0, carried.max(1), AI_STORE_SPEED).remove(0)
        } else {
            simple_stations(carried.max(1), 0, AI_STORE_SPEED).remove(0)
        };
        spec.store.rounds = tore_sim::ai::weapon_service::Rounds::Finite(carried);
        if guns_only && !gun {
            spec.store.rounds = tore_sim::ai::weapon_service::Rounds::Finite(0);
        }
        spec.station = tore_sim::ai::weapon_service::StationId(index as u8);
        spec.guided = w.flags & 1 != 0;
        spec.capability = tore_sim::ai::weapon_service::StoreCapability {
            air: w.flags & 0x10000 != 0,
            surface: w.flags & 0x20000 != 0,
        };
        // The reviewed default inventory owns the record used at release.
        spec.debit = u32::from(w.burst.actual_rounds_per_game).max(1);
        // A rocket pod's weight is the full pod's: each rocket takes its share.
        // The same carried mass as a player's loadout (`carried_weight_lb`),
        // so one loadout weighs the same under the AI and a human.
        spec.external_round_lbs = if station.internal || w.source == "SUU16.JT" {
            0.0
        } else {
            w.carried_weight_lb() / f64::from(live::pod_rounds(w))
        };
        // Fitted: one representative projectile per release, as in
        // the player live adapter. The source ammunition debit remains separate.
        spec.projectile_count = 1;
        spec.store_speed = ScalarSpeed(f64::from(w.movement.maximum_speed));
        spec.tracking_delay =
            tore_sim::ai::weapon_service::Delay::quarters(u32::from(w.guidance.track_t));
        spec.damage_vs_category = f64::from(w.damage.by_class[0]);
        spec.mount = if gun {
            station.gun_mount()
        } else {
            station.mount
        };
        spec.infrared_envelope = (w.flags & 0x10000 != 0
            && w.seeker_kind() == tore_formats::weapons::SeekerKind::Infrared)
            .then_some((
                w.infrared_all_aspect(),
                f64::from(w.seeker.zones[0].maximum_range),
            ));
        spec.requires_radar = w.seeker_kind() == tore_formats::weapons::SeekerKind::SemiActiveRadar;
        spec.requires_sensor = w.flags & 0x400 != 0;
        let zone = w.seeker.zones[1];
        spec.minimum_range_ft = f64::from(zone.minimum_range);
        spec.maximum_range_ft = Some(f64::from(zone.maximum_range));
        spec.employment_limit_deg = None;
        spec.employment_zone = Some(zone);
        stores.push(spec);
    }
    stores
}

/// Whether `flight` is a helicopter or the V-22. These AI aircraft fly the
/// hybrid model (their powered-lift physics, through the AI's rotor
/// adapter) under every AI flight model, `Standard` included: on the legacy
/// model a rotorcraft flies as an aeroplane and the AI could never hover,
/// take off or land it. Opinionated (John, 2026-10-12: AI helicopter and
/// VTOL hover, takeoff and landing; the lead's decision on the mechanism,
/// slice H1). The vectoring jets keep the AI flight model's choice.
fn hovers(flight: &flight::State) -> bool {
    flight.model().powered_lift().is_some_and(|lift| {
        matches!(
            lift.kind,
            tore_sim::models::variety::LiftKind::Helicopter
                | tore_sim::models::variety::LiftKind::Tiltrotor
        )
    })
}

/// Jet throttle is a heat input. Rotor collective is not engine heat; retain
/// the existing observation (including a handoff's engine snapshot).
pub(crate) fn energy_heat(flight: &flight::State, previous: seeker::Heat) -> seeker::Heat {
    if hovers(flight) {
        seeker::Heat::Engine {
            on: flight.engine,
            throttle: 1.,
            afterburner: false,
        }
    } else {
        aircraft_heat(flight, previous)
    }
}

pub(crate) fn aircraft_heat(flight: &flight::State, previous: seeker::Heat) -> seeker::Heat {
    if hovers(flight) {
        previous
    } else {
        seeker::Heat::Engine {
            on: flight.engine,
            throttle: flight.throttle,
            afterburner: flight.burner,
        }
    }
}

/// Puts an AI aircraft on the hybrid flight model, seeded as the AI probe
/// seeds its researched actors. An airborne powered-lift aircraft that has
/// not flown yet is trimmed into forward flight as it goes, so a helicopter or
/// a jet that a multiplayer seat, a revival or a mission start puts on the
/// model begins in balance, with its final mass and altitude (VTOL overhaul,
/// decision 8). Its velocity is air-relative, as every AI start's is.
fn enter_hybrid(flight: &mut flight::State, seed: i32) -> WorldResult<()> {
    flight.enable_research(seed)?;
    flight.start_airborne([0.; 3]);
    Ok(())
}

/// What an AI aircraft built from `config` carries: its external equipment
/// and every round of `stores` hung outside it, pounds.
fn payload_lbs(config: &live::Configuration, stores: &[tore_sim::ai::mission::StationSpec]) -> f64 {
    f64::from(config.external_equipment_lbs)
        + stores
            .iter()
            .map(|s| match s.rounds() {
                tore_sim::ai::weapon_service::Rounds::Finite(n) => {
                    f64::from(n) * s.external_round_lbs
                }
                tore_sim::ai::weapon_service::Rounds::Unlimited => 0.0,
            })
            .sum::<f64>()
}

/// A new aircraft for the AI, one the mission did not start with: a
/// revival's (stage F phase 2, slice F2-V), built as the mission builds an AI
/// aircraft with a lobby loadout.
pub(crate) struct NewAircraft {
    pub id: u32,
    pub side: launch::Side,
    pub wing: u8,
    /// Its member number in the wing, from 0.
    pub member: u8,
    /// Its combat configuration, with the loadout's weapons on its stations.
    pub config: live::Configuration,
    /// What each station carries.
    pub quantities: Vec<u16>,
    pub fuel_lbs: f64,
    /// Its flight state, placed and moving.
    pub flight: flight::State,
    /// The creator's Guns only: every station but the gun starts empty.
    pub guns_only: bool,
    /// Its wing's own skill as the mission launched it (slice R1), or
    /// `None` to borrow a wingmate's.
    pub experience: Option<tore_sim::ai::experience::ResolvedExperience>,
    /// A training target (an AI respawn in a dummy wing, slice R1).
    pub dummy: bool,
}

/// A receiver of perceived attacks: its id, the human flying it with its
/// ownship, and the mission's actor of that id.
type Receiver<'a, 'h> = (
    u32,
    Option<(&'a HumanAircraft<'h>, &'a live::Ownship)>,
    Option<&'a AiActor>,
);

/// One perceived attack: the receiver, its report, the world bearing and
/// the event's id.
type PerceivedAttack = (
    u32,
    tore_sim::ai::engagement::ThreatReport,
    Option<f64>,
    Option<u32>,
);

/// The live AI bridge for one mission.
pub struct AiWings {
    mission: AiMission,
    mission_preset: Preset,
    reports: reports::Reports,
    /// Formation trace rows waiting for the app; `None` while tracing is off.
    formation_trace: Option<FormationBatch>,
    slots: Vec<Slot>,
    /// The mission setting for the AI's flight model.
    flight_model: AiFlightModel,
    /// The theater's airfields, kept for aircraft inserted later.
    airfields: Airfields,
    /// Every human-flown aircraft, in id order. The host refreshes it each
    /// step ([`Self::step`]); a fresh bridge starts with single player's one.
    humans: Vec<HumanSlot>,
    weapons: BTreeMap<(u32, u8), tore_formats::weapons::Weapon>,
    device_random: tore_sim::ai::DecisionRandom,
    device_effectiveness: BTreeMap<u32, (u8, u8)>,
    /// Each actor's combat configuration, for what its hardpoints carry.
    configs: BTreeMap<u32, live::Configuration>,
    /// Stations a system fault has put out of action; they stay out when
    /// Air combat guns only is turned off.
    damaged_stations: std::collections::BTreeSet<(u32, u8)>,
    /// The skill of each aircraft a human has taken, kept for putting it
    /// back in the AI when the human gives it back.
    handed_over: BTreeMap<u32, tore_sim::ai::experience::ResolvedExperience>,
    /// The air-to-ground tasking (Strike role, ground order) of each
    /// aircraft a human has taken, put back with it (slice ST-K).
    handed_tasking: BTreeMap<u32, takeover::Tasking>,
    /// Draws for what a damaged ECM suite loses, apart from the decoy rolls.
    fault_random: tore_sim::ai::DecisionRandom,
    /// Projectile ids already turned into threat reports.
    seen_projectiles: BTreeSet<u32>,
    /// How far each kind of surface missile may be launched at a
    /// helicopter (its threat rings), worked out once per kind: a fixed
    /// function of the records, never checkpointed.
    rotor_reach: crate::surface::rotor_reach::RotorReach,
    /// Each guided store's time of flight to a ground target (a
    /// helicopter's stand-off advance), by record and whether the energy
    /// rules fly it: a fixed function of the record, never checkpointed.
    flight_times: BTreeMap<(String, bool), Option<tore_sim::ai::strike::FlightTimes>>,
    /// The answers on guided shots from farther out a striker plans a
    /// stand-off release with (John, 2026-10-29), by record, launch band and
    /// range step: a fixed function of the record, never checkpointed.
    shot_cache: std::sync::Arc<strike_reach::ShotCache>,
    /// Projectile id to the actor that fired it, for B47 attribution. The
    /// player's shots are absent; the bridge remembers its own launchers.
    ai_shots: BTreeMap<u32, u32>,
    /// Last observed hit points per actor, for the damage mirror.
    last_hp: BTreeMap<u32, i32>,
    /// Aircraft lost this tick to overspeed: no shooter earns
    /// them (see [`AiWings::lose_uncredited`]).
    uncredited_losses: std::collections::BTreeSet<u32>,
    pub ejection_events: Vec<(u32, String, bool)>,
    /// Radio events for `radio_calls`, drained by the host each tick.
    pub chatter: Vec<Chatter>,
    watch: chatter::Watch,
    last_activity: BTreeMap<u32, Activity>,
    next_projectile_id: u32,
    weapon_rules: Rules,
    last_message_tick: u64,
    /// The aircraft the activity line names, and a later change of that
    /// aircraft's activity that arrived inside the interval.
    shown_actor: Option<u32>,
    stale_line: Option<(u32, Activity)>,
    /// Launch events that could not become a projectile, for honest reporting.
    pub dropped_launches: u32,
    /// Projectiles this bridge created.
    pub realised_launches: u32,
    /// The most recent B47 deliveries as (receiving actor, missile id). Only
    /// the aircraft a missile is aimed at ever appears here.
    threat_reports: Vec<(u32, u32)>,
    /// A line for `FlightUi::message`, taken by the host once.
    pending_message: Option<String>,
    pending_guns: BTreeMap<(u32, u8), PendingGun>,
    gun_ordinals: BTreeMap<(u32, u8), u64>,
    /// Enemy AI cheat level in force; None leaves each aircraft its own.
    enemy_skill: Option<tore_sim::ai::Experience>,
    /// Each enemy's mission skill, kept so Unchanged can restore it.
    mission_skill: BTreeMap<u32, tore_sim::ai::experience::ResolvedExperience>,
    /// Air combat guns only cheat in force.
    guns_only: bool,
    /// The human-flown aircraft that took off and have not yet lined up on an
    /// approach, so their gear-down climb-out does not claim landing priority.
    departing: std::collections::BTreeSet<u32>,
    /// What the latest [`Self::step`] produced, kept for the replay recorder
    /// and debug panels. Nothing reads it back into a decision.
    last_output: tore_sim::ai::mission::MissionOutput,
    /// Every decoy roll of the latest [`Self::step`], for the replay
    /// recorder. Write-only: no decision reads it.
    decoy_rolls: Vec<DecoyRoll>,
    /// The highest the world's ground reaches over each square, worked out
    /// at the first step: the mission's shortcut for sight lines above it.
    ground_ceiling: Option<std::sync::Arc<tore_sim::ground_ceiling::GroundCeiling>>,
    /// The AI test settings (observation rate, lean sensors): host setup the
    /// step hands to the mission. A checkpoint leaves them out.
    ai_test: tore_sim::ai::test_settings::AiTestSettings,
    /// The mission's airborne height, feet (the Quick Mission altitude
    /// setting): a heavy striker's release floor (slice ST-T2). The host sets
    /// it before every step; `None` lets a striker use the height it was
    /// tasked at.
    mission_height_ft: Option<f64>,
    /// The surface radars painting aircraft this tick, as (aircraft,
    /// emitter) pairs (slice H4): a helicopter learns where they stand. The
    /// host sets it before every step.
    painting: Vec<(u32, u32)>,
}

struct PendingGun {
    groups: VecDeque<(u32, u16)>,
    next_scaled: u64,
    ordinal: u64,
}

/// `fitted`: the per-actor decision seed.
///
/// Rule: `side * 1_000_000 + wing * 1_000 + member`, mixed with a fixed salt so
/// two actors never share a stream and the same setup screen always produces
/// the same run. The spec gives draw thresholds, never sequences, so any
/// injective derivation satisfies it; this one is readable in a probe dump.
pub fn actor_seed(side: launch::Side, wing: u8, member: u8) -> u64 {
    let side = u64::from(side.is_enemy());
    ACTOR_SEED_SALT ^ (side * 1_000_000 + u64::from(wing) * 1_000 + u64::from(member))
}

/// The salt of [`actor_seed`] and [`new_actor_seed`].
const ACTOR_SEED_SALT: u64 = 0x5749_4E47_5F41_4900; // "WING_AI\0"

/// `fitted` (slice R1): the decision seed of an aircraft the mission did not
/// start with (a revival's or an AI respawn's new plane), from its plane id.
/// Plane ids never repeat, and the values sit above every [`actor_seed`]
/// value (at most 1,255,255 before the salt), so no two actors share a
/// stream.
pub fn new_actor_seed(id: u32) -> u64 {
    ACTOR_SEED_SALT ^ (2_000_000 + u64::from(id))
}

fn side_of(side: launch::Side) -> Side {
    if side.is_enemy() {
        ENEMY_SIDE
    } else {
        FRIENDLY_SIDE
    }
}

/// Where each target row is by id, for a walk of the AI slots: sorted when
/// there are enough slots to repay the sort, otherwise found by a walk of the
/// rows. Either way the first row of an id answers.
enum Rows {
    Sorted(Places),
    Walk,
}
impl Rows {
    fn of(targets: &[live::Target], lookups: usize) -> Self {
        if lookups > 4 {
            Self::Sorted(Places::of(targets.iter().map(|t| t.id)))
        } else {
            Self::Walk
        }
    }
    fn get(&self, targets: &[live::Target], id: u32) -> Option<usize> {
        match self {
            Self::Sorted(places) => places.get(id),
            Self::Walk => targets.iter().position(|t| t.id == id),
        }
    }
}

/// The widest maximum airspeed any loaded envelope permits at this altitude.
/// `fitted`: `WorldObject::maximum_speed` has no recovered producer, so the
/// bridge reads the same envelope block the flight model and the AI's own
/// `speed_limits` read, and falls back to the AI module's documented fixture
/// value when no envelope covers the altitude.
fn maximum_speed(state: &flight::State) -> ScalarSpeed {
    // The rows' bands the AI's own speed limits read at this altitude.
    let best = flight::RowSpeeds::of(state)
        .bands()
        .filter_map(|(_, speeds)| speeds)
        .map(|(_, maximum)| maximum)
        .fold(f64::NEG_INFINITY, f64::max);
    if best.is_finite() {
        ScalarSpeed(best)
    } else {
        ScalarSpeed(tore_sim::ai::mission::FALLBACK_MAXIMUM_FPS)
    }
}

/// Each human-flown aircraft's launcher, for the ones combat keeps an ownship
/// for, in id order: what combat's missile support reads for their shots.
fn human_launchers(state: &live::State, humans: &[&HumanAircraft]) -> Vec<(u32, live::Launcher)> {
    humans
        .iter()
        .filter(|h| state.ownship(h.slot.id).is_some())
        .map(|h| (h.slot.id, crate::combat::launcher(h.flight)))
        .collect()
}

/// Records the intended target of the rounds `realise` just added, so the
/// debrief can count unguided fire aimed at each aircraft.
fn aim_latest(state: &mut live::State, emitted: u32, target: u32) {
    let first = state.projectiles.len().saturating_sub(emitted as usize);
    for projectile in &state.projectiles[first..] {
        state.ledger.aim(projectile.id, target);
    }
}

/// One runway an AI aircraft may call home, with the sides allowed to use it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomeRunway {
    pub view: RunwayView,
    pub friendly: bool,
    pub enemy: bool,
}

/// A wing's parked start: the departure runway and one surface point and
/// heading per aircraft, leader first. Friendly Wing 1's leader is the
/// player in single player; Enemy Wing 1 parks on one for an enemy ground
/// start ([`Airfields::enemy_departure`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Departure {
    pub runway: RunwayView,
    pub headings: Vec<f64>,
    pub slots: Vec<Vector>,
}

/// What the AI needs to know about the theater's airfields at launch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Airfields {
    /// Map bounds used to clip creator CAP regions. Setup, retained on restore.
    pub patrol_bounds: Option<[[f64; 2]; 2]>,
    pub runways: Vec<HomeRunway>,
    /// Present when the player's wing starts on the ground.
    pub departure: Option<Departure>,
    /// Present when Enemy Wing 1 starts on the ground (slice CL-R, the
    /// Redfor mirror of [`Self::departure`]).
    pub enemy_departure: Option<Departure>,
}

impl Airfields {
    /// `fitted`, agent decision 2026-09-23: which airfields each side may
    /// return to. It mirrors the player's tower service and airport list: a
    /// field of the side, or a neutral one that grants permission
    /// ([`Airport::serves`](tore_sim::airport::Airport::serves)). Allegiance is
    /// recorded from Blue's point of view and taken from the runway's layout
    /// side (slice AL1), so for the enemy side a hostile field is its own. The
    /// single neutral-permission flag is used for both sides because no
    /// per-side permission is recorded.
    pub fn from_scene(scene: &tore_sim::airport::Scene, departure: Option<Departure>) -> Self {
        let mut runways = Vec::new();
        for airport in &scene.airports {
            let friendly = airport.serves(false);
            let enemy = airport.serves(true);
            if !friendly && !enemy {
                continue;
            }
            // Spec-derived: conventional aircraft never land on a vertical
            // pad, so none is anyone's home. Opinionated, John 2026-09-30: nor
            // is a short strip (under `SHORT_STRIP_FT`), so no AI aircraft
            // picks one as its home runway, landing field or return point.
            for runway in airport
                .runway_objects
                .iter()
                .filter(|id| !scene.vertical_pad(**id) && !scene.short_strip(**id))
                .filter_map(|id| scene.runway(*id))
            {
                runways.push(HomeRunway {
                    view: RunwayView::from(runway),
                    friendly,
                    enemy,
                });
            }
        }
        Self {
            patrol_bounds: None,
            runways,
            departure,
            enemy_departure: None,
        }
    }

    /// [`from_scene`](Self::from_scene) with each runway's airfield points.
    pub fn from_world(world: &Terrain, departure: Option<Departure>) -> Self {
        let mut fields = Self::from_scene(&world.airport_scene, departure);
        fields.patrol_bounds = Some([
            [0., 0.],
            [
                (world.theater.cols - 1) as f64 * 8192.,
                (world.theater.rows - 1) as f64 * 8192.,
            ],
        ]);
        for runway in &mut fields.runways {
            runway.view.anchors = world.airfield_anchors.get(&runway.view.object).copied();
        }
        fields
    }

    /// These airfields with Enemy Wing 1 parked by `departure`.
    pub fn with_enemy_departure(mut self, departure: Option<Departure>) -> Self {
        self.enemy_departure = departure;
        self
    }

    /// The parked start of `side`'s first wing, if it starts on the ground.
    pub fn departure_of(&self, side: launch::Side) -> Option<&Departure> {
        if side.is_enemy() {
            self.enemy_departure.as_ref()
        } else {
            self.departure.as_ref()
        }
    }

    /// `fitted`, agent decision 2026-09-23: an aircraft's home is the nearest
    /// runway its side may use, measured horizontally from where it starts to
    /// the runway centre. Ties go to the lower runway object id so the choice
    /// never depends on list order. None when no runway is usable.
    pub fn home(&self, position: Vector, side: launch::Side) -> Option<RunwayView> {
        let enemy = side.is_enemy();
        self.runways
            .iter()
            .filter(|r| if enemy { r.enemy } else { r.friendly })
            .map(|r| {
                let d = (r.view.center[0] - position[0]).hypot(r.view.center[2] - position[2]);
                (d, r.view)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.object.cmp(&b.1.object)))
            .map(|(_, view)| view)
    }
}

impl AiWings {
    /// Build the bridge from a resolved launch payload and the targets the
    /// existing spawner has already placed.
    ///
    /// `targets` must be `combat.state.targets` immediately after
    /// `Combat::reset`, whose rows are the flattened wing members in payload
    /// order, with id `index + 1` in single player and `index` in an open
    /// mission. Nothing here recomputes a spawn position: the
    /// AI aircraft start exactly where the fixtures would have started.
    pub fn build(
        wings: &[WingLaunch],
        targets: &[live::Target],
        guns_only: bool,
        resources: &dyn ResourceSource,
    ) -> WorldResult<Self> {
        Self::build_mission(wings, targets, guns_only, resources, &Airfields::default())
    }

    /// [`build`](Self::build) with the theater's airfields: home runways for
    /// every aircraft and, for a ground start, the player's wing parked on the
    /// departure runway.
    pub fn build_mission(
        wings: &[WingLaunch],
        targets: &[live::Target],
        guns_only: bool,
        resources: &dyn ResourceSource,
        airfields: &Airfields,
    ) -> WorldResult<Self> {
        Self::build_mission_for(
            wings,
            targets,
            guns_only,
            resources,
            airfields,
            &[HumanSlot::SINGLE_PLAYER],
        )
    }

    /// [`build_mission`](Self::build_mission) with the mission's human-flown
    /// aircraft named: the AI fills each wing's member numbers the humans
    /// leave free, in order.
    pub fn build_mission_for(
        wings: &[WingLaunch],
        targets: &[live::Target],
        guns_only: bool,
        resources: &dyn ResourceSource,
        airfields: &Airfields,
        humans: &[HumanSlot],
    ) -> WorldResult<Self> {
        Self::build_mission_loaded(
            wings,
            targets,
            guns_only,
            resources,
            airfields,
            humans,
            &BTreeMap::new(),
        )
    }

    /// [`build_mission_for`](Self::build_mission_for) with the loadouts the
    /// players chose for some planes of an open mission (EF4): each such
    /// aircraft carries its loadout's stores, quantities and fuel, which a
    /// human taking it over keeps.
    pub fn build_mission_loaded(
        wings: &[WingLaunch],
        targets: &[live::Target],
        guns_only: bool,
        resources: &dyn ResourceSource,
        airfields: &Airfields,
        humans: &[HumanSlot],
        loadouts: &BTreeMap<u32, tore_sim::combat::loadout::Loadout>,
    ) -> WorldResult<Self> {
        let mut bridge = Self::build_for(wings, targets, airfields, humans, |id| {
            let bytes = resources
                .get(id.pt())
                .ok_or_else(|| format!("aircraft cache missing {}", id.pt()))?;
            let mut aircraft = Aircraft::parse(bytes)?;
            aircraft.id = id;
            let found = sensors::SensorProfiles::from_source(&aircraft, |name| {
                resources
                    .get(name)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
            })?;
            Ok((aircraft, Some(found)))
        })?;
        for actor in bridge.mission.actors_mut() {
            let pt = actor.identity().aircraft.pt();
            let aircraft = Aircraft::parse(
                resources
                    .get(pt)
                    .ok_or_else(|| format!("aircraft cache missing {pt}"))?,
            )?;
            let loaded = loadouts.get(&actor.id());
            let config = match loaded {
                Some(load) => load.configuration.clone(),
                None => live::Configuration::from_source(&aircraft, |name| {
                    resources
                        .get(name)
                        .cloned()
                        .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
                })?,
            };
            let stores = match loaded {
                Some(load) => station_specs_loaded(&config, &load.quantities, guns_only),
                None => station_specs(&config, guns_only),
            };
            if let Some(load) = loaded {
                actor.flight_mut().fuel = load.fuel_lbs;
            }
            // Every AI aircraft starts with its tanks full and its engine
            // count, the player's rule, whether a loadout was given or it
            // flies its aircraft's default load (which can hang a tank: the
            // F/A-18's and F-16C's centreline). Before 2026-10-20 only a
            // given loadout did this, so a default tank was carried full
            // weight with no fuel in it and a twin flew one engine's systems.
            actor.flight_mut().systems =
                tore_sim::aircraft_systems::Systems::new(config.engines, config.external_fuel_lbs);
            for (index, station) in config.stations.iter().enumerate() {
                bridge
                    .weapons
                    .insert((actor.id(), index as u8), station.weapon.clone());
            }
            actor
                .flight_mut()
                .set_payload(payload_lbs(&config, &stores))?;
            actor.set_stations(stores);
            actor.set_guns(
                config
                    .stations
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| live::is_gun(&s.weapon))
                    .map(|(i, s)| (i as u8, s.weapon.clone()))
                    .collect(),
            );
            actor.set_dispensers(vec![
                tore_sim::ai::threat::DispenserStore {
                    class: SeekerClass::Infrared,
                    count: u32::from(config.ecm.flare[0]),
                },
                tore_sim::ai::threat::DispenserStore {
                    class: SeekerClass::Radar,
                    count: u32::from(config.ecm.chaff[0]),
                },
            ]);
            bridge
                .device_effectiveness
                .insert(actor.id(), (config.ecm.flare[1], config.ecm.chaff[1]));
            bridge.configs.insert(actor.id(), config);
        }
        Ok(bridge)
    }

    /// [`build`](Self::build) with the aircraft records supplied by the caller,
    /// so a test can build a mission from a synthetic profile and no media.
    #[cfg(any(test, feature = "test-support"))]
    pub fn build_with(
        wings: &[WingLaunch],
        targets: &[live::Target],
        _station: usize,
        resolve: impl FnMut(AircraftId) -> WorldResult<(Aircraft, Option<sensors::SensorProfiles>)>,
    ) -> WorldResult<Self> {
        Self::build_at(wings, targets, &Airfields::default(), resolve)
    }

    /// [`build_with`](Self::build_with) with the theater's airfields.
    pub fn build_at(
        wings: &[WingLaunch],
        targets: &[live::Target],
        airfields: &Airfields,
        resolve: impl FnMut(AircraftId) -> WorldResult<(Aircraft, Option<sensors::SensorProfiles>)>,
    ) -> WorldResult<Self> {
        Self::build_for(
            wings,
            targets,
            airfields,
            &[HumanSlot::SINGLE_PLAYER],
            resolve,
        )
    }

    /// [`build_at`](Self::build_at) with the mission's human-flown aircraft
    /// named. Each wing's AI members take the member numbers the humans in it
    /// leave free, lowest first, so one human at member 0 of Friendly wing 1
    /// (single player) shifts that wing's AI aircraft to members 1 and up.
    pub fn build_for(
        wings: &[WingLaunch],
        targets: &[live::Target],
        airfields: &Airfields,
        humans: &[HumanSlot],
        mut resolve: impl FnMut(AircraftId) -> WorldResult<(Aircraft, Option<sensors::SensorProfiles>)>,
    ) -> WorldResult<Self> {
        let mut humans = humans.to_vec();
        humans.sort_by_key(|h| h.id);
        let mut mission = AiMission::new();
        // Opinionated host setup: level delta formations using B43's
        // alternating trailing slots, 512 ft spacing, independently per wing.
        mission.set_spacing(512, 0);
        mission.set_humans(
            humans
                .iter()
                .map(|h| HumanMember {
                    id: h.id,
                    side: side_of(h.side),
                    wing: h.wing,
                    member: h.member,
                    pilot_alive: false,
                })
                .collect(),
        );
        let mut slots = Vec::new();
        let mut watch = chatter::Watch::default();
        let mut profiles: Vec<(AircraftId, Aircraft, Option<sensors::SensorProfiles>)> = Vec::new();
        let mut index = 0usize;
        for wing in wings {
            if wing.is_empty() {
                continue;
            }
            if !profiles.iter().any(|(id, _, _)| *id == wing.aircraft) {
                let (aircraft, found) = resolve(wing.aircraft)?;
                profiles.push((wing.aircraft, aircraft, found));
            }
            let (_, aircraft, found) = profiles
                .iter()
                .find(|(id, _, _)| *id == wing.aircraft)
                .expect("just inserted");
            watch.learn(wing.aircraft, aircraft);
            let taken: Vec<u8> = humans
                .iter()
                .filter(|h| h.side == wing.wing.side && h.wing == wing.wing.index)
                .map(|h| h.member)
                .collect();
            for member in &wing.members {
                // A wing with humans in it leaves their member numbers free
                // of AI aircraft: the k-th AI member takes the k-th number no
                // human holds.
                let member_index = (0u8..)
                    .filter(|number| !taken.contains(number))
                    .nth(usize::from(member.member))
                    .expect("member numbers are unbounded");
                let target = targets.get(index).ok_or_else(|| {
                    format!(
                        "AI wing member {index} has no spawned target; the fixture spawner and the launch payload disagree"
                    )
                })?;
                let mut state = flight::State::new(aircraft, target.position)?;
                let [yaw, pitch, bank] = target.basis.angles();
                state.yaw = yaw;
                state.pitch = pitch;
                state.bank = bank;
                // `opinionated` (agent decision, 2026-09-17): the AI aircraft
                // keeps the flight model's own start airspeed rather than the
                // 300 ft/s drift velocity `live::State::add_dummy` gives a
                // straight-flight fixture. Rule: 300 ft/s is below the stall
                // speed of every ported fighter, so an actor handed it would
                // spend its first seconds recovering instead of flying. The
                // spawn *position* and *attitude* are unchanged.
                state.velocity = Basis::new(yaw, pitch, bank)
                    .forward
                    .map(|v| v * state.speed);
                // A ground start parks Friendly Wing 1, the wing the
                // departure is laid out for, on the departure runway instead
                // of dropping it from altitude, whether or not a human flies
                // in it: in an open mission the AI parks its lead too. An
                // enemy ground start parks Enemy Wing 1 the same way on its
                // own (slice CL-R). Only the researched flight model can
                // stand on a runway.
                let departure = airfields
                    .departure_of(wing.wing.side)
                    .filter(|_| wing.wing.index == 0);
                let ground_start = match departure {
                    Some(departure) => {
                        let order = usize::from(member_index);
                        let (Some(slot), Some(heading)) =
                            (departure.slots.get(order), departure.headings.get(order))
                        else {
                            return Err(format!(
                                "the ground start has no slot for wingman {member_index}"
                            )
                            .into());
                        };
                        // Deterministic per-aircraft seed: the lead (the
                        // player, or the AI's plane 0) uses 1, wingman n uses
                        // 1 + n.
                        state.enable_research(1 + i32::from(member_index))?;
                        state.start_on_runway(*slot, *heading)?;
                        Some(GroundStart {
                            runway: departure.runway,
                            end: ApproachEnd::Near,
                            order: member_index,
                        })
                    }
                    None => None,
                };
                // A wing that started on a vertical pad (it cannot land
                // there) goes home to the nearest usable runway instead.
                let home = match &ground_start {
                    Some(start)
                        if airfields
                            .runways
                            .iter()
                            .any(|r| r.view.object == start.runway.object) =>
                    {
                        Some(start.runway)
                    }
                    _ => airfields.home(target.position, wing.wing.side),
                };
                let sensors = found.clone().map(Sensors::new);
                let setup = ActorSetup {
                    identity: ActorIdentity {
                        actor: ActorId(target.id),
                        side: side_of(wing.wing.side),
                        wing: wing.wing.index,
                        member: member_index,
                        // The wing's first member leads at the start.
                        leads: member_index == 0,
                        aircraft: wing.aircraft,
                        human_controlled: false,
                    },
                    profile: BehaviorProfile {
                        family: BehaviorFamily::FighterStrike,
                        role: MissionRole::AirToAir,
                    },
                    experience: member.experience,
                    seed: actor_seed(wing.wing.side, wing.wing.index, member.member),
                    flight: state,
                    sensors,
                    stations: simple_stations(AI_MISSILES, AI_GUN_ROUNDS, AI_STORE_SPEED),
                    dispensers: simple_dispensers(AI_DISPENSER_COUNT),
                    wing_slot: member_index.max(1),
                    // `fitted`: the home airport is the home runway's centre.
                    // Without a usable runway the spawn point stands in, as
                    // before. Rule: B48 only needs somewhere to fly home to
                    // when fuel runs low, and the spawn point is the one
                    // position the setup screen actually decided.
                    home_airport: Some(match home {
                        Some(runway) => route::Position {
                            x: runway.center[0],
                            z: runway.center[2],
                        },
                        None => route::Position {
                            x: target.position[0],
                            z: target.position[2],
                        },
                    }),
                };
                let mut actor = AiActor::new(setup).map_err(|e| e.to_string())?;
                if wing.dummy {
                    actor.set_dummy();
                }
                // After the ground start, which would otherwise make the
                // departure runway home.
                if let Some(start) = ground_start {
                    actor.start_on_ground(start);
                }
                actor.set_home_runway(home);
                mission.push(actor);
                slots.push(Slot {
                    id: target.id,
                    side: wing.wing.side,
                    wing_number: wing.wing.display_number(),
                    member_number: member_index + 1,
                    aircraft: wing.aircraft,
                });
                index += 1;
            }
        }
        mission.start_in_formation();
        // The helicopters and the V-22 fly the powered-lift physics under
        // every AI flight model (slice H1).
        for actor in mission
            .actors_mut()
            .iter_mut()
            .filter(|a| !a.is_dummy() && hovers(a.flight()))
        {
            if actor.flight().research.is_none() {
                let seed = 1 + actor.id() as i32;
                enter_hybrid(actor.flight_mut(), seed)?;
            }
        }
        Ok(Self {
            mission,
            mission_preset: Preset::Free,
            formation_trace: None,
            slots,
            flight_model: AiFlightModel::Standard,
            airfields: airfields.clone(),
            humans,
            weapons: BTreeMap::new(),
            device_random: tore_sim::ai::DecisionRandom::seeded(0xdec0),
            device_effectiveness: BTreeMap::new(),
            configs: BTreeMap::new(),
            damaged_stations: Default::default(),
            handed_over: Default::default(),
            handed_tasking: Default::default(),
            fault_random: tore_sim::ai::DecisionRandom::seeded(0xfa17),
            seen_projectiles: BTreeSet::new(),
            rotor_reach: Default::default(),
            flight_times: BTreeMap::new(),
            shot_cache: Default::default(),
            ai_shots: BTreeMap::new(),
            ejection_events: Vec::new(),
            chatter: Vec::new(),
            watch,
            last_hp: BTreeMap::new(),
            uncredited_losses: Default::default(),
            last_activity: BTreeMap::new(),
            reports: reports::Reports::default(),
            next_projectile_id: AI_PROJECTILE_ID_BASE,
            weapon_rules: Rules::Spec,
            last_message_tick: 0,
            shown_actor: None,
            stale_line: None,
            dropped_launches: 0,
            realised_launches: 0,
            threat_reports: Vec::new(),
            pending_message: None,
            pending_guns: BTreeMap::new(),
            gun_ordinals: BTreeMap::new(),
            enemy_skill: None,
            mission_skill: BTreeMap::new(),
            guns_only: false,
            departing: Default::default(),
            last_output: tore_sim::ai::mission::MissionOutput::default(),
            decoy_rolls: Vec::new(),
            ground_ceiling: None,
            ai_test: Default::default(),
            mission_height_ft: None,
            painting: Vec::new(),
        })
    }

    /// Take AI aircraft `id` out of the AI, for a human to fly. The others
    /// keep their order. The combat target row stays where it is: the caller
    /// turns it into the human's own record.
    pub fn remove_actor(&mut self, id: u32) -> Option<RemovedActor> {
        let slot = *self.slot(id)?;
        let actor = self.mission.remove_actor(id)?;
        let experience = actor.experience();
        self.hand_over_tasking(id, &actor);
        let mut parts = actor.into_parts();
        // What the human gets is what the aircraft's systems put out of
        // action, not what the mission's Air combat guns only setting holds
        // back; that setting is the AI's, and the human's selection ring
        // has its own.
        let aircraft = slot.aircraft;
        for spec in &mut parts.stations {
            let by_setting = self.guns_only
                && self
                    .weapons
                    .get(&(id, spec.station.0))
                    .is_some_and(|w| !aircraft.mounts_gun(&w.source))
                && !self.damaged_stations.contains(&(id, spec.station.0));
            if by_setting {
                spec.store.inhibited = false;
            }
        }
        self.handed_over.insert(id, experience);
        self.slots.retain(|s| s.id != id);
        self.weapons.retain(|(actor, _), _| *actor != id);
        self.device_effectiveness.remove(&id);
        let config = self.configs.remove(&id);
        self.damaged_stations.retain(|(actor, _)| *actor != id);
        self.pending_guns.retain(|(actor, _), _| *actor != id);
        self.gun_ordinals.retain(|(actor, _), _| *actor != id);
        self.last_hp.remove(&id);
        self.last_activity.remove(&id);
        self.mission_skill.remove(&id);
        self.watch.forget(id);
        Some(RemovedActor {
            slot,
            parts,
            experience,
            config,
        })
    }

    /// Put an aircraft into the AI, in id order, as at mission start: its
    /// place in the roster, the seed rule (its rank among the wing's members
    /// no human holds), fresh awareness and neutral. Its home is the nearest
    /// runway its side may use.
    pub fn insert_actor(&mut self, insert: ActorInsert) -> WorldResult<()> {
        let id = insert.id;
        self.insert_actor_seeded(insert, None)?;
        self.restore_tasking(id);
        Ok(())
    }

    /// [`Self::insert_actor`] with the decision seed `seed` in place of the
    /// roster's rule, when given.
    fn insert_actor_seeded(&mut self, insert: ActorInsert, seed: Option<u64>) -> WorldResult<()> {
        if self.mission.actor(insert.id).is_some() {
            return Err(format!("aircraft {} is already flown by the AI", insert.id).into());
        }
        let side = side_of(insert.side);
        // The AI members' launch numbering: the rank of this member number
        // among the ones no other human holds.
        let seed_member = (0..insert.member)
            .filter(|number| {
                !self.humans.iter().any(|h| {
                    h.id != insert.id
                        && h.side == insert.side
                        && h.wing == insert.wing
                        && h.member == *number
                })
            })
            .count() as u8;
        let home = self.airfields.home(insert.flight.position, insert.side);
        let position = insert.flight.position;
        let setup = ActorSetup {
            identity: ActorIdentity {
                actor: ActorId(insert.id),
                side,
                wing: insert.wing,
                member: insert.member,
                leads: insert.member == 0,
                aircraft: insert.aircraft,
                human_controlled: false,
            },
            profile: BehaviorProfile {
                family: BehaviorFamily::FighterStrike,
                role: MissionRole::AirToAir,
            },
            experience: insert.experience,
            seed: seed.unwrap_or_else(|| actor_seed(insert.side, insert.wing, seed_member)),
            flight: insert.flight,
            sensors: insert.sensors,
            stations: insert.stations,
            dispensers: insert.dispensers,
            wing_slot: insert.member.max(1),
            home_airport: Some(match home {
                Some(runway) => route::Position {
                    x: runway.center[0],
                    z: runway.center[2],
                },
                None => route::Position {
                    x: position[0],
                    z: position[2],
                },
            }),
        };
        // A station that arrives out of action is out for good, as a fault
        // leaves it, and stays so when Air combat guns only is turned off.
        let failed: Vec<u8> = setup
            .stations
            .iter()
            .filter(|spec| spec.store.inhibited)
            .map(|spec| spec.station.0)
            .collect();
        let mut actor = AiActor::new(setup).map_err(|e| e.to_string())?;
        if (self.flight_model == AiFlightModel::AllHybrid || hovers(actor.flight()))
            && actor.flight().research.is_none()
        {
            enter_hybrid(actor.flight_mut(), 1 + insert.id as i32)?;
        }
        actor.set_home_runway(home);
        if let Some(warnings) = insert.warnings {
            actor.set_missile_threats(warnings);
        }
        actor.fail_equipment(insert.equipment);
        if let Some(config) = insert.config {
            for (index, station) in config.stations.iter().enumerate() {
                self.weapons
                    .insert((insert.id, index as u8), station.weapon.clone());
            }
            actor.set_guns(
                config
                    .stations
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| live::is_gun(&s.weapon))
                    .map(|(i, s)| (i as u8, s.weapon.clone()))
                    .collect(),
            );
            self.device_effectiveness
                .insert(insert.id, (config.ecm.flare[1], config.ecm.chaff[1]));
            self.configs.insert(insert.id, config);
        }
        // Air combat guns only reaches every AI aircraft in the mission.
        if self.guns_only {
            let gun = |station: u8| {
                self.weapons
                    .get(&(insert.id, station))
                    .is_some_and(|w| insert.aircraft.mounts_gun(&w.source))
            };
            for spec in actor.stations_mut() {
                if !gun(spec.station.0) {
                    spec.store.inhibited = true;
                }
            }
        }
        self.mission
            .insert_actor(actor)
            .map_err(|e| e.to_string())?;
        self.damaged_stations
            .extend(failed.into_iter().map(|station| (insert.id, station)));
        self.handed_over.remove(&insert.id);
        let at = self.slots.partition_point(|s| s.id < insert.id);
        self.slots.insert(
            at,
            Slot {
                id: insert.id,
                side: insert.side,
                wing_number: insert.wing + 1,
                member_number: insert.member + 1,
                aircraft: insert.aircraft,
            },
        );
        Ok(())
    }

    /// Puts a new aircraft into the AI ([`NewAircraft`]): its stores, fuel
    /// and payload as the mission's build gives an aircraft with a lobby
    /// loadout, fresh sensors and missile warnings, the dispensers its
    /// configuration fills, and its wing's skill (a wingmate's when the
    /// mission's is not given, Average when the side has no AI aircraft
    /// left: agent decision, F2-V), the enemy skill cheat over it as over
    /// every enemy. Then as [`Self::insert_actor`], seeded by its plane id
    /// ([`new_actor_seed`], slice R1), and given the orders its wing flies
    /// under: a wingmate's assignment, the leader's first (agent decision,
    /// slice R1; a wing in a mission of opportunity is under free
    /// engagement).
    pub(crate) fn insert_new(&mut self, new: NewAircraft) -> WorldResult<()> {
        let NewAircraft {
            id,
            side,
            wing,
            member,
            config,
            quantities,
            fuel_lbs,
            mut flight,
            guns_only,
            experience,
            dummy,
        } = new;
        let stations = station_specs_loaded(&config, &quantities, guns_only);
        flight.fuel = fuel_lbs;
        flight.systems =
            tore_sim::aircraft_systems::Systems::new(config.engines, config.external_fuel_lbs);
        flight.set_payload(payload_lbs(&config, &stations))?;
        let own = experience
            .or_else(|| self.experience_for(id, side, wing))
            .unwrap_or(tore_sim::ai::experience::ResolvedExperience {
                level: tore_sim::ai::Experience::Average,
                origin: tore_sim::ai::experience::ExperienceOrigin::ExplicitPerObject,
            });
        let cheat = match self.enemy_skill {
            Some(level) if side == launch::Side::Enemy && experience.is_some() && !dummy => {
                Some(tore_sim::ai::experience::ResolvedExperience {
                    level,
                    origin: tore_sim::ai::experience::ExperienceOrigin::EnemyOverride,
                })
            }
            _ => None,
        };
        // The wing's orders, read before the aircraft joins it.
        let orders = {
            let leader = self.mission.wing_leader(side_of(side), wing);
            let wing_actors = || {
                self.mission
                    .actors()
                    .iter()
                    .filter(|a| a.identity().side == side_of(side) && a.identity().wing == wing)
            };
            wing_actors()
                .find(|a| Some(a.id()) == leader && a.alive())
                .or_else(|| wing_actors().find(|a| a.alive()))
                .or_else(|| wing_actors().next())
                .map(|a| a.assignment().clone())
        };
        // A new aircraft takes no tasking a lost one left (slice ST-K).
        self.handed_tasking.remove(&id);
        self.insert_actor_seeded(
            ActorInsert {
                id,
                side,
                wing,
                member,
                aircraft: config.aircraft,
                experience: cheat.unwrap_or(own),
                flight,
                sensors: Some(Sensors::new(config.sensors.clone())),
                stations,
                dispensers: vec![
                    tore_sim::ai::threat::DispenserStore {
                        class: SeekerClass::Infrared,
                        count: u32::from(config.ecm.flare[0]),
                    },
                    tore_sim::ai::threat::DispenserStore {
                        class: SeekerClass::Radar,
                        count: u32::from(config.ecm.chaff[0]),
                    },
                ],
                warnings: None,
                equipment: EquipmentFaults::default(),
                config: Some(config),
            },
            Some(new_actor_seed(id)),
        )?;
        if cheat.is_some() {
            self.mission_skill.insert(id, own);
        }
        if let Some(actor) = self.mission.actor_mut(id) {
            if let Some(orders) = orders
                && &orders != actor.assignment()
            {
                actor.set_assignment(orders);
            }
            if dummy {
                actor.set_dummy();
            }
        }
        Ok(())
    }

    /// Takes the wreck of AI aircraft `id` out of the AI for good (slice R1:
    /// an AI wreck retired to make room). Returns whether the AI flew it.
    pub(crate) fn retire_actor(&mut self, id: u32) -> bool {
        let removed = self.remove_actor(id).is_some();
        self.handed_over.remove(&id);
        removed
    }

    /// Cuts what each station of AI aircraft `id` carries to what `keep`
    /// leaves of its rounds, for its station's weapon (a revival's weapons
    /// rule, slice F2-V). Unlimited rounds stay unlimited unless `keep`
    /// leaves none of one round.
    pub(crate) fn cut_stores(
        &mut self,
        id: u32,
        keep: impl Fn(&tore_formats::weapons::Weapon, u32) -> u32,
    ) -> WorldResult<()> {
        let config = self
            .configs
            .get(&id)
            .ok_or_else(|| format!("the AI has no configuration for plane {id}"))?;
        let actor = self
            .mission
            .actor_mut(id)
            .ok_or_else(|| format!("the AI does not fly plane {id}"))?;
        for spec in actor.stations_mut() {
            let Some(station) = config.stations.get(usize::from(spec.station.0)) else {
                continue;
            };
            spec.store.rounds = match spec.store.rounds {
                tore_sim::ai::weapon_service::Rounds::Finite(rounds) => {
                    tore_sim::ai::weapon_service::Rounds::Finite(keep(&station.weapon, rounds))
                }
                unlimited if keep(&station.weapon, 1) > 0 => unlimited,
                _ => tore_sim::ai::weapon_service::Rounds::Finite(0),
            };
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// The mission's flight model for AI aircraft. `AllHybrid` puts every
    /// AI aircraft that is not on the hybrid model yet on it, seeded as the
    /// probe seeds its researched actors; aircraft inserted later follow it.
    /// `Standard` leaves the aircraft as they are, so it cannot undo
    /// `AllHybrid`. Set it once, before the mission steps.
    pub fn set_flight_model(&mut self, model: AiFlightModel) -> WorldResult<()> {
        self.flight_model = model;
        if model == AiFlightModel::AllHybrid {
            for actor in self
                .mission
                .actors_mut()
                .iter_mut()
                .filter(|a| !a.is_dummy())
            {
                if actor.flight().research.is_none() {
                    let seed = 1 + actor.id() as i32;
                    enter_hybrid(actor.flight_mut(), seed)?;
                }
            }
        }
        Ok(())
    }

    pub fn flight_model(&self) -> AiFlightModel {
        self.flight_model
    }

    /// Explicit headless-probe setup; normal mission adapter defaults are unchanged.
    pub fn configure_probe(
        &mut self,
        researched: bool,
        enemy_heading: Option<f64>,
        wind: Vector,
    ) -> WorldResult<()> {
        for actor in self.mission.actors_mut() {
            let id = actor.id();
            if actor.identity().side == ENEMY_SIDE
                && let Some(heading) = enemy_heading
            {
                let flight = actor.flight_mut();
                flight.yaw = heading;
                let forward = Basis::new(heading, flight.pitch, flight.bank).forward;
                flight.velocity = std::array::from_fn(|i| forward[i] * flight.speed + wind[i]);
            }
            if researched && actor.flight().research.is_none() {
                enter_hybrid(actor.flight_mut(), 1 + id as i32)?;
            }
        }
        Ok(())
    }

    /// The AI mission, changeable, for tests that set up an aircraft's state.
    #[cfg(test)]
    pub(crate) fn mission_mut(&mut self) -> &mut AiMission {
        &mut self.mission
    }

    pub fn mission(&self) -> &AiMission {
        &self.mission
    }

    /// Gives every AI aircraft `pounds` of internal fuel (the probe's
    /// `--probe-fuel`, which brings a low-fuel return forward without a
    /// half-hour mission; slice H3).
    pub fn set_internal_fuel_all(&mut self, pounds: f64) {
        for actor in self.mission.actors_mut() {
            actor.set_internal_fuel(pounds);
        }
    }

    /// Hand the AI mission the lead hold's claims for the next step (the
    /// lobby pass's slice R2; `world/lead_hold.rs`).
    pub(crate) fn set_lead_claims(&mut self, claims: Vec<tore_sim::ai::mission::LeadClaim>) {
        self.mission.set_lead_claims(claims);
    }

    /// Hand the AI what the flight data link holds for the next step, as
    /// [`Self::step`]'s humans are handed over: the humans' locked targets
    /// (slice G2), which its engagement table counts as attacks on the wing's
    /// behalf, and the tracks assigned wingmen fly toward while their own
    /// sensors do not hold the aircraft they were sent after (slice G3b), and
    /// what the AI leads share and sort from: the bandits each side knows and
    /// each member's last published state (slice G4). The step consumes
    /// them, so the world sets them before every step, after combat has read
    /// the sensors.
    pub fn set_link(&mut self, link: &crate::datalink::DataLink) {
        let (bandits, states) = link.lead_input();
        self.mission.set_link(tore_sim::ai::link::LinkInput {
            humans: link
                .human_engagements()
                .into_iter()
                .map(|e| tore_sim::ai::link::HumanEngagement {
                    plane: e.plane,
                    target: e.target,
                })
                .collect(),
            pursuits: link.pursuits(),
            bandits,
            states,
            wingmen: link.human_wingmen(),
        });
    }

    /// `actor` and a flightmate hold one bandit the lead did not mean: the AI
    /// actor leaves it alone for ten seconds when it has another to take
    /// (slice G4). `false` for an aircraft that is no living AI actor.
    pub fn yield_target(&mut self, actor: u32, target: u32) -> bool {
        self.mission.yield_target(actor, target)
    }

    /// The remaining waypoints of the wing aircraft `plane` flies in (a human
    /// or an AI aircraft), in order, world feet. A wing whose human leader is
    /// lost flies them after its search for the enemy
    /// ([`AiMission::set_wing_route`]). Quick Mission has no mission route
    /// yet, so only the probe calls this.
    pub fn set_wing_route(&mut self, plane: u32, route: Vec<[f64; 3]>) {
        let wing = self
            .humans
            .iter()
            .find(|h| h.id == plane)
            .map(|h| (side_of(h.side), h.wing))
            .or_else(|| {
                self.mission
                    .actor(plane)
                    .map(|a| (a.identity().side, a.identity().wing))
            });
        if let Some((side, wing)) = wing {
            self.mission.set_wing_route(side, wing, route);
        }
    }

    /// The seeker class (2 infrared, 3 radar) of each missile lock an enemy
    /// holds on aircraft `id`: its lock is on that aircraft
    /// ([`tore_sim::ai::link::lock_of`]: its target, with its weapon service
    /// past the lock check with a guided store chosen, waiting out the
    /// tracking delay or firing). Read-only, for the RWR warning tones
    /// (docs/spec/rwr.md#warning-tones).
    pub fn locks_on(&self, id: u32) -> Vec<u8> {
        self.mission
            .actors()
            .iter()
            .filter(|actor| {
                tore_sim::ai::link::lock_of(actor) == Some(id)
                    && self
                        .slot(actor.id())
                        .is_some_and(|slot| slot.side.is_enemy())
            })
            .filter_map(|actor| {
                let chosen = actor.controller().trace().weapons.as_ref()?.chosen?;
                let weapon = self.weapons.get(&(actor.id(), chosen.0))?;
                matches!(weapon.seeker.signature, 2 | 3).then_some(weapon.seeker.signature)
            })
            .collect()
    }

    /// The AI aircraft that engage `plane` ([`tore_sim::ai::link::engagement_of`]:
    /// alive, with `plane` as their target) and carry a guided air-to-air
    /// store that is ready (not inhibited, not spent). The situation music
    /// reads it as "a missile is being aimed at you". It asks for no lock, so
    /// the music builds before the warning receiver's tone. Read-only
    /// (docs/spec/flight-music.md).
    pub fn aiming_at(&self, plane: u32) -> Vec<u32> {
        use tore_sim::ai::weapon_service::{self, Rounds, TargetClass};
        self.mission
            .actors()
            .iter()
            .filter(|actor| {
                tore_sim::ai::link::engagement_of(actor) == Some(plane)
                    && actor.stations().iter().any(|s| {
                        s.guided
                            && !s.store.inhibited
                            && weapon_service::store_eligible(s.capability, TargetClass::Air)
                            && !matches!(s.store.rounds, Rounds::Finite(0))
                    })
            })
            .map(|actor| actor.id())
            .collect()
    }

    /// Recent B47 deliveries, newest last, as (receiving actor, missile id).
    pub fn threat_reports(&self) -> &[(u32, u32)] {
        &self.threat_reports
    }

    /// What the latest [`Self::step`] produced: launches, device releases,
    /// activities, fallbacks, wing requests and launch calls.
    pub fn last_output(&self) -> &tore_sim::ai::mission::MissionOutput {
        &self.last_output
    }

    /// Drain the AI message journal (attack reports, wing orders, escort
    /// priorities, missile warnings). The recorder calls this once per tick.
    pub fn take_ai_journal(&mut self) -> tore_sim::ai::thought::JournalBatch {
        self.mission.take_journal()
    }

    /// One AI aircraft's controller record of its latest tick.
    #[allow(dead_code)] // Read by the mission recorder and debug panels.
    pub fn controller_trace(&self, id: u32) -> Option<&tore_sim::ai::thought::ControllerTrace> {
        self.mission
            .actor(id)
            .map(|actor| actor.controller().trace())
    }

    /// One AI aircraft's mission record of its latest tick.
    #[allow(dead_code)] // Read by the mission recorder and debug panels.
    pub fn actor_trace(&self, id: u32) -> Option<&tore_sim::ai::thought::ActorTrace> {
        self.mission.actor(id).map(AiActor::trace)
    }

    pub fn slot(&self, id: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// The combat configuration of AI aircraft `id`, when the bridge holds one.
    pub fn configuration(&self, id: u32) -> Option<&live::Configuration> {
        self.configs.get(&id)
    }

    /// The skill for an aircraft of `side` and `wing` that joins the AI
    /// (index from 0): its own if the AI flew it before and a human took it,
    /// otherwise a wingmate's, otherwise the side's first aircraft's.
    pub fn experience_for(
        &self,
        id: u32,
        side: launch::Side,
        wing: u8,
    ) -> Option<tore_sim::ai::experience::ResolvedExperience> {
        let of = |slot: &Slot| self.mission.actor(slot.id).map(AiActor::experience);
        self.handed_over.get(&id).copied().or_else(|| {
            self.slots
                .iter()
                .filter(|s| s.side == side && s.wing_number == wing + 1)
                .find_map(of)
                .or_else(|| self.slots.iter().filter(|s| s.side == side).find_map(of))
        })
    }

    /// Enemy AI cheat: every enemy aircraft flies at `level` from now on, or
    /// at its own mission skill again for None. Friendly aircraft and
    /// straight-flight fixtures are unaffected.
    pub fn set_enemy_skill(&mut self, level: Option<tore_sim::ai::Experience>) {
        use tore_sim::ai::experience::{ExperienceOrigin, ResolvedExperience};
        if level == self.enemy_skill {
            return;
        }
        self.enemy_skill = level;
        for slot in self.slots.iter().filter(|s| s.side == launch::Side::Enemy) {
            let Some(actor) = self.mission.actor_mut(slot.id) else {
                continue;
            };
            if actor.is_dummy() {
                continue;
            }
            let own = *self
                .mission_skill
                .entry(slot.id)
                .or_insert_with(|| actor.experience());
            actor.set_experience(level.map_or(own, |level| ResolvedExperience {
                level,
                origin: ExperienceOrigin::EnemyOverride,
            }));
        }
    }

    /// Air combat guns only: every AI aircraft may fire only its gun. Turning
    /// it off restores the other stores; missiles in flight are unaffected.
    pub fn set_guns_only(&mut self, on: bool) {
        if on == self.guns_only {
            return;
        }
        self.guns_only = on;
        for slot in &self.slots {
            let Some(actor) = self.mission.actor_mut(slot.id) else {
                continue;
            };
            for spec in actor.stations_mut() {
                let gun = self
                    .weapons
                    .get(&(slot.id, spec.station.0))
                    .is_some_and(|w| slot.aircraft.mounts_gun(&w.source));
                if !gun {
                    spec.store.inhibited =
                        on || self.damaged_stations.contains(&(slot.id, spec.station.0));
                }
            }
        }
    }

    /// The aircraft on `side`, AI and human alike, which that side's T and
    /// Enter skip.
    pub fn friendly_ids(&self, side: launch::Side) -> std::collections::BTreeSet<u32> {
        self.slots
            .iter()
            .filter(|s| s.side == side)
            .map(|s| s.id)
            .chain(self.humans.iter().filter(|h| h.side == side).map(|h| h.id))
            .collect()
    }

    /// Take the pending activity line, if the rate limiter released one.
    pub fn take_message(&mut self) -> Option<String> {
        self.take_line()
    }

    /// Sets the AI test settings for the steps from now on. Defaults are
    /// today's behaviour; see [`tore_sim::ai::test_settings`]. The host sets
    /// them wherever it steps the world, and a restored world starts with the
    /// defaults until it does.
    pub fn set_ai_test(&mut self, settings: tore_sim::ai::test_settings::AiTestSettings) {
        self.ai_test = settings;
    }

    /// The AI test settings in force.
    pub fn ai_test(&self) -> tore_sim::ai::test_settings::AiTestSettings {
        self.ai_test
    }

    /// One 120 Hz tick of AI, run immediately after `Combat::step`.
    ///
    /// The order matters: combat has already flown the player, the projectiles
    /// and the straight-line target integration, so the bridge reads the damage
    /// combat just applied, decides, and then writes the authoritative AI pose
    /// into the targets. Next tick's straight-line integration therefore starts
    /// from the true AI pose and advances it by the true AI velocity, which is
    /// what the missile collision sweep needs.
    ///
    /// `humans` is one entry per human-flown aircraft, in id order. Each is a
    /// world object in every AI decision; the RWR records, emitters and sensor
    /// picture that reach the AI's attack evidence are the first ownship's.
    /// Uses `ceiling`, built from the terrain every step is given, instead
    /// of building the same one again.
    pub(crate) fn share_ground_ceiling(
        &mut self,
        ceiling: std::sync::Arc<tore_sim::ground_ceiling::GroundCeiling>,
    ) {
        self.ground_ceiling = Some(ceiling);
    }

    pub fn step(
        &mut self,
        state: &mut live::State,
        humans: &[HumanAircraft],
        world: &Terrain,
    ) -> WorldResult<()> {
        let mut humans: Vec<&HumanAircraft> = humans.iter().collect();
        humans.sort_by_key(|h| h.slot.id);
        self.humans = humans.iter().map(|h| h.slot).collect();
        // The mission works out each wing's leader from these and its actors:
        // a wing led by a human has that human as its external leader, and
        // one that loses it passes the lead on.
        self.mission.set_humans(
            humans
                .iter()
                .map(|h| HumanMember {
                    id: h.slot.id,
                    side: side_of(h.slot.side),
                    wing: h.slot.wing,
                    member: h.slot.member,
                    // Ejected and unhurt: the pilot is alive to say so.
                    pilot_alive: h.flight.escape.is_some() && !h.flight.systems.pilot.dead,
                })
                .collect(),
        );
        let ground = |x: f64, z: f64| f64::from(world.height(x as f32, z as f32));
        // The decoy draws and rolls describe this step only. Clearing the
        // log never touches the generator's state.
        self.device_random.clear_log();
        self.decoy_rolls.clear();
        self.weapon_rules = state.weapon_rules;
        self.mission
            .set_missiles(if state.weapon_rules.uses_seeker() {
                state.missile_snapshots(&human_launchers(state, &humans))
            } else {
                Vec::new()
            });
        self.mission
            .end_missile_threats(state.ended_missile_threats());
        let ceiling = self
            .ground_ceiling
            .get_or_insert_with(|| std::sync::Arc::new(world.ground_ceiling()));
        self.mission.set_ground_ceiling(Some(ceiling.clone()));
        self.mission.set_test_settings(self.ai_test);
        self.mission.set_gun_rounds(
            state
                .projectiles
                .iter()
                .filter(|p| live::is_gun(state.weapon(p)))
                .map(|p| tore_sim::ai::incoming_fire::Round {
                    id: p.id,
                    owner: p.owner,
                    position: p.position,
                    previous: p.previous,
                    tracer: p.tracer,
                })
                .collect(),
        );
        self.lose_out_of_bounds(&mut state.ledger, |x, z| world.edge_distance_nm(x, z));
        let objects = humans.iter().map(|h| Self::human_object(h)).collect();
        ground::step(self, state, world, &ground);
        strike::step(self, state, &ground);
        gunship::step(self, state, &ground);
        // Aircraft on the researched flight model roll on runways and feel
        // the wind; legacy airborne actors keep the terrain-only surface.
        self.prepare_energy_launches(state);
        let output = self.advance_on_ground(
            objects,
            &mut state.targets,
            &ground,
            &|x, z| world.surface(x, z),
            &|x, z| world.surface_height(x, z),
        )?;
        self.lose_uncredited(&mut state.ledger);
        self.observe_chatter(&output, &humans);
        let positions: Vec<(u32, Vector)> = humans
            .iter()
            .map(|h| (h.slot.id, h.flight.position))
            .collect();
        for event in &output.launches {
            if let Some(weapon) = self.weapons.get(&(event.actor, event.station.0)).cloned() {
                if live::is_gun(&weapon) && self.striker_gun(event) {
                    // A striker's gun run (slice ST-T2).
                    let key = (event.actor, event.station.0);
                    let ordinal = *self.gun_ordinals.entry(key).or_default();
                    let emitted = self.release_strafe(
                        event,
                        &mut state.projectiles,
                        &state.targets,
                        &weapon,
                        ordinal,
                        &ground,
                    );
                    aim_latest(state, emitted, event.target);
                    *self.gun_ordinals.get_mut(&key).unwrap() += u64::from(emitted);
                } else if live::is_gun(&weapon)
                    && self
                        .mission
                        .actor(event.actor)
                        .is_some_and(|a| a.physical_gun(event.station))
                {
                    let key = (event.actor, event.station.0);
                    let ordinal = *self.gun_ordinals.entry(key).or_default();
                    let emitted = self.realise(
                        event,
                        &mut state.projectiles,
                        &weapon,
                        usize::from(event.station.0),
                        &positions,
                        Some(ordinal),
                    );
                    aim_latest(state, emitted, event.target);
                    *self.gun_ordinals.get_mut(&key).unwrap() += u64::from(emitted);
                } else if live::is_gun(&weapon) {
                    let rounds = u16::from(weapon.burst.actual_rounds_per_game.max(1))
                        .saturating_mul(event.projectiles.min(u32::from(u16::MAX)) as u16);
                    let now = self.mission.tick();
                    let physical = u64::from(weapon.burst.actual_rounds_per_game.max(1))
                        * u64::from(weapon.burst.game_rounds_in_burst.max(1));
                    let pending = self
                        .pending_guns
                        .entry((event.actor, event.station.0))
                        .or_insert(PendingGun {
                            groups: VecDeque::new(),
                            next_scaled: now.saturating_mul(physical),
                            ordinal: 0,
                        });
                    if pending.groups.is_empty() {
                        pending.next_scaled = pending.next_scaled.max(now.saturating_mul(physical));
                    }
                    let queued: usize = pending
                        .groups
                        .iter()
                        .map(|(_, remaining)| usize::from(*remaining))
                        .sum();
                    let accepted = usize::from(rounds).min(MAX_PROJECTILES.saturating_sub(queued));
                    self.dropped_launches += u32::from(rounds) - accepted as u32;
                    if accepted > 0 {
                        pending.groups.push_back((event.target, accepted as u16));
                    }
                } else {
                    self.release(event, state, &weapon, &positions, &ground);
                }
            } else {
                self.dropped_launches += event.projectiles;
            }
        }
        let now = self.mission.tick();
        let keys: Vec<_> = self.pending_guns.keys().copied().collect();
        for key @ (actor, station) in keys {
            let Some(weapon) = self.weapons.get(&key).cloned() else {
                continue;
            };
            let physical = u64::from(weapon.burst.actual_rounds_per_game.max(1))
                * u64::from(weapon.burst.game_rounds_in_burst.max(1));
            let Some(mut pending) = self.pending_guns.remove(&key) else {
                continue;
            };
            if self.mission.actor(actor).is_none_or(|actor| !actor.alive()) {
                self.dropped_launches += pending
                    .groups
                    .iter()
                    .map(|(_, remaining)| u32::from(*remaining))
                    .sum::<u32>();
                pending.groups.clear();
            }
            if let Some((target, remaining)) = pending.groups.front_mut()
                && now.saturating_mul(physical) >= pending.next_scaled
            {
                let event = LaunchEvent {
                    actor,
                    station: tore_sim::ai::weapon_service::StationId(station),
                    target: *target,
                    request_id: RequestId(0),
                    projectiles: 1,
                };
                let emitted = self.realise(
                    &event,
                    &mut state.projectiles,
                    &weapon,
                    usize::from(station),
                    &positions,
                    Some(pending.ordinal),
                );
                aim_latest(state, emitted, *target);
                *remaining -= 1;
                pending.ordinal = pending.ordinal.wrapping_add(1);
                pending.next_scaled = pending
                    .next_scaled
                    .saturating_add(u64::from(weapon.burst.game_burst_t.max(1)).saturating_mul(30));
            }
            if pending
                .groups
                .front()
                .is_some_and(|(_, remaining)| *remaining == 0)
            {
                pending.groups.pop_front();
            }
            self.pending_guns.insert(key, pending);
        }
        for event in &output.devices {
            self.realise_device(event, state)?;
        }
        jettison::apply(self, state, &output.jettison)?;
        state.set_actor_supports(self.actor_supports());
        if state.weapon_rules == Rules::Compatibility {
            // Each round's seeker comes from the weapon it carries: its own
            // record, or a station of its owner's ownship.
            self.report_threats(&state.projectiles, |projectile| {
                match state.weapon(projectile).seeker.signature {
                    2 => Some(SeekerClass::Infrared),
                    3 => Some(SeekerClass::Radar),
                    _ => None,
                }
            });
        } else {
            // Diagnostics list perceived incoming threats only. No launch
            // event is broadcast to an aircraft that cannot detect the missile.
            self.threat_reports = self
                .mission
                .actors()
                .iter()
                .flat_map(|actor| {
                    actor
                        .missile_threats()
                        .filter(|record| record.targeting_receiver)
                        .map(move |record| (actor.id(), record.missile_id))
                })
                .take(64)
                .collect();
        }
        self.report_perceived_attacks(state, &humans, &ground);
        self.last_output = output;
        Ok(())
    }

    /// The AI aircraft's fire-control answers for the missile step: each live
    /// actor's observation of its target, its sensors' support and its radar.
    /// The step hands them to combat; the surface tick merges its own with
    /// these in one `set_actor_supports` call
    /// (docs/spec/surface-defenses.md, "SAM missiles").
    pub fn actor_supports(&self) -> impl Iterator<Item = live::ActorSupport> + '_ {
        self.mission
            .actors()
            .iter()
            .filter(|a| a.alive())
            .map(|actor| {
                let observation = actor
                    .controller()
                    .target()
                    .and_then(|id| {
                        actor
                            .awareness()
                            .current_observations()
                            .find(|record| record.target.id == id)
                    })
                    .map(|record| {
                        let delta = missiles::sub(record.target.position, actor.flight().position);
                        seeker::Observation {
                            id: record.target.id,
                            position: record.target.position,
                            velocity: record.velocity,
                            quality: 1.0,
                            off_axis: 0.0,
                            range: missiles::length(delta),
                        }
                    });
                live::ActorSupport {
                    owner: actor.id(),
                    supported: observation
                        .is_some_and(|o| actor.sensors().is_some_and(|s| s.supports(o.id))),
                    observation,
                    radar_position: actor.flight().position,
                    radar_emitting: actor.flight().radar
                        && actor.sensors().is_some_and(|s| {
                            matches!(s.mode(), Some(sensors::Mode::Rws | sensors::Mode::Tws))
                        }),
                }
            })
    }

    /// Report observable attacks, never an opponent's private target choice.
    fn report_perceived_attacks(
        &mut self,
        state: &live::State,
        humans: &[&HumanAircraft],
        ground: &dyn Fn(f64, f64) -> f64,
    ) {
        let attacks = self.perceived_attacks(state, humans, ground);
        // Every test that steps the AI checks the reordered search against
        // the original.
        #[cfg(test)]
        assert_eq!(
            format!("{attacks:?}"),
            format!(
                "{:?}",
                self.perceived_attacks_reference(state, humans, ground)
            ),
            "perceived attacks differ from the original search"
        );
        for (receiver, report, bearing, event_id) in attacks {
            self.mission
                .report_attack_evidence(receiver, report, bearing, event_id);
        }
    }

    /// The attack evidence each receiver perceives this tick, in delivery
    /// order: (receiver, report, world bearing, event id).
    fn perceived_attacks(
        &self,
        state: &live::State,
        humans: &[&HumanAircraft],
        ground: &dyn Fn(f64, f64) -> f64,
    ) -> Vec<PerceivedAttack> {
        use tore_sim::ai::{awareness, engagement::ThreatReport};
        use tore_sim::combat::threats::EvidenceSource;
        // The human-flown aircraft that combat keeps an ownship for, in id
        // order: each one's own records (RWR, emitters, sensor picture) reach
        // the AI's attack evidence for that aircraft alone.
        let combats: Vec<(&HumanAircraft, &live::Ownship)> = humans
            .iter()
            .copied()
            .filter_map(|h| state.ownship(h.slot.id).map(|own| (h, own)))
            .collect();
        let mut reports = Vec::new();
        // A human-flown aircraft's RWR may identify a supporting source only
        // by a unique independently observed hostile emitter at the received
        // bearing. The records belong to one aircraft, so each is delivered
        // to that aircraft and no other.
        for (human, own, record) in combats.iter().flat_map(|(human, own)| {
            own.missile_threats
                .records()
                .filter(|r| r.targeting_receiver && !r.stale)
                .map(move |record| (*human, *own, record))
        }) {
            let attacker_id = if record.source == EvidenceSource::ElectronicSupported {
                record.radar_bearing_deg.and_then(|bearing| {
                    let mut matches = own.emitters.iter().filter(|emitter| {
                        self.slot(emitter.id)
                            .is_some_and(|slot| slot.side != human.slot.side)
                            && own.sensors.observation(emitter.id).is_some()
                            && ((emitter.bearing_rad.to_degrees() - bearing + 180.)
                                .rem_euclid(360.)
                                - 180.)
                                .abs()
                                <= 2.
                    });
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first.id)
                })
            } else {
                None
            };
            reports.push((
                human.slot.id,
                ThreatReport {
                    attacker_id,
                    defended_id: human.slot.id,
                },
                Some(
                    (human.flight.yaw.to_degrees()
                        + record.radar_bearing_deg.unwrap_or(record.bearing_deg))
                    .rem_euclid(360.),
                ),
                Some(record.missile_id),
            ));
        }
        // A fresh, visibly departing missile or tracer can reveal its shooter
        // only when that aircraft is independently observed. Hidden projectile
        // target IDs do not participate in this association.
        //
        // The receivers, each looked up once: a human with its ownship, or
        // an AI actor. A human without an ownship has no records to perceive
        // a shot with, so it is not a receiver here.
        let receivers: Vec<Receiver<'_, '_>> = combats
            .iter()
            .map(|&(h, own)| (h.slot.id, Some((h, own)), self.mission.actor(h.slot.id)))
            .chain(
                self.mission
                    .actors()
                    .iter()
                    .filter(|a| a.alive() && !a.is_dummy())
                    .map(|a| {
                        // An id a human also flies reads as that human.
                        let human = combats.iter().copied().find(|(h, _)| h.slot.id == a.id());
                        (a.id(), human, self.mission.actor(a.id()))
                    }),
            )
            .collect();
        // A sight line wholly above the highest ground is clear without
        // sampling; the answer is the samples' either way.
        let ceiling = self.ground_ceiling.as_deref();
        let terrain_clear = |from, to| {
            ceiling.is_some_and(|c| c.clear_above(from, to)) || terrain_visible(from, to, ground)
        };
        for projectile in state
            .projectiles
            .iter()
            .filter(|p| p.age <= 30 && p.threatening())
        {
            let weapon = state.weapon(projectile);
            let gun = live::is_gun(weapon);
            if gun && !projectile.tracer {
                continue;
            }
            for &(receiver, human, actor) in &receivers {
                if receiver == projectile.owner {
                    continue;
                }
                // `actor` is the mission's actor of the receiver's id, if any:
                // for a human only its lookout and its incoming fire are read.
                let (position, velocity, heading, pitch, skill) = if let Some((human, _)) = human {
                    let player = human.flight;
                    (
                        player.position,
                        player.velocity,
                        player.yaw.to_degrees(),
                        player.pitch.to_degrees(),
                        tore_sim::ai::Experience::Ace,
                    )
                } else {
                    let actor = actor.unwrap();
                    (
                        actor.flight().position,
                        actor.flight().velocity,
                        actor.flight().yaw.to_degrees(),
                        actor.flight().pitch.to_degrees(),
                        actor.controller().experience().level,
                    )
                };
                // Range and the lookout's cones before the ground under the
                // sight line, as the result needs.
                let lookout = actor.and_then(AiActor::lookout);
                let pilot_sees = |point| {
                    let clear = || terrain_clear(position, point);
                    if let Some(lookout) = lookout {
                        lookout.check_with(skill, point, None, clear)
                            == awareness::VisualResult::Visible
                    } else {
                        awareness::visual_eligible(
                            skill, position, heading, pitch, point, None, true,
                        ) && clear()
                    }
                };
                if !pilot_sees(projectile.position) {
                    continue;
                }
                let (possible_shooters, incoming) = if let Some((human, own)) = human {
                    (
                        state
                            .targets
                            .iter()
                            .filter(|target| {
                                self.slot(target.id)
                                    .is_some_and(|slot| slot.side != human.slot.side)
                            })
                            .filter_map(|target| {
                                own.sensors
                                    .observation(target.id)
                                    .map(|o| (target.id, o.position))
                            })
                            .collect::<Vec<_>>(),
                        own.missile_threats.records().any(|r| {
                            r.missile_id == projectile.id && r.targeting_receiver && !r.stale
                        }),
                    )
                } else {
                    let actor = actor.unwrap();
                    (
                        actor
                            .awareness()
                            .current_observations()
                            .filter(|o| o.target.side != actor.identity().side)
                            .map(|o| (o.target.id, o.target.position))
                            .collect::<Vec<_>>(),
                        actor.missile_threats().any(|r| {
                            r.missile_id == projectile.id && r.targeting_receiver && !r.stale
                        }),
                    )
                };
                let mut launch_sources = possible_shooters.into_iter().filter(|(_, shooter)| {
                    missiles::length(missiles::sub(*shooter, projectile.previous)) <= 1000.
                        && pilot_sees(*shooter)
                });
                let Some((shooter, _)) = launch_sources.next() else {
                    continue;
                };
                if launch_sources.next().is_some() {
                    continue;
                }
                let gun_incoming = if gun && !humans.iter().any(|h| h.slot.id == receiver) {
                    actor
                        .and_then(AiActor::incoming_fire_cue)
                        .is_some_and(|cue| {
                            cue.round == Some(projectile.id)
                                && cue.observed_tick + 1 == self.mission.tick()
                        })
                } else if gun {
                    let delta = missiles::sub(projectile.position, position);
                    let movement = std::array::from_fn::<_, 3, _>(|i| {
                        (projectile.position[i] - projectile.previous[i]) * 120. - velocity[i]
                    });
                    let vv = tore_sim::attitude::dot(movement, movement);
                    let time = if vv > 0. {
                        -tore_sim::attitude::dot(delta, movement) / vv
                    } else {
                        -1.
                    };
                    let closest = std::array::from_fn::<_, 3, _>(|i| delta[i] + movement[i] * time);
                    (0. ..=15.).contains(&time) && missiles::length(closest) <= 1000.
                } else {
                    false
                };
                if !(incoming || gun_incoming) {
                    continue;
                }
                let delta = missiles::sub(projectile.position, position);
                reports.push((
                    receiver,
                    ThreatReport {
                        attacker_id: Some(shooter),
                        defended_id: receiver,
                    },
                    Some(delta[0].atan2(delta[2]).to_degrees().rem_euclid(360.)),
                    Some(projectile.id),
                ));
            }
        }
        reports
    }

    /// [`advance_on_surface`](Self::advance_on_surface) over terrain only,
    /// as the headless tests drive it.
    #[cfg(test)]
    pub fn advance(
        &mut self,
        player: WorldObject,
        targets: &mut [live::Target],
        ground: &(dyn Fn(f64, f64) -> f64 + Sync),
    ) -> WorldResult<tore_sim::ai::mission::MissionOutput> {
        self.advance_on_surface(vec![player], targets, ground, &|x, z| {
            tore_sim::research::Surface::terrain(ground(x, z))
        })
    }

    /// The AI half of one tick, with the combat world reduced to its target
    /// rows: damage in, one world snapshot, one mission step, pose out,
    /// activity line. `humans` is one world object per human-flown aircraft,
    /// in id order. `surface` is the host's full surface query, so runways
    /// are solid for aircraft that start or land on them; `terrain` is the
    /// plain terrain height that legacy-adapter aircraft keep using.
    pub fn advance_on_surface(
        &mut self,
        humans: Vec<WorldObject>,
        targets: &mut [live::Target],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> tore_sim::research::Surface + Sync),
    ) -> WorldResult<tore_sim::ai::mission::MissionOutput> {
        self.advance_on_ground(humans, targets, terrain, surface, &|x, z| {
            surface(x, z).height
        })
    }

    /// [`Self::advance_on_surface`] with the host's height-only query,
    /// exactly `surface(x, z).height` ([`Terrain::surface_height`]).
    pub fn advance_on_ground(
        &mut self,
        humans: Vec<WorldObject>,
        targets: &mut [live::Target],
        terrain: &(dyn Fn(f64, f64) -> f64 + Sync),
        surface: &(dyn Fn(f64, f64) -> tore_sim::research::Surface + Sync),
        height: &(dyn Fn(f64, f64) -> f64 + Sync),
    ) -> WorldResult<tore_sim::ai::mission::MissionOutput> {
        // Each target row and AI actor by id, found once for the walks below
        // (no row or actor joins or leaves until the mission step).
        let rows = Rows::of(targets, self.slots.len());
        self.mirror_damage_in_with(targets, &rows);
        let escaped: Vec<_> = self
            .mission
            .actors()
            .iter()
            .filter(|a| a.flight().escape.is_some())
            .map(AiActor::id)
            .collect();

        let objects = self.snapshot_with(humans, targets, &rows);
        // `fitted`: `TimeOfDay` is an opaque host clock the AI only orders
        // against a mission hold time, so the mission tick is used directly. It
        // is monotonic and deterministic, which is all the ordering needs.
        let now = TimeOfDay(self.mission.tick());
        let output = self
            .mission
            .step_on_ground(&objects, terrain, surface, height, now)
            .map_err(|e| e.to_string())?;

        let places = self.mission.actor_places();
        for slot in &self.slots {
            let Some(actor) = places.get(slot.id).map(|at| &self.mission.actors()[at]) else {
                continue;
            };
            if actor.flight().escape.is_some() && !escaped.contains(&slot.id) {
                self.ejection_events.push((
                    slot.id,
                    format!("{} pilot ejected", slot.label()),
                    slot.side == launch::Side::Friendly,
                ));
                if let Some(target) = rows.get(targets, slot.id).map(|at| &mut targets[at]) {
                    target.hp = 0;
                    target.radar_emitting = false;
                    target.jammer_active = false;
                }
            }
            // An aircraft whose own flight has ended, in a crash or a fatal
            // system failure such as a dead pilot or failed structure, is
            // lost; combat takes over its wreck and its last attacker gets
            // the kill.
            if actor.flight().crashed
                && let Some(target) = rows.get(targets, slot.id).map(|at| &mut targets[at])
                && target.hp > 0
            {
                // Lost to the aircraft's own structure (overspeed, the map
                // edge): no shooter caused it.
                if actor.flight().systems.structure.cause.is_some() {
                    self.uncredited_losses.insert(slot.id);
                }
                target.hp = 0;
                target.radar_emitting = false;
                target.jammer_active = false;
            }
        }
        self.record_formation_trace();
        self.formation_reports();
        self.mirror_pose_out_with(targets, &rows);
        self.announce(&output.activities);
        Ok(output)
    }

    /// Switch the formation trace on or off. While it is on, every sampled tick
    /// adds its rows to a bounded list that [`take_formation_trace`] drains;
    /// switching it off drops whatever was waiting. The trace only reads the
    /// mission, so it never changes how an aircraft flies.
    ///
    /// [`take_formation_trace`]: Self::take_formation_trace
    pub fn set_formation_trace(&mut self, on: bool) {
        self.formation_trace = on.then(FormationBatch::default);
    }

    /// The rows collected since the last call, in tick order. The app calls
    /// this every tick; with tracing off the batch is empty.
    pub fn take_formation_trace(&mut self) -> FormationBatch {
        self.formation_trace
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    fn record_formation_trace(&mut self) {
        let Some(batch) = self.formation_trace.as_mut() else {
            return;
        };
        let tick = self.mission.tick();
        if !tick.is_multiple_of(FORMATION_TRACE_EVERY) {
            return;
        }
        for actor in self.mission.actors() {
            let Some(trace) = actor.controller().formation_trace() else {
                continue;
            };
            if batch.rows.len() >= FORMATION_TRACE_MAX_ROWS {
                break;
            }
            let state = actor.flight();
            let input = actor.last_input();
            batch.rows.push(FormationRow {
                tick,
                actor: actor.id(),
                trace,
                position: state.position,
                speed: state.speed,
                bank: state.bank,
                g: state.g,
                pitch_input: input.pitch,
                roll_input: input.roll,
                yaw_input: input.yaw,
                throttle: state.throttle,
                afterburner: state.afterburner_active(),
            });
        }
        batch.flush |= tick.is_multiple_of(FORMATION_TRACE_FLUSH_EVERY);
    }

    /// A human-flown aircraft as the AI sees it: an ordinary object on its
    /// side, never a special case in the decision path. The AI never shoots
    /// at a human of its own side and always may at one of the other.
    pub fn human_object(human: &HumanAircraft) -> WorldObject {
        let player = human.flight;
        let id = human.slot.id;
        let player_hp = human.hit_points;
        let on_ground = player.research.as_ref().is_some_and(|r| r.on_ground);
        WorldObject {
            id,
            side: side_of(human.slot.side),
            position: player.position,
            velocity: player.velocity,
            heading_deg: player.yaw.to_degrees(),
            pitch_deg: player.pitch.to_degrees(),
            speed: ScalarSpeed(player.speed),
            maximum_speed: maximum_speed(player),
            is_aircraft: true,
            is_fighter: true,
            infrared_strength: human.signature.infrared.max(0.) / 100.
                * seeker::Heat::Engine {
                    on: player.engine,
                    throttle: player.throttle,
                    afterburner: player.burner,
                }
                .factor(),
            human_controlled: true,
            alive: human.alive(),
            destroyed: player_hp <= 0,
            on_ground,
            observable: Some(
                Observable {
                    id,
                    position: player.position,
                    velocity: player.velocity,
                    basis: Basis::new(player.yaw, player.pitch, player.bank),
                    configuration: sensors::Configuration::carrying(human.stores),
                    signature: human.signature,
                    jammer: human.jammer.clone(),
                    jammer_active: player.jammer && player.engine,
                    radar_emitting: player.radar && player.engine,
                    airborne: true,
                    destroyed: player_hp <= 0,
                }
                .on_ground(on_ground),
            ),
        }
    }

    /// Ejected AI pilots in roster order, each with the aircraft it left.
    pub fn escapees(&self) -> impl Iterator<Item = (u32, &tore_sim::ejection::Escape)> {
        self.mission
            .actors()
            .iter()
            .filter_map(|a| a.flight().escape.as_ref().map(|escape| (a.id(), escape)))
    }

    /// The second crew members of the ejected two-seaters among them, in
    /// roster order, each with the aircraft it left.
    pub fn crew_escapees(&self) -> impl Iterator<Item = (u32, &tore_sim::ejection::Escape)> {
        self.mission.actors().iter().filter_map(|a| {
            a.flight()
                .crew_escape
                .as_ref()
                .map(|escape| (a.id(), escape))
        })
    }

    /// An AI aircraft's flight state, for a development probe that flies it
    /// on a scripted line (`--flak-pass`). The game never calls it.
    pub fn actor_flight_mut(&mut self, id: u32) -> Option<&mut tore_sim::flight::State> {
        self.mission.actor_mut(id).map(|actor| actor.flight_mut())
    }

    /// A missile blast knocks an AI aircraft around, like the player's;
    /// `share` scales the knock (a flak burst's is half).
    pub fn jolt(&mut self, id: u32, from: [f64; 3], strength: f64, share: f64) {
        if let Some(actor) = self.mission.actor_mut(id) {
            actor.flight_mut().jolt_scaled(from, strength, share);
        }
    }

    /// Victim-only evidence from combat, before the AI step. Terrain and
    /// ordinary flight damage must not manufacture a gunfire cue.
    pub fn report_weapon_hits(&mut self, events: &[live::Event]) {
        for event in events {
            if let live::Event::Hit(id) = event
                && let Some(actor) = self.mission.actor_mut(*id)
            {
                actor.report_hit();
            }
        }
    }

    /// Damage and death flow from the combat world into the actors: an actor
    /// whose target row lost hit points is told it was hit, and one whose row
    /// reached zero stops flying.
    /// `rows` holds each target row's place by id.
    fn mirror_damage_in_with(&mut self, targets: &mut [live::Target], rows: &Rows) {
        let places = self.mission.actor_places();
        for slot in &self.slots {
            let Some(target) = rows.get(targets, slot.id).map(|at| &targets[at]) else {
                continue;
            };
            let place = places.get(slot.id);
            let was_alive = place.is_some_and(|at| self.mission.actors()[at].alive());
            if let Some(actor) = place.map(|at| &mut self.mission.actors_mut()[at]) {
                let flight = actor.flight_mut();
                flight.damage_regions = target.localized_damage.fractions(target.initial_hp);
                if target.hp <= 0 {
                    flight.crashed = true;
                    flight.position = target.position;
                    flight.velocity = target.velocity;
                    [flight.yaw, flight.pitch, flight.bank] = target.basis.angles();
                    flight.wreck = target.wreck.clone();
                    if target
                        .wreck
                        .as_ref()
                        .is_some_and(|w| w.phase != tore_sim::wreck::Phase::Falling)
                        || matches!(
                            target.localized_damage.structural_section,
                            Some(live::DamageSection::Nose | live::DamageSection::Cockpit)
                        )
                    {
                        flight.systems.kill_pilot("Pilot killed before escape");
                    }
                }
                actor.flight_mut().damage_fraction = 1.0
                    - (f64::from(target.hp) / f64::from(target.initial_hp.max(1))).clamp(0.0, 1.0);
            }
            let previous = *self.last_hp.entry(slot.id).or_insert(target.hp);
            if target.hp < previous
                && let Some(actor) = place.map(|at| &mut self.mission.actors_mut()[at])
            {
                actor.report_damage();
            }
            self.last_hp.insert(slot.id, target.hp);
            if target.hp <= 0 && was_alive {
                if let Some(actor) = place.map(|at| &mut self.mission.actors_mut()[at]) {
                    actor.set_alive(false);
                }
                for other in self.mission.actors_mut() {
                    other.report_removed(slot.id);
                }
            }
        }
        for target in targets.iter_mut().filter(|t| t.hp > 0) {
            self.deliver_faults(target, places.get(target.id));
        }
    }

    /// Hand an AI aircraft the system faults its hits caused, always as
    /// Realistic damage whatever the player's Damage cheat says. Its flight
    /// systems take each fault, and a hardpoint fault takes out what that
    /// hardpoint carries, as on the player's aircraft.
    /// `place` is the target's actor's place in the mission, if it has one.
    fn deliver_faults(&mut self, target: &mut live::Target, place: Option<usize>) {
        use tore_sim::combat::systems::EcmLoss;
        let Some(actor) = place.map(|at| &mut self.mission.actors_mut()[at]) else {
            return;
        };
        let mut hardpoints = Vec::new();
        {
            let flight = actor.flight_mut();
            for (index, &count) in target.faults.counts.iter().enumerate() {
                while flight.systems.counts[index] < count {
                    flight.systems.hit(index, flight.throttle);
                    hardpoints.extend(index.checked_sub(36));
                }
            }
        }
        let Some(config) = self.configs.get(&target.id) else {
            return;
        };
        let mut equipment = EquipmentFaults::default();
        for h in hardpoints {
            if config.tanks.iter().any(|tank| tank.hardpoint == h)
                && config.external_fuel_lbs[h] > 0.
            {
                actor.flight_mut().systems.fuel.external[h] = 0.;
            } else if let Some(&Some(station)) = config.hardpoint_slots.get(h) {
                let station = station as u8;
                self.damaged_stations.insert((target.id, station));
                for spec in actor.stations_mut() {
                    if spec.station.0 == station {
                        spec.store.inhibited = true;
                    }
                }
            } else if Some(h) == config.radar_hardpoint {
                equipment.radar = true;
            } else if h == config.visual_hardpoint {
                equipment.visual = true;
            } else if Some(h) == config.infrared_hardpoint {
                equipment.infrared = true;
            } else if Some(h) == config.rwr_hardpoint {
                equipment.rwr = true;
            } else if Some(h) == config.ecm_hardpoint {
                let random = &mut self.fault_random;
                let lost = match tore_sim::combat::systems::ecm_loss(&config.ecm, |n| {
                    random.below(u32::from(n)) as u16
                }) {
                    Some(EcmLoss::Everything) => {
                        target.jammer = None;
                        target.jammer_active = false;
                        None
                    }
                    Some(EcmLoss::Chaff) => Some(SeekerClass::Radar),
                    Some(EcmLoss::Flares) => Some(SeekerClass::Infrared),
                    None => continue,
                };
                let dispensers = actor
                    .dispensers()
                    .iter()
                    .map(|d| tore_sim::ai::threat::DispenserStore {
                        class: d.class,
                        count: if lost.is_none_or(|class| class == d.class) {
                            0
                        } else {
                            d.count
                        },
                    })
                    .collect();
                actor.set_dispensers(dispensers);
            }
        }
        actor.fail_equipment(equipment);
    }

    /// One world snapshot: the human-flown aircraft first, in id order, then
    /// every AI aircraft.
    #[cfg(test)]
    fn snapshot(&self, humans: Vec<WorldObject>, targets: &[live::Target]) -> Vec<WorldObject> {
        self.snapshot_with(humans, targets, &Rows::of(targets, self.slots.len()))
    }

    /// [`Self::snapshot`] with each target row's place by id.
    fn snapshot_with(
        &self,
        humans: Vec<WorldObject>,
        targets: &[live::Target],
        rows: &Rows,
    ) -> Vec<WorldObject> {
        let mut objects = Vec::with_capacity(self.slots.len() + humans.len());
        objects.extend(humans);
        let places = self.mission.actor_places();
        for slot in &self.slots {
            let Some(actor) = places.get(slot.id).map(|at| &self.mission.actors()[at]) else {
                continue;
            };
            let f = actor.flight();
            let target = rows.get(targets, slot.id).map(|at| &targets[at]);
            let on_ground = f.research.as_ref().is_some_and(|r| r.on_ground);
            objects.push(WorldObject {
                id: slot.id,
                side: side_of(slot.side),
                position: f.position,
                velocity: f.velocity,
                heading_deg: f.yaw.to_degrees(),
                pitch_deg: f.pitch.to_degrees(),
                speed: ScalarSpeed(f.speed),
                maximum_speed: maximum_speed(f),
                is_aircraft: true,
                is_fighter: true,

                infrared_strength: target.map_or(1., |t| t.signature.infrared.max(0.) / 100.)
                    * aircraft_heat(f, target.map_or(seeker::Heat::Unknown, |t| t.heat)).factor(),
                human_controlled: false,
                alive: actor.alive(),
                destroyed: target.is_some_and(|t| t.hp <= 0),
                on_ground,
                observable: target.map(|t| {
                    Observable {
                        id: t.id,
                        position: f.position,
                        velocity: f.velocity,
                        basis: Basis::new(f.yaw, f.pitch, f.bank),
                        configuration: t.configuration,
                        signature: t.signature,
                        jammer: t.jammer.clone(),
                        jammer_active: t.jammer_active,
                        radar_emitting: f.radar,
                        airborne: t.airborne,
                        destroyed: t.hp <= 0,
                    }
                    .on_ground(on_ground)
                }),
            });
        }
        objects
    }

    /// Each AI aircraft and whether it is in an airfield's takeoff or landing
    /// sequence (the A-G page lists aircraft only then).
    pub fn airfield_attachment(&self) -> Vec<(u32, bool)> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let actor = self.mission.actor(slot.id)?;
                Some((slot.id, actor.alive() && actor.airfield().is_some()))
            })
            .collect()
    }

    /// The AI pose becomes the target pose, so the renderer, the player's
    /// sensors, the missile collision sweep and the damage model all see one
    /// world with one authority per aircraft.
    pub fn mirror_pose_out(&self, targets: &mut [live::Target]) {
        let rows = Rows::of(targets, self.slots.len());
        self.mirror_pose_out_with(targets, &rows);
    }

    /// [`Self::mirror_pose_out`] with each target row's place by id.
    fn mirror_pose_out_with(&self, targets: &mut [live::Target], rows: &Rows) {
        let places = self.mission.actor_places();
        for slot in &self.slots {
            let Some(actor) = places.get(slot.id).map(|at| &self.mission.actors()[at]) else {
                continue;
            };
            let Some(target) = rows.get(targets, slot.id).map(|at| &mut targets[at]) else {
                continue;
            };
            // Combat owns falling wrecks after destruction.
            if !actor.alive() || target.hp <= 0 {
                continue;
            }
            let f = actor.flight();
            target.position = f.position;
            target.velocity = f.velocity;
            target.basis = Basis::new(f.yaw, f.pitch, f.bank);
            target.heat = if self.weapon_rules == Rules::Energy {
                energy_heat(f, target.heat)
            } else {
                aircraft_heat(f, target.heat)
            };
            target.radar_emitting = f.radar;
            target.on_ground = f.research.as_ref().is_some_and(|r| r.on_ground);
            target.wreck_power = f.wreck_power(target.wreck_power.engine_count.max(1));
            target.configuration = sensors::Configuration::carrying(self.external_stores(slot.id));
        }
    }

    /// What AI aircraft `id` carries outside right now, from its combat
    /// configuration and its live station rounds: the count every radar,
    /// jammer comparison and missile seeker reads (docs/radar.md, "External
    /// stores").
    pub fn external_stores(&self, id: u32) -> sensors::Stores {
        let (Some(actor), Some(config)) = (self.mission.actor(id), self.configs.get(&id)) else {
            return sensors::Stores::NONE;
        };
        config.external_stores(
            |index| {
                actor
                    .stations()
                    .iter()
                    .find(|s| usize::from(s.station.0) == index)
                    .map_or(0, |s| match s.rounds() {
                        tore_sim::ai::weapon_service::Rounds::Finite(n) => n,
                        tore_sim::ai::weapon_service::Rounds::Unlimited => {
                            let station = &config.stations[index];
                            u32::from(station.count) * live::pod_rounds(&station.weapon)
                        }
                    })
            },
            // An AI aircraft never drops a gun pod.
            |_| true,
        )
    }

    /// Turn one AI launch into a real projectile flown by the existing combat
    /// code. Live guns debit only after all release checks, once per emitted
    /// round. Missiles and legacy synthetic groups were already debited.
    fn realise(
        &mut self,
        event: &LaunchEvent,
        projectiles: &mut Vec<live::Projectile>,
        weapon: &tore_formats::weapons::Weapon,
        station: usize,
        humans: &[(u32, Vector)],
        gun_ordinal: Option<u64>,
    ) -> u32 {
        let Some(actor) = self.mission.actor(event.actor) else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let gun = live::is_gun(weapon);
        let physical_gun = gun && actor.physical_gun(event.station);
        if physical_gun && !actor.alive() {
            return 0;
        }
        let flight = actor.flight();
        let basis = Basis::new(flight.yaw, flight.pitch, flight.bank);
        let mount = actor
            .stations()
            .iter()
            .find(|s| s.station == event.station)
            .map_or([0.; 3], |s| s.mount);
        // A store leaves its own side of its station, as a human's does
        // (B3); an aircraft with no combat configuration keeps its centre.
        let origin = match if gun {
            Some(mount)
        } else {
            self.release_mount(event.actor, station)
        } {
            Some(mount) => std::array::from_fn(|i| {
                flight.position[i]
                    + basis.right[i] * mount[0]
                    + basis.up[i] * mount[1]
                    + basis.forward[i] * mount[2]
            }),
            None => flight.position,
        };
        if physical_gun {
            let Some(target) = actor.gun_target().filter(|t| t.id == event.target) else {
                return 0;
            };
            if !tore_sim::ai::gunnery::solve(
                weapon,
                &crate::combat::launcher(flight),
                mount,
                target,
            )
            .is_some_and(|s| s.aligned)
            {
                return 0;
            }
        }
        let launch_velocity = flight.velocity;
        let observed = actor
            .awareness()
            .current_observations()
            .find(|record| record.target.id == event.target)
            .copied();
        let aim = if let Some(observation) = observed {
            observation.target.position
        } else if actor.sensors().is_none() {
            // Sensorless synthetic fixtures explicitly supply permitted world
            // targets. Live aircraft never use this fallback.
            if let Some(&(_, position)) = humans.iter().find(|(id, _)| *id == event.target) {
                position
            } else if let Some(other) = self.mission.actor(event.target) {
                other.flight().position
            } else {
                self.dropped_launches += event.projectiles;
                return 0;
            }
        } else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let direction = if gun {
            basis.forward
        } else {
            unit([aim[0] - origin[0], aim[1] - origin[1], aim[2] - origin[2]])
        };
        if direction.iter().any(|v| !v.is_finite()) {
            self.dropped_launches += event.projectiles;
            return 0;
        }
        let Ok(speed) = launch_speed(&weapon.movement, (actor.flight().speed * 256.) as i32) else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let profile = (self.weapon_rules.uses_seeker())
            .then(|| missiles::Profile::for_weapon(weapon))
            .flatten();
        let guidance = profile.map(|profile| {
            Flight::from_supported_launch(
                profile,
                LaunchMode::Cued,
                seeker::Observation {
                    id: event.target,
                    position: aim,
                    velocity: observed.map_or([0.; 3], |record| record.velocity),
                    quality: 1.0,
                    off_axis: 0.0,
                    range: missiles::length(missiles::sub(aim, origin)),
                },
                origin,
            )
        });
        // Compatibility keeps its existing steering; reviewed profiles use
        // the same owner-aware seeker/propulsion lifecycle as player shots.
        let launched = (self.mission.tick() / 30) as u16;
        let incoming = humans
            .iter()
            .any(|(id, _)| *id == event.target)
            .then_some(event.target);
        let mut emitted = 0;
        for _ in 0..event.projectiles {
            if projectiles.len() >= MAX_PROJECTILES {
                self.dropped_launches += 1;
                continue;
            }
            if physical_gun
                && !self
                    .mission
                    .actor_mut(event.actor)
                    .is_some_and(|a| a.debit_gun_round(event.station))
            {
                break;
            }
            let id = self.next_projectile_id;
            self.next_projectile_id = self.next_projectile_id.wrapping_add(1);
            projectiles.push(live::Projectile {
                id,
                // Attribute the round to the AI actor that fired it, so an
                // AI kill never credits the player's score.
                owner: event.actor,
                weapon: Some(weapon.clone()),
                guidance: guidance.clone(),
                motion: profile.map(|_| {
                    Motion::launch_rules(weapon, launch_velocity, origin[1], self.weapon_rules)
                }),
                guidance_ticks: profile.map(|p| p.guidance_ticks),
                age: 0,
                incoming,
                station,
                position: origin,
                previous: origin,
                direction,
                speed_f8: speed * 256,
                launched_t: launched,
                target: (weapon.seeker.signature != 0).then_some(event.target),
                fall: FallState::default(),
                gun_round: gun_ordinal.map(|ordinal| {
                    (ordinal % u64::from(weapon.burst.actual_rounds_per_game.max(1))) as u8
                }),
                tracer: gun_ordinal.is_some_and(|ordinal| ordinal.is_multiple_of(3)),
            });
            self.ai_shots.insert(id, event.actor);
            self.realised_launches += 1;
            emitted += 1;
        }
        emitted
    }

    /// Missile kinematics are supplied by combat's bounded prediction service.
    /// This changes neither the defense controller nor the dispense schedule.
    fn prepare_energy_launches(&mut self, state: &mut live::State) {
        for actor in self.mission.actors_mut() {
            let id = actor.id();
            let launcher = crate::combat::launcher(actor.flight());
            let observed = actor
                .current_target()
                .and_then(|id| actor.sensors()?.observation(id))
                .map(|o| seeker::Observation {
                    id: o.id,
                    position: o.position,
                    velocity: o.velocity,
                    quality: 1.,
                    range: missiles::length(missiles::sub(o.position, launcher.position)),
                    off_axis: 0.,
                });
            for station in actor.stations_mut() {
                let Some(w) = self.weapons.get(&(id, station.station.0)) else {
                    continue;
                };
                if !station.guided || w.movement.fuel_t <= w.movement.ignite_t {
                    continue;
                }
                let mut mount = launcher;
                mount.position = std::array::from_fn(|i| {
                    launcher.position[i]
                        + launcher.basis.right[i] * station.mount[0]
                        + launcher.basis.up[i] * station.mount[1]
                        + launcher.basis.forward[i] * station.mount[2]
                });
                if state.weapon_rules == Rules::Energy {
                    station.energy_target = Some(None);
                    station.maximum_range_ft = None;
                    if let Some(zone) = &mut station.employment_zone {
                        zone.maximum_range = i32::MAX;
                    }
                    if let Some(o) = observed {
                        state.request_energy_solution(
                            id,
                            usize::from(station.station.0),
                            w,
                            mount,
                            o,
                        );
                        if state
                            .energy_solution(id, usize::from(station.station.0), w, mount, o)
                            .is_some()
                        {
                            station.energy_target = Some(Some(o.id));
                        }
                    }
                } else if station.energy_target.take().is_some() {
                    station.maximum_range_ft = Some(f64::from(w.seeker.zones[1].maximum_range));
                    if let Some(zone) = &mut station.employment_zone {
                        zone.maximum_range = w.seeker.zones[1].maximum_range;
                    }
                }
            }
            if state.weapon_rules == Rules::Energy
                && let Some(config) = self.configs.get(&id)
                && let Some(cockpit) = state.strike_cockpit(id)
                && cockpit.armed
                && let Some(w) = config.stations.get(cockpit.selected).map(|s| &s.weapon)
                && w.movement.fuel_t > w.movement.ignite_t
                && tore_sim::combat::guided::kind(w).is_some()
            {
                let station = cockpit.selected;
                // The already-owned laser/sight point, not a hidden aircraft.
                let kind = tore_sim::combat::guided::kind(w).unwrap();
                let point = if kind == tore_sim::combat::missiles::Guidance::Laser {
                    cockpit.laser.point
                } else {
                    match cockpit.target {
                        Some(live::StrikeTarget::Point(point)) => Some(point),
                        Some(live::StrikeTarget::Object(target)) => state
                            .targets
                            .iter()
                            .find(|t| t.id == target)
                            .map(|t| t.position),
                        None => None,
                    }
                };
                if let Some(point) = point {
                    state.request_energy_solution(
                        id,
                        station,
                        w,
                        launcher,
                        seeker::Observation {
                            id: u32::MAX,
                            position: point,
                            velocity: state.guided_point_velocity(
                                id,
                                tore_sim::combat::guided::kind(w).unwrap(),
                            ),
                            quality: 1.,
                            range: missiles::length(missiles::sub(point, launcher.position)),
                            off_axis: 0.,
                        },
                    );
                }
            }
        }
    }

    fn realise_device(
        &mut self,
        event: &tore_sim::ai::mission::DeviceEvent,
        state: &mut live::State,
    ) -> WorldResult<()> {
        use tore_sim::ai::threat::{self, DecoyOutcome, GuidingMissile};
        let Some(actor) = self.mission.actor(event.actor) else {
            return Ok(());
        };
        let effectiveness = self
            .device_effectiveness
            .get(&event.actor)
            .copied()
            .unwrap_or((100, 100));
        // Imported records can hold more than 100; like the player's own
        // dispensers, a percentage above 100 counts as 100 (certain).
        let effectiveness = match event.class {
            SeekerClass::Infrared => effectiveness.0,
            SeekerClass::Radar => effectiveness.1,
        }
        .min(100);
        let flight = actor.flight();
        let release = tore_sim::combat::countermeasures::Release {
            position: flight.position,
            velocity: flight.velocity,
            basis: Basis::new(flight.yaw, flight.pitch, flight.bank),
        };
        for _ in 0..event.released {
            state.device_released(
                release,
                match event.class {
                    SeekerClass::Infrared => live::EffectKind::Flare,
                    SeekerClass::Radar => live::EffectKind::Chaff,
                },
                event.actor,
            );
            // The number this device was given, which each roll below is
            // against.
            let device = state.devices.released();
            if state.weapon_rules == Rules::Energy {
                state.energy_devices.effectiveness(
                    event.actor,
                    match event.class {
                        SeekerClass::Infrared => missiles::decoys::Kind::Flare,
                        SeekerClass::Radar => missiles::decoys::Kind::Chaff,
                    },
                    effectiveness,
                );
                continue;
            }
            // The seeker and decoy chance of each round, from the weapon it
            // carries, so no particular ownship is needed.
            let seekers: Vec<(u8, u8)> = state
                .projectiles
                .iter()
                .map(|p| {
                    let seeker = &state.weapon(p).seeker;
                    (seeker.signature, seeker.chaff_flare_chance)
                })
                .collect();
            for (projectile, (signature, chaff_flare_chance)) in
                state.projectiles.iter_mut().zip(seekers)
            {
                let class = match signature {
                    2 => SeekerClass::Infrared,
                    3 => SeekerClass::Radar,
                    _ => continue,
                };
                let missile = GuidingMissile {
                    seeker: class,
                    guiding_on_releaser: projectile.target == Some(event.actor)
                        && projectile.guidance.as_ref().is_none_or(|flight| {
                            flight.enabled
                                && flight.seeker.acquired
                                && flight.seeker.observation.is_some()
                        }),
                    decoy_susceptibility_percent: chaff_flare_chance.min(100),
                };
                let decoy = threat::decoy_missile(
                    &missile,
                    event.class,
                    effectiveness,
                    &mut self.device_random,
                )
                .map_err(|e| e.to_string())?;
                if decoy != DecoyOutcome::NotEligible {
                    // Write-only: the roll this missile just made.
                    self.decoy_rolls.push(DecoyRoll {
                        projectile: projectile.id,
                        releaser: event.actor,
                        class: event.class,
                        device,
                        susceptibility: missile.decoy_susceptibility_percent,
                        effectiveness,
                        draw: self.device_random.log().draws().last().copied(),
                        decoyed: decoy == DecoyOutcome::Decoyed,
                    });
                }
                if decoy == DecoyOutcome::Decoyed {
                    state
                        .ledger
                        .resolve(projectile.id, tore_sim::combat::ledger::Resolution::Spoofed);
                    projectile.target = None;
                    projectile.guidance = None;
                    // Fitted: coast after decoy, using the existing record lifetime.
                }
            }
        }
        Ok(())
    }

    /// B47: every missile in the world is reported once, to its target only.
    /// Wingmen are not told, and neither is anyone else: only the aircraft the
    /// shot is actually aimed at receives a report.
    fn report_threats(
        &mut self,
        projectiles: &[live::Projectile],
        seeker_of: impl Fn(&live::Projectile) -> Option<SeekerClass>,
    ) {
        let mut reports: Vec<(u32, ThreatReport)> = Vec::new();
        for projectile in projectiles {
            if !self.seen_projectiles.insert(projectile.id) {
                continue;
            }
            let Some(target) = projectile.target else {
                continue;
            };
            if self.humans.iter().any(|h| h.id == target) {
                continue;
            }
            let Some(slot) = self.slot(target) else {
                continue;
            };
            let Some(actor) = self.mission.actor(target) else {
                continue;
            };
            let Some(seeker) = seeker_of(projectile) else {
                continue;
            };
            let launcher_id = self
                .ai_shots
                .get(&projectile.id)
                .copied()
                .unwrap_or(projectile.owner);
            let launcher_side = match self.humans.iter().find(|h| h.id == launcher_id) {
                Some(human) => Some(human.side),
                None => self.slot(launcher_id).map(|s| s.side),
            };
            let position = actor.flight().position;
            let d = [
                position[0] - projectile.position[0],
                position[1] - projectile.position[1],
                position[2] - projectile.position[2],
            ];
            reports.push((
                target,
                ThreatReport {
                    missile_id: projectile.id,
                    seeker,
                    launcher_id,
                    launcher_same_side: launcher_side
                        .is_some_and(|side| side_of(side) == side_of(slot.side)),
                    distance_at_launch_ft: (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
                    launch_tick: self.mission.tick(),
                },
            ));
        }
        for (id, report) in reports {
            if let Some(actor) = self.mission.actor_mut(id) {
                self.threat_reports.push((id, report.missile_id));
                actor.report_threat(report);
            }
        }
        // Bounded history: this is a report channel, not a log.
        const KEEP: usize = 64;
        if self.threat_reports.len() > KEEP {
            let drop = self.threat_reports.len() - KEEP;
            self.threat_reports.drain(..drop);
        }
        // The seen list only has to outlive the projectiles themselves.
        if self.seen_projectiles.len() > MAX_PROJECTILES * 4 {
            let live: BTreeSet<u32> = projectiles.iter().map(|p| p.id).collect();
            self.seen_projectiles.retain(|id| live.contains(id));
            self.ai_shots.retain(|id, _| live.contains(id));
        }
    }

    /// `opinionated` (agent decision, 2026-09-17): the single in-flight message
    /// bar carries AI activity changes. Only a change into an activity
    /// [`worth_announcing`] posts, and at most one line per
    /// [`MESSAGE_INTERVAL_TICKS`].
    ///
    /// A held change of the aircraft the line already names is posted once
    /// the interval allows (agent decision, 2026-09-28, overnight bug battery),
    /// so the line never keeps saying "Defending" about an aircraft that has
    /// since been destroyed.
    fn announce(&mut self, activities: &[(u32, Activity)]) {
        let tick = self.mission.tick();
        let open = |last: u64| last == 0 || tick >= last + MESSAGE_INTERVAL_TICKS;
        for (id, activity) in activities {
            let changed = self.last_activity.insert(*id, *activity) != Some(*activity);
            if !changed || !worth_announcing(*activity) {
                continue;
            }
            if !open(self.last_message_tick) {
                let remaining = self.last_message_tick + MESSAGE_INTERVAL_TICKS - tick;
                self.activity_held(*id, *activity, remaining);
                if self.shown_actor == Some(*id) {
                    self.stale_line = Some((*id, *activity));
                }
                continue;
            }
            self.post_activity(*id, *activity, tick);
        }
        if open(self.last_message_tick)
            && let Some((id, activity)) = self.stale_line.take()
            && self.last_activity.get(&id) == Some(&activity)
        {
            self.post_activity(id, activity, tick);
        }
    }

    fn post_activity(&mut self, id: u32, activity: Activity, tick: u64) {
        let Some(slot) = self.slot(id) else { return };
        let message = format!("{}: {}", slot.label(), activity.label());
        self.activity_posted(id, activity);
        self.pending_message = Some(message);
        self.last_message_tick = tick.max(1);
        self.shown_actor = Some(id);
        if self.stale_line.is_some_and(|(stale, _)| stale != id) {
            self.stale_line = None;
        }
    }

    /// Records the aircraft lost this tick to overspeed
    /// as lost without credit, exactly like the map edge: whoever shot at them
    /// earlier is not rewarded for a crash it did not cause (requested by John,
    /// 2026-09-29). The debrief still counts them as lost aircraft. Returns the
    /// ids recorded.
    pub fn lose_uncredited(&mut self, ledger: &mut tore_sim::combat::ledger::Ledger) -> Vec<u32> {
        let lost: Vec<u32> = std::mem::take(&mut self.uncredited_losses)
            .into_iter()
            .collect();
        for id in &lost {
            ledger.lose_without_credit(*id);
        }
        lost
    }

    /// An AI aircraft 105 nautical miles beyond the edge of the map is lost,
    /// with no warning and no shooter to credit, and counts as a crash for the
    /// debrief and the objectives (requested by John, 2026-09-29). Returns the
    /// ids lost this call. `distance_nm` is the distance beyond the map edge.
    pub fn lose_out_of_bounds(
        &mut self,
        ledger: &mut tore_sim::combat::ledger::Ledger,
        distance_nm: impl Fn(f64, f64) -> f64,
    ) -> Vec<u32> {
        let lost: Vec<u32> = self
            .mission
            .actors()
            .iter()
            .filter(|a| a.alive())
            .filter(|a| {
                let [x, _, z] = a.flight().position;
                distance_nm(x, z) >= crate::terrain::EDGE_DESTROY_NM
            })
            .map(AiActor::id)
            .collect();
        for id in &lost {
            if let Some(actor) = self.mission.actor_mut(*id) {
                let flight = actor.flight_mut();
                flight
                    .systems
                    .destroy(tore_sim::aircraft_systems::LossCause::OutOfBounds);
                flight.crashed = true;
            }
            ledger.lose_without_credit(*id);
        }
        lost
    }

    /// A compact deterministic line per actor, for the headless probe.
    pub fn probe_lines(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let actor = self.mission.actor(slot.id)?;
                let f = actor.flight();
                Some(format!(
                    "actor={} {} {:?} activity={} alive={} rounds={} x={:.1} y={:.1} z={:.1} hdg={:.1}{}",
                    slot.id,
                    slot.label(),
                    slot.aircraft,
                    actor.activity().label(),
                    actor.alive(),
                    actor.rounds_remaining(),
                    f.position[0],
                    f.position[1],
                    f.position[2],
                    f.yaw.to_degrees(),
                    // Only when lost to overspeed or the map edge.
                    f.systems
                        .structure
                        .cause
                        .map_or_else(String::new, |c| format!(" cause={}", c.label())),
                ))
            })
            .collect()
    }

    /// Every actor's position, for determinism and motion tests.
    pub fn positions(&self) -> Vec<Vector> {
        self.slots
            .iter()
            .filter_map(|s| self.mission.actor(s.id).map(|a| a.flight().position))
            .collect()
    }
}

#[cfg(test)]
#[path = "ai_wings/perceived_reference.rs"]
mod perceived_reference;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::terrain as world;
    pub(crate) use crate::test_support::{aircraft, combat_fixture, payload, spawned, target};
    use tore_sim::ai::{
        Experience,
        controller::TargetView,
        engagement::{GroupObjective, Policy, Priority},
        experience::{EnemySkillOverride, ExperienceOrigin},
        launch::{WingId, WingSelection, resolve_wings},
    };
    use tore_sim::combat::live::OwnshipInput;

    #[test]
    fn six_full_wings_have_separate_delta_formations_and_29_ai_members() {
        let selections: Vec<_> = [launch::Side::Friendly, launch::Side::Enemy]
            .into_iter()
            .flat_map(|side| {
                (0..3).map(move |index| WingSelection {
                    wing: WingId::new(side, index).unwrap(),
                    aircraft: AircraftId::F18,
                    count: if side == launch::Side::Friendly && index == 0 {
                        4
                    } else {
                        5
                    },
                    skill_level: i32::from(index),
                })
            })
            .collect();
        let wings = resolve_wings(&selections, None).unwrap();
        let spawns = mission_spawns(&wings, &SpawnPlan::airborne(10560.0));
        assert_eq!(spawns.len(), 29);
        assert_eq!(spawns[0].offset, [512., 0., -512.]);
        assert_eq!(spawns[1].offset, [-512., 0., -512.]);
        assert_eq!(spawns[2].offset, [1024., 0., -1024.]);
        assert_eq!(spawns[3].offset, [-1024., 0., -1024.]);
        assert_eq!(spawns[4].offset, [-4096., 0., -4096.]);
        assert_eq!(spawns[9].offset, [4096., 0., -4096.]);
        assert_eq!(spawns[14].offset, [0., 0., 10560.]);
        assert_eq!(spawns[15].offset, [-512., 0., 11072.]);
        assert_eq!(spawns[16].offset, [512., 0., 11072.]);
        assert_eq!(spawns[19].offset, [-4096., 0., 10560.]);
        assert_eq!(spawns[24].offset, [4096., 0., 10560.]);
        let basis = Basis::new(std::f64::consts::FRAC_PI_2, 0., 0.);
        let targets: Vec<_> = spawns
            .iter()
            .enumerate()
            .map(|(i, spawn)| {
                let (position, attitude) = spawn.pose([100., 20000., 300.], basis);
                let mut row = target(i as u32 + 1, position, attitude.angles()[0]);
                row.basis = attitude;
                assert_eq!(position[1], 20000.);
                assert_eq!(spawn.opposing, i >= 14);
                assert!((attitude.forward[0] - if i >= 14 { -1. } else { 1. }).abs() < 1e-9);
                row
            })
            .collect();
        for (i, a) in targets.iter().enumerate() {
            for b in &targets[i + 1..] {
                let distance = (a.position[0] - b.position[0]).hypot(a.position[2] - b.position[2]);
                assert!(
                    distance >= 512.,
                    "overlapping wing slots: {} and {}",
                    a.id,
                    b.id
                );
            }
        }
        let bridge = AiWings::build_with(&wings, &targets, 0, |_| Ok((aircraft(), None))).unwrap();
        assert_eq!(bridge.len(), 29);
        for id in 1..=4 {
            assert!(!bridge.mission.actor(id).unwrap().identity().is_leader());
            assert_eq!(
                bridge.mission.actor(id).unwrap().identity().member,
                id as u8
            );
        }
        for id in [5, 10, 15, 20, 25] {
            assert!(bridge.mission.actor(id).unwrap().identity().is_leader());
        }
    }

    /// The fixture spawner's own rule, copied so a test can predict where a
    /// straight-flight target ends up: `live::State::step` adds
    /// `velocity / 120` to every live target once per tick.
    const FIXTURE_TICK_RATE: f64 = 120.0;

    pub(super) fn flat(_x: f64, _z: f64) -> f64 {
        0.0
    }

    fn runway_view(object: u32, center: Vector) -> RunwayView {
        RunwayView {
            airport: object,
            object,
            center,
            heading: 0.,
            length_ft: 6000.,
            elevation_ft: center[1],
            anchors: None,
        }
    }

    /// The player's wing of three (two AI wingmen), one friendly wing 2
    /// aircraft and a pair of enemies.
    fn ground_wings() -> Vec<WingLaunch> {
        let selections = [
            (launch::Side::Friendly, 0u8, 2usize),
            (launch::Side::Friendly, 1, 1),
            (launch::Side::Enemy, 0, 2),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        resolve_wings(&selections, None).unwrap()
    }

    #[test]
    fn runway_slots_park_the_player_wing_and_the_enemy_bearing_turns() {
        let wings = ground_wings();
        let plan = SpawnPlan {
            separation_ft: 60_000.,
            enemy_turn: std::f64::consts::FRAC_PI_2,
            runway_slots: Some(vec![[0., 0.], [40., -250.], [-40., -500.]]),
            situation: Situation::Neutral,
        };
        let spawns = mission_spawns(&wings, &plan);
        assert_eq!(spawns.len(), 5);
        assert_eq!(spawns[0].runway_order, Some(1));
        assert_eq!(spawns[0].offset, [40., 0., -250.]);
        assert_eq!(spawns[1].runway_order, Some(2));
        assert_eq!(spawns[1].offset, [-40., 0., -500.]);
        // Friendly wing 2 keeps its airborne slot and does not turn.
        assert_eq!(spawns[2].runway_order, None);
        assert_eq!(spawns[2].offset, [-4096., 0., -4096.]);
        assert_eq!(spawns[2].turn, 0.);
        // The enemy leader is turned 90 degrees clockwise about the player:
        // due east of a north-facing player, facing back west at it.
        let (position, basis) = spawns[3].pose([0., 9000., 0.], Basis::new(0., 0., 0.));
        assert!((position[0] - 60_000.).abs() < 1e-6 && position[2].abs() < 1e-6);
        assert!((basis.forward[0] + 1.).abs() < 1e-9);
        // An airborne plan leaves the enemy straight ahead.
        let straight = mission_spawns(&wings, &SpawnPlan::airborne(60_000.));
        assert_eq!(straight[3].turn, 0.);
        assert_eq!(straight[0].runway_order, None);
        // The map check sees every enemy aircraft relative to its placement
        // point.
        let group = enemy_group_offsets(&wings, Situation::Neutral);
        assert_eq!(group, [[0., 0.], [-512., 512.]]);
        assert!(enemy_group_offsets(&ground_wings()[..2], Situation::Neutral).is_empty());
    }

    #[test]
    fn the_situation_turns_and_places_the_enemy_formation() {
        use crate::mission::Situation;
        let wings = ground_wings();
        let player = [0., 9000., 0.];
        let north = Basis::new(0., 0., 0.);
        let separation = 30_000.;
        for (situation, lead_z, wingman, faces_north) in [
            // Head-on: ahead, wingman farther away, facing south.
            (
                Situation::Neutral,
                separation,
                [-512., separation + 512.],
                false,
            ),
            // On the enemy's tail: ahead, its wingman trailing toward us.
            (
                Situation::Advantage,
                separation,
                [512., separation - 512.],
                true,
            ),
            // The enemy on ours: behind, its wingman trailing farther back.
            (
                Situation::Disadvantage,
                -separation,
                [512., -separation - 512.],
                true,
            ),
        ] {
            let plan = SpawnPlan {
                situation,
                ..SpawnPlan::airborne(separation)
            };
            let spawns = mission_spawns(&wings, &plan);
            let enemy: Vec<_> = spawns.iter().filter(|s| s.opposing).collect();
            assert_eq!(enemy.len(), 2);
            let (lead, basis) = enemy[0].pose(player, north);
            assert_eq!([lead[0], lead[2]], [0., lead_z], "{situation:?}");
            assert_eq!(
                enemy[1].offset,
                [wingman[0], 0., wingman[1]],
                "{situation:?}"
            );
            assert_eq!(basis.forward[2] > 0.9, faces_north, "{situation:?}");
            // Friendly spawns never change with the situation.
            assert!(spawns.iter().filter(|s| !s.opposing).all(|s| !s.head_on));
            // The map check sees the same group from its placement point,
            // in the frame that points from the player to it.
            let group = enemy_group_offsets(&wings, situation);
            let sign = if situation == Situation::Disadvantage {
                -1.
            } else {
                1.
            };
            for spawn in &enemy {
                let relative = [sign * spawn.offset[0], sign * (spawn.offset[2] - lead_z)];
                assert!(
                    group.contains(&relative),
                    "{situation:?}: {relative:?} in {group:?}"
                );
            }
        }
    }

    #[test]
    fn home_runways_follow_side_allegiance_and_distance() {
        use tore_sim::airport::{Airport, Allegiance, OrientedBox, Runway, Scene};
        let runway = |object: u32, x: f64| Runway {
            object,
            airport: object,
            name: "R".into(),
            surface: OrientedBox {
                center: [x, 0., 0.],
                half: [75., 2., 3000.],
                heading: 0.,
                pitch: 0.,
                bank: 0.,
            },
            approach_center: [x, 0., 0.],
            elevation_ft: 0.,
            heading: 0.,
            length_ft: 6000.,
        };
        let airport = |id: u32, allegiance, neutral_permission| Airport {
            id,
            name: "A".into(),
            runway_objects: vec![id],
            allegiance,
            neutral_permission,
        };
        let scene = Scene {
            aliases: Default::default(),
            objects: Vec::new(),
            runways: [
                (1, 0.),
                (2, 10_000.),
                (3, 20_000.),
                (4, 30_000.),
                (5, 40_000.),
            ]
            .map(|(id, x)| runway(id, x))
            .to_vec(),
            airports: vec![
                airport(1, Allegiance::Friendly, false),
                airport(2, Allegiance::Hostile, false),
                airport(3, Allegiance::Neutral, false),
                airport(4, Allegiance::Neutral, true),
                airport(5, Allegiance::Unknown, true),
            ],
        };
        let fields = Airfields::from_scene(&scene, None);
        // Unknown and unpermitted neutral fields are never home.
        assert_eq!(
            fields
                .runways
                .iter()
                .map(|r| r.view.object)
                .collect::<Vec<_>>(),
            [1, 2, 4]
        );
        let home = |x: f64, side| fields.home([x, 5000., 0.], side).map(|r| r.object);
        assert_eq!(home(9_000., launch::Side::Friendly), Some(1));
        assert_eq!(home(26_000., launch::Side::Friendly), Some(4));
        assert_eq!(home(1_000., launch::Side::Enemy), Some(2));
        assert_eq!(home(29_000., launch::Side::Enemy), Some(4));
        // Equal distance goes to the lower runway id.
        assert_eq!(home(20_000., launch::Side::Enemy), Some(2));
        assert_eq!(
            Airfields::default().home([0.; 3], launch::Side::Enemy),
            None
        );
    }

    /// Regression, found at Goose Green (LFA, 2026-09-23): wingmen that
    /// started on a vertical pad kept it as home and crashed landing there
    /// after a bug out. A departure runway that is nobody's home (a pad is
    /// left out of [`Airfields::runways`]) gives way to the nearest one.
    #[test]
    fn wingmen_parked_on_a_pad_go_home_to_the_nearest_runway() {
        let wings = ground_wings();
        let pad = runway_view(7, [0., 30., 3000.]);
        let field = runway_view(9, [0., 10., 30_000.]);
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![HomeRunway {
                view: field,
                friendly: true,
                enemy: true,
            }],
            departure: Some(Departure {
                runway: pad,
                headings: vec![0.; 3],
                slots: vec![[0., 30., 1100.], [40., 30., 850.], [-40., 30., 600.]],
            }),
            enemy_departure: None,
        };
        let targets = vec![
            target(1, [40., 9000., 850.], 0.),
            target(2, [-40., 9000., 600.], 0.),
            target(3, [-4096., 9000., -3000.], 0.),
            target(4, [0., 9000., 80_000.], std::f64::consts::PI),
            target(5, [-512., 9000., 80_512.], std::f64::consts::PI),
        ];
        let bridge =
            AiWings::build_at(&wings, &targets, &airfields, |_| Ok((aircraft(), None))).unwrap();
        for id in [1u32, 2] {
            let actor = bridge.mission.actor(id).unwrap();
            assert_eq!(actor.ground_start().unwrap().runway, pad);
            assert_eq!(actor.home_runway(), Some(&field));
        }
    }

    /// An open mission (D3c): the AI flies the lead of Friendly Wing 1 too, so
    /// its spawn and its runway slot are the leader's, and the whole wing
    /// parks with no human in it.
    #[test]
    fn an_open_mission_parks_the_whole_first_wing_lead_included() {
        let selections = [
            (launch::Side::Friendly, 0u8, 3usize),
            (launch::Side::Enemy, 0, 2),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let wings = resolve_wings(&selections, None).unwrap();
        let runway_slots = vec![[0., 0.], [40., -250.], [-40., -500.]];
        let plan = SpawnPlan {
            separation_ft: 60_000.,
            enemy_turn: 0.,
            runway_slots: Some(runway_slots.clone()),
            situation: Situation::Neutral,
        };
        let spawns = mission_spawns_for(&wings, &plan, false);
        assert_eq!(spawns.len(), 5);
        for (order, slot) in runway_slots.iter().enumerate() {
            assert_eq!(spawns[order].runway_order, Some(order as u8));
            assert_eq!(spawns[order].offset, [slot[0], 0., slot[1]]);
        }
        // Single player's player takes the lead's slot outside the wing.
        assert_eq!(mission_spawns(&wings, &plan)[0].runway_order, Some(1));

        let departure_runway = runway_view(7, [0., 30., 3000.]);
        let slots = vec![[0., 30., 1100.], [40., 30., 850.], [-40., 30., 600.]];
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![HomeRunway {
                view: departure_runway,
                friendly: true,
                enemy: true,
            }],
            departure: Some(Departure {
                runway: departure_runway,
                headings: vec![0.; 3],
                slots: slots.clone(),
            }),
            enemy_departure: None,
        };
        let targets = vec![
            target(0, [0., 9000., 1100.], 0.),
            target(1, [40., 9000., 850.], 0.),
            target(2, [-40., 9000., 600.], 0.),
            target(3, [0., 9000., 80_000.], std::f64::consts::PI),
            target(4, [-512., 9000., 80_512.], std::f64::consts::PI),
        ];
        let bridge = AiWings::build_for(&wings, &targets, &airfields, &[], |_| {
            Ok((aircraft(), None))
        })
        .unwrap();
        for id in 0u32..3 {
            let actor = bridge.mission.actor(id).unwrap();
            let flight = actor.flight();
            assert!(flight.research.as_ref().is_some_and(|r| r.on_ground));
            let slot = slots[id as usize];
            assert_eq!([flight.position[0], flight.position[2]], [slot[0], slot[2]]);
            assert_eq!(actor.ground_start().unwrap().order, id as u8);
            assert_eq!(actor.identity().member, id as u8);
        }
        assert!(bridge.mission.actor(0).unwrap().identity().is_leader());
        // The enemy stays in the air.
        assert!(bridge.mission.actor(3).unwrap().ground_start().is_none());
    }

    #[test]
    fn an_enemy_ground_start_parks_enemy_wing_one_on_its_own_runway() {
        let wings = ground_wings();
        let red_runway = runway_view(8, [0., 10., 90_000.]);
        let slots = vec![[0., 10., 88_000.], [40., 10., 87_750.]];
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![
                HomeRunway {
                    view: runway_view(7, [0., 30., 3000.]),
                    friendly: true,
                    enemy: false,
                },
                HomeRunway {
                    view: red_runway,
                    friendly: false,
                    enemy: true,
                },
            ],
            departure: None,
            enemy_departure: None,
        }
        .with_enemy_departure(Some(Departure {
            runway: red_runway,
            headings: vec![std::f64::consts::PI; 2],
            slots: slots.clone(),
        }));
        let targets = vec![
            target(1, [40., 9000., 850.], 0.),
            target(2, [-40., 9000., 600.], 0.),
            target(3, [-4096., 9000., -3000.], 0.),
            target(4, [0., 9000., 80_000.], std::f64::consts::PI),
            target(5, [-512., 9000., 80_512.], std::f64::consts::PI),
        ];
        let bridge =
            AiWings::build_at(&wings, &targets, &airfields, |_| Ok((aircraft(), None))).unwrap();
        for (id, order) in [(4u32, 0u8), (5, 1)] {
            let actor = bridge.mission.actor(id).unwrap();
            let flight = actor.flight();
            assert!(flight.research.as_ref().is_some_and(|r| r.on_ground));
            let slot = slots[usize::from(order)];
            assert_eq!([flight.position[0], flight.position[2]], [slot[0], slot[2]]);
            let start = actor.ground_start().unwrap();
            assert_eq!((start.order, start.runway), (order, red_runway));
            assert_eq!(actor.home_runway(), Some(&red_runway));
        }
        // The friendly side stays in the air, with its own field as home.
        for id in 1u32..=3 {
            let actor = bridge.mission.actor(id).unwrap();
            assert!(actor.ground_start().is_none());
            assert_eq!(actor.home_runway().map(|r| r.object), Some(7));
        }
    }

    #[test]
    fn a_ground_start_parks_wingmen_on_the_runway_hidden_from_radar() {
        let wings = ground_wings();
        let departure_runway = runway_view(7, [0., 30., 3000.]);
        let slots = vec![[0., 30., 1100.], [40., 30., 850.], [-40., 30., 600.]];
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![
                HomeRunway {
                    view: departure_runway,
                    friendly: true,
                    enemy: true,
                },
                HomeRunway {
                    view: runway_view(8, [0., 10., 90_000.]),
                    friendly: false,
                    enemy: true,
                },
            ],
            departure: Some(Departure {
                runway: departure_runway,
                headings: vec![0., std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2],
                slots: slots.clone(),
            }),
            enemy_departure: None,
        };
        // Combat::reset placed everyone in the air around the player.
        let mut targets = vec![
            target(1, [40., 9000., 850.], 0.),
            target(2, [-40., 9000., 600.], 0.),
            target(3, [-4096., 9000., -3000.], 0.),
            target(4, [0., 9000., 80_000.], std::f64::consts::PI),
            target(5, [-512., 9000., 80_512.], std::f64::consts::PI),
        ];
        let mut bridge =
            AiWings::build_at(&wings, &targets, &airfields, |_| Ok((aircraft(), None))).unwrap();
        for (id, order) in [(1u32, 1u8), (2, 2)] {
            let actor = bridge.mission.actor(id).unwrap();
            let flight = actor.flight();
            assert!(flight.research.as_ref().is_some_and(|r| r.on_ground));
            let slot = slots[usize::from(order)];
            assert_eq!([flight.position[0], flight.position[2]], [slot[0], slot[2]]);
            assert!(flight.position[1] >= 30. && flight.position[1] < 60.);
            assert!(flight.speed == 0. && flight.brake_out && flight.gear_down);
            assert_eq!(flight.yaw, std::f64::consts::FRAC_PI_2);
            let start = actor.ground_start().unwrap();
            assert_eq!((start.order, start.end), (order, ApproachEnd::Near));
            assert_eq!(start.runway, departure_runway);
            assert_eq!(actor.home_runway(), Some(&departure_runway));
        }
        // Airborne aircraft stay on the legacy model at their spawn, with
        // the nearest runway their side may use as home.
        let wing_two = bridge.mission.actor(3).unwrap();
        assert!(wing_two.flight().research.is_none() && wing_two.ground_start().is_none());
        assert_eq!(wing_two.home_runway().map(|r| r.object), Some(7));
        let enemy = bridge.mission.actor(4).unwrap();
        assert_eq!(enemy.flight().position, [0., 9000., 80_000.]);
        assert_eq!(enemy.home_runway().map(|r| r.object), Some(8));

        // Parked aircraft return no radar echo to anyone; airborne ones do.
        let objects = bridge.snapshot(vec![player_object([0., 30., 1100.])], &targets);
        let radar = |id: u32| {
            objects
                .iter()
                .find(|o| o.id == id)
                .and_then(|o| o.observable.as_ref())
                .map(|o| o.signature.radar)
                .unwrap()
        };
        assert!(objects.iter().find(|o| o.id == 1).unwrap().on_ground);
        assert_eq!(radar(1), 0.);
        assert_eq!(radar(2), 0.);
        assert!(radar(3) > 0. && radar(4) > 0.);
        // The combat rows follow, so the player's radar ignores them too.
        bridge.mirror_pose_out(&mut targets);
        assert!(targets[0].on_ground && targets[1].on_ground && !targets[3].on_ground);
        assert_eq!(
            targets[0].position,
            bridge.mission.actor(1).unwrap().flight().position
        );

        // The bridge steps them on the runway surface without crashing.
        let surface = |_: f64, _: f64| tore_sim::research::Surface::runway(30.);
        for _ in 0..120 {
            bridge
                .advance_on_surface(
                    vec![player_object([0., 30., 1100.])],
                    &mut targets,
                    &|_, _| 30.,
                    &surface,
                )
                .unwrap();
        }
        for id in [1, 2] {
            let flight = bridge.mission.actor(id).unwrap().flight();
            assert!(!flight.crashed, "wingman {id} crashed on the runway");
        }
        // A missing slot is an error, never an aircraft dropped from the sky.
        let mut short = airfields.clone();
        short.departure.as_mut().unwrap().slots.truncate(2);
        assert!(
            AiWings::build_at(&wings, &spawned_ground(), &short, |_| Ok((
                aircraft(),
                None
            )))
            .is_err()
        );
    }

    fn spawned_ground() -> Vec<live::Target> {
        (1..=5)
            .map(|id| target(id, [0., 9000., f64::from(id) * 1000.], 0.))
            .collect()
    }

    #[test]
    fn player_group_objective_distinguishes_two_populated_enemy_groups_from_free_fire() {
        let selections = [
            (launch::Side::Friendly, 1u8),
            (launch::Side::Enemy, 0),
            (launch::Side::Enemy, 1),
        ]
        .map(|(side, index)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count: 2,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        let targets = vec![
            target(1, [0., 20000., 0.], 0.),
            target(2, [1500., 20000., 0.], 0.),
            target(3, [0., 20000., 40000.], std::f64::consts::PI),
            target(4, [1500., 20000., 40000.], std::f64::consts::PI),
            target(5, [8000., 20000., 40000.], std::f64::consts::PI),
            target(6, [9500., 20000., 40000.], std::f64::consts::PI),
        ];
        let mut wings =
            AiWings::build_with(&payload, &targets, 0, |_| Ok((aircraft(), None))).unwrap();
        let enemy_one = WingId::new(launch::Side::Enemy, 0).unwrap();
        let mut objectives = [GroupObjective::Inherit; 6];
        objectives[0] = GroupObjective::Intercept(enemy_one);
        objectives[1] = GroupObjective::Intercept(enemy_one);
        wings.apply_group_objectives(&objectives, [0., 20000., 0.]);

        assert_eq!(
            wings.mission.human_assignment(PLAYER_ID).destroy_ids,
            [3, 4]
        );
        for id in [1, 2] {
            assert_eq!(
                wings.mission.actor(id).unwrap().assignment().destroy_ids,
                [3, 4]
            );
        }
        for id in [3, 4] {
            assert!(wings.objective_for_player(id));
        }
        for id in [5, 6] {
            assert!(!wings.objective_for_player(id));
        }
        let mut group_one = crate::target_window::Readout::new(
            &crate::readout::TargetRow::of(&targets[2]),
            wings.mission.actor(3).unwrap().flight(),
            "TEST".into(),
        );
        group_one.with_brief(&crate::target_window::TargetBrief::of(
            &wings,
            PLAYER_ID,
            group_one.id,
        ));
        let mut group_two = crate::target_window::Readout::new(
            &crate::readout::TargetRow::of(&targets[4]),
            wings.mission.actor(5).unwrap().flight(),
            "TEST".into(),
        );
        group_two.with_brief(&crate::target_window::TargetBrief::of(
            &wings,
            PLAYER_ID,
            group_two.id,
        ));
        assert_eq!(
            group_one.objective,
            Some(crate::target_window::TargetObjective::Destroy)
        );
        assert_eq!(group_two.objective, None);

        objectives[0] = GroupObjective::Free;
        wings.apply_group_objectives(&objectives, [0., 20000., 0.]);
        assert!(
            wings
                .mission
                .human_assignment(PLAYER_ID)
                .destroy_ids
                .is_empty()
        );
        assert!(
            [3, 4, 5, 6]
                .into_iter()
                .all(|id| !wings.objective_for_player(id))
        );
        let contact = |id, position| TargetView {
            infrared_strength: 1.,
            id,
            side: ENEMY_SIDE,
            position,
            heading_deg: 0.,
            pitch_deg: 0.,
            speed: ScalarSpeed(400.),
            maximum_speed: ScalarSpeed(800.),
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
        };
        for candidate in [
            contact(3, [1000., 20000., 0.]),
            contact(5, [1000., 20000., 0.]),
        ] {
            let selected = Policy::default()
                .select(
                    PLAYER_ID,
                    FRIENDLY_SIDE,
                    [0., 20000., 0.],
                    &wings.mission.human_assignment(PLAYER_ID),
                    &[candidate],
                    &[],
                    &[],
                    None,
                    1,
                )
                .unwrap();
            assert_eq!(selected.id, candidate.id);
            assert_eq!(selected.priority, Priority::Free);
        }
    }

    #[test]
    fn dummy_mode_reaches_live_targets_without_changing_other_wings() {
        let mut payload = payload(None);
        payload[1].dummy = true;
        let mut targets = spawned();
        let mut wings =
            AiWings::build_with(&payload, &targets, 0, |_| Ok((aircraft(), None))).unwrap();
        wings.mirror_pose_out(&mut targets);
        assert!(!wings.mission.actor(1).unwrap().is_dummy());
        assert!(wings.mission.actor(3).unwrap().is_dummy());
        let start = targets[2].position;
        let velocity = targets[2].velocity;
        for _ in 0..120 {
            wings
                .advance(
                    player_object([0., 20000., -20000.]),
                    &mut targets,
                    &|_, _| 0.,
                )
                .unwrap();
        }
        for i in 0..3 {
            assert!((targets[2].position[i] - start[i] - velocity[i]).abs() < 1e-7);
        }
        assert_eq!(targets[2].velocity, velocity);
        assert!(
            (tore_sim::attitude::dot(velocity, velocity).sqrt() - launch::DUMMY_SPEED_FPS).abs()
                < 1e-9
        );
        targets[2].hp = 0;
        let stopped = targets[2].position;
        wings
            .advance(
                player_object([0., 20000., -20000.]),
                &mut targets,
                &|_, _| 0.,
            )
            .unwrap();
        assert_eq!(targets[2].position, stopped);
        assert!(!wings.mission.actor(3).unwrap().alive());
    }

    pub(super) fn build(
        enemy_override: Option<EnemySkillOverride>,
    ) -> (AiWings, Vec<live::Target>) {
        let targets = spawned();
        let wings = AiWings::build_with(&payload(enemy_override), &targets, 0, |_| {
            Ok((aircraft(), None))
        })
        .unwrap();
        (wings, targets)
    }

    #[test]
    fn startup_is_neutral_but_only_ai_led_wings_release_on_permitted_contact() {
        let (mut wings, mut targets) = build(None);
        let human_wing = wings.mission.actor(1).unwrap().identity().wing;
        wings
            .mission
            .set_external_leader(FRIENDLY_SIDE, human_wing, PLAYER_ID);
        wings.apply_mission_preset(Preset::Free, [0., 20000., 0.]);
        wings.apply_group_objectives(&[GroupObjective::Inherit; 6], [0., 20000., 0.]);
        assert!(wings.mission.actors().iter().all(AiActor::is_neutral));
        for _ in 0..240 {
            let output = wings
                .advance(
                    player_object([0., 20000., -20000.]),
                    &mut targets,
                    &|_, _| 0.,
                )
                .unwrap();
            assert!(output.launches.is_empty());
        }
        for actor in wings.mission.actors() {
            assert_eq!(actor.is_neutral(), actor.identity().side == FRIENDLY_SIDE);
        }
    }

    /// The single-player entry for a flight state, for tests.
    pub(crate) fn human(flight: &flight::State) -> HumanAircraft<'_> {
        static CONFIG: std::sync::OnceLock<live::Configuration> = std::sync::OnceLock::new();
        HumanAircraft::from_parts(
            HumanSlot::SINGLE_PLAYER,
            flight,
            1000,
            CONFIG.get_or_init(|| combat_fixture(false).own().configuration().clone()),
            sensors::Stores::NONE,
        )
    }

    /// [`human`] for any slot.
    pub(crate) fn human_at(slot: HumanSlot, flight: &flight::State) -> HumanAircraft<'_> {
        HumanAircraft {
            slot,
            ..human(flight)
        }
    }

    #[test]
    fn a_human_seat_shows_the_ai_radars_its_external_stores() {
        let player = flight::State::new(&aircraft(), [0., 20000., 0.]).unwrap();
        let clean = AiWings::human_object(&human(&player));
        let stores = sensors::Stores {
            count: 2.,
            lbs: 8770.,
        };
        let loaded = AiWings::human_object(&HumanAircraft {
            stores,
            ..human(&player)
        });
        let seen = |o: &WorldObject| o.observable.as_ref().unwrap().configuration;
        assert_eq!(seen(&clean), sensors::Configuration::CLEAN);
        assert_eq!(seen(&loaded), sensors::Configuration::carrying(stores));
    }

    #[test]
    fn every_human_aircraft_is_a_world_object_in_id_order() {
        // Friendly wing 1 has two humans (members 0 and 1) and two AI
        // wingmen, and a lone enemy flies against them.
        let selections = [
            (launch::Side::Friendly, 0u8, 2usize),
            (launch::Side::Enemy, 0, 1),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        let targets = vec![
            target(1, [0., 20000., 0.], 0.),
            target(2, [1500., 20000., 0.], 0.),
            target(3, [0., 20000., 40000.], std::f64::consts::PI),
        ];
        let slot = |id, member| HumanSlot {
            id,
            side: launch::Side::Friendly,
            wing: 0,
            member,
        };
        let humans = [slot(5, 1), slot(0, 0)];
        let wings = AiWings::build_for(&payload, &targets, &Airfields::default(), &humans, |_| {
            Ok((aircraft(), None))
        })
        .unwrap();
        // The AI fills the member numbers the humans leave free.
        let members: Vec<_> = wings
            .mission
            .actors()
            .iter()
            .map(|a| (a.id(), a.identity().member))
            .collect();
        assert_eq!(members, [(1, 2), (2, 3), (3, 0)]);
        assert_eq!(
            wings.humans,
            [slot(0, 0), slot(5, 1)],
            "registered in id order"
        );
        assert_eq!(
            wings.friendly_ids(launch::Side::Friendly),
            [0, 1, 2, 5].into(),
            "the friendly list holds every friendly aircraft, human or AI"
        );
        assert_eq!(
            wings.friendly_ids(launch::Side::Enemy),
            [3].into(),
            "and the enemy list only the enemy"
        );

        let one = flight::State::new(&aircraft(), [0., 20000., -1000.]).unwrap();
        let mut two = flight::State::new(&aircraft(), [500., 21000., -1000.]).unwrap();
        two.crashed = true;
        let objects = wings.snapshot(
            [human_at(slot(0, 0), &one), human_at(slot(5, 1), &two)]
                .iter()
                .map(AiWings::human_object)
                .collect(),
            &targets,
        );
        let ids: Vec<_> = objects.iter().map(|o| (o.id, o.human_controlled)).collect();
        assert_eq!(
            ids,
            [(0, true), (5, true), (1, false), (2, false), (3, false)]
        );
        assert!(objects[0].alive && !objects[1].alive);
        assert_eq!(objects[1].position, [500., 21000., -1000.]);
        // Every human is a world object on its own side.
        assert!(objects[..2].iter().all(|o| o.side == FRIENDLY_SIDE));
    }

    /// Friendly wings 1 and 2 each led by a human (ids 0 and 9) with one AI
    /// wingman (ids 1 and 2), and one enemy (id 3).
    fn two_led_wings() -> (AiWings, Vec<live::Target>, [HumanSlot; 2]) {
        let selections = [
            (launch::Side::Friendly, 0u8, 1usize),
            (launch::Side::Friendly, 1, 1),
            (launch::Side::Enemy, 0, 1),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        let targets = vec![
            target(1, [512., 20000., -512.], 0.),
            target(2, [-4096., 20000., -4096.], 0.),
            target(3, [0., 20000., 80000.], std::f64::consts::PI),
        ];
        let humans = [
            HumanSlot::SINGLE_PLAYER,
            HumanSlot {
                id: 9,
                side: launch::Side::Friendly,
                wing: 1,
                member: 0,
            },
        ];
        let wings = AiWings::build_for(&payload, &targets, &Airfields::default(), &humans, |_| {
            Ok((aircraft(), None))
        })
        .unwrap();
        (wings, targets, humans)
    }

    #[test]
    fn a_wing_led_by_any_human_follows_that_human() {
        let (mut wings, mut targets, _) = two_led_wings();
        let led = |position: Vector, id: u32| WorldObject {
            infrared_strength: 1.,
            id,
            ..player_object(position)
        };
        // Both leaders are registered, and only wing members follow.
        for _ in 0..30 {
            wings
                .advance_on_surface(
                    vec![led([0., 20000., 0.], 0), led([-4096., 20000., -4096.], 9)],
                    &mut targets,
                    &flat,
                    &|x, z| tore_sim::research::Surface::terrain(flat(x, z)),
                )
                .unwrap();
        }
        let formating = |wings: &AiWings, id: u32| {
            wings
                .mission
                .actor(id)
                .unwrap()
                .controller()
                .formation_trace()
                .is_some()
        };
        for id in [1, 2] {
            assert!(formating(&wings, id), "actor {id} has a leader to fly on");
        }
        // Wing 2's leader disappears from the world: its wingman has none,
        // wing 1's still does.
        for _ in 0..30 {
            wings
                .advance_on_surface(
                    vec![led([0., 20000., 0.], 0)],
                    &mut targets,
                    &flat,
                    &|x, z| tore_sim::research::Surface::terrain(flat(x, z)),
                )
                .unwrap();
        }
        assert!(formating(&wings, 1));
        assert!(!formating(&wings, 2));
    }

    #[test]
    fn orders_go_to_the_senders_own_wing_and_humans_only_record_them() {
        use tore_sim::ai::wing::{Formation, PlayerOrder};
        let (mut wings, _, _) = two_led_wings();
        wings
            .command(9, PlayerOrder::Formation(Formation::LineAstern), None, None)
            .unwrap();
        assert_eq!(
            wings
                .mission
                .actor(2)
                .unwrap()
                .controller()
                .ordered_formation(),
            Some(Formation::LineAstern)
        );
        assert_eq!(
            wings
                .mission
                .actor(1)
                .unwrap()
                .controller()
                .ordered_formation(),
            None,
            "the other wing's wingman is not ordered"
        );
        assert_eq!(wings.next_formation(9, None), Formation::Echelon);
        assert_eq!(
            wings.next_formation(PLAYER_ID, None),
            Formation::LineAbreast
        );
        // A second human in wing 1 (member 1) is addressed too, and answers
        // that it is flown by a human.
        wings.humans.push(HumanSlot {
            id: 12,
            side: launch::Side::Friendly,
            wing: 1,
            member: 1,
        });
        let report = wings
            .command(9, PlayerOrder::Formation(Formation::Echelon), None, None)
            .unwrap();
        assert!(
            report.message.ends_with("1 flown by a human"),
            "{}",
            report.message
        );
        // An aircraft that is not a human-flown member has no wing to order.
        let report = wings
            .command(77, PlayerOrder::Formation(Formation::Echelon), None, None)
            .unwrap();
        assert!(report.message.contains("no addressed wingmen"));
    }

    #[test]
    fn assignments_and_survival_are_kept_for_each_human() {
        let (mut wings, _, _) = two_led_wings();
        wings.apply_mission_preset(Preset::Escort, [0., 20000., 0.]);
        // Every friendly AI aircraft protects every friendly human; the
        // enemy primary intercepts them all.
        assert_eq!(
            wings.mission.actor(1).unwrap().assignment().protected_ids,
            [0, 9]
        );
        assert_eq!(
            wings.mission.actor(3).unwrap().assignment().destroy_ids,
            [0, 9]
        );
        // A group stamp reaches only the humans in that group.
        let mut objectives = [GroupObjective::Inherit; 6];
        objectives[1] = GroupObjective::Hold;
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert_eq!(
            wings.mission.human_assignment(9).stance,
            tore_sim::ai::engagement::Stance::WeaponsHold
        );
        assert_ne!(
            wings.mission.human_assignment(PLAYER_ID).stance,
            tore_sim::ai::engagement::Stance::WeaponsHold
        );
        // Survival lists are per human, and name the humans of the group.
        let mut survival = [false; 6];
        survival[1] = true;
        wings.apply_group_survival(&survival);
        assert_eq!(wings.mission.must_survive(0), [2, 9]);
        assert_eq!(wings.mission.must_survive(9), [2, 9]);
        wings.mission.set_must_survive(9, vec![3]);
        assert_eq!(wings.mission.must_survive(0), [2, 9]);
        assert_eq!(wings.mission.must_survive(9), [3]);
        assert_eq!(wings.mission.must_survive(55), [] as [u32; 0]);
    }

    #[test]
    fn landing_priority_is_updated_for_each_human() {
        let (mut wings, _, _) = two_led_wings();
        wings.set_priority_landing(0, Some(7));
        wings.set_priority_landing(9, Some(8));
        assert_eq!(
            wings.mission.priority_landings().collect::<Vec<_>>(),
            [(0, 7), (9, 8)]
        );
        wings.set_priority_landing(0, None);
        assert_eq!(
            wings.mission.priority_landings().collect::<Vec<_>>(),
            [(9, 8)]
        );
    }

    /// Friendly wing 2 (index 1) of `count` AI aircraft, ids 1 upward, led by
    /// its member 0, against one distant enemy. `humans` are flown by people
    /// and take the member numbers they name.
    fn led_wing(count: usize, humans: &[HumanSlot]) -> (AiWings, Vec<live::Target>) {
        led_wing_of(count, humans, 20000., aircraft)
    }

    /// [`led_wing`] with every aircraft built from `profile`.
    fn led_wing_of(
        count: usize,
        humans: &[HumanSlot],
        altitude: f64,
        profile: impl Fn() -> Aircraft,
    ) -> (AiWings, Vec<live::Target>) {
        let selections = [
            (launch::Side::Friendly, 1u8, count),
            (launch::Side::Enemy, 0, 1),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        let mut targets: Vec<_> = (1..=count as u32)
            .map(|id| target(id, [f64::from(id) * 600., altitude, 0.], 0.))
            .collect();
        targets.push(target(
            count as u32 + 1,
            [0., altitude, 900_000.],
            std::f64::consts::PI,
        ));
        let mut wings =
            AiWings::build_for(&payload, &targets, &Airfields::default(), humans, |_| {
                Ok((profile(), None))
            })
            .unwrap();
        // Weapons hold: the flights keep formation instead of chasing the
        // enemy the fixture's sensorless aircraft can see at any range.
        wings.apply_mission_preset(Preset::Hold, [0., altitude, 0.]);
        (wings, targets)
    }

    fn fly_one_tick(
        wings: &mut AiWings,
        targets: &mut [live::Target],
        humans: Vec<WorldObject>,
    ) -> tore_sim::ai::mission::MissionOutput {
        wings
            .advance_on_surface(humans, targets, &flat, &|x, z| {
                tore_sim::research::Surface::terrain(flat(x, z))
            })
            .unwrap()
    }

    fn leads(wings: &AiWings) -> Vec<u32> {
        wings
            .mission
            .actors()
            .iter()
            .filter(|a| a.identity().is_leader() && a.identity().side == FRIENDLY_SIDE)
            .map(AiActor::id)
            .collect()
    }

    #[test]
    fn an_ai_leader_dies_and_the_next_member_leads_the_re_formed_flight() {
        let (mut wings, mut targets) = led_wing(4, &[]);
        assert_eq!(leads(&wings), [1]);
        let output = fly_one_tick(&mut wings, &mut targets, vec![]);
        assert!(
            output.leadership.is_empty(),
            "nothing changes while it lives"
        );
        // Formation slots follow member order behind the leader.
        let slots = |wings: &AiWings| -> Vec<(u32, u8)> {
            wings
                .mission
                .actors()
                .iter()
                .filter(|a| a.alive() && a.identity().side == FRIENDLY_SIDE)
                .map(|a| (a.id(), a.wing_slot()))
                .collect()
        };
        assert_eq!(slots(&wings), [(1, 1), (2, 1), (3, 2), (4, 3)]);

        for _ in 0..10 {
            fly_one_tick(&mut wings, &mut targets, vec![]);
        }
        targets[0].hp = 0;
        let output = fly_one_tick(&mut wings, &mut targets, vec![]);
        assert_eq!(output.leadership.len(), 1, "lead passes on that tick");
        let change = output.leadership[0];
        assert_eq!((change.leader, change.previous), (2, 1));
        assert_eq!((change.side, change.wing), (FRIENDLY_SIDE, 1));
        assert!(!change.previous_pilot_alive);
        assert_eq!(leads(&wings), [2]);
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(2));
        // The flight re-forms: the followers close up behind the new leader.
        assert_eq!(slots(&wings), [(2, 1), (3, 1), (4, 2)]);
        // The wingmen fly on the new leader, not free.
        for _ in 0..10 {
            fly_one_tick(&mut wings, &mut targets, vec![]);
        }
        for id in [3, 4] {
            assert!(
                wings
                    .mission
                    .actor(id)
                    .unwrap()
                    .controller()
                    .formation_trace()
                    .is_some(),
                "actor {id} formates on its new leader"
            );
        }
        // And again when the new leader goes.
        targets[1].hp = 0;
        let output = fly_one_tick(&mut wings, &mut targets, vec![]);
        assert_eq!(output.leadership.len(), 1);
        assert_eq!(leads(&wings), [3]);
        assert_eq!(slots(&wings), [(3, 1), (4, 1)]);
        // A wing with no other member left keeps what it had.
        targets[2].hp = 0;
        targets[3].hp = 0;
        fly_one_tick(&mut wings, &mut targets, vec![]);
        let output = fly_one_tick(&mut wings, &mut targets, vec![]);
        assert!(output.leadership.is_empty());
    }

    #[test]
    fn a_human_leader_dies_and_an_ai_wingman_leads() {
        let (mut wings, mut targets) = led_wing(
            2,
            &[HumanSlot {
                wing: 1,
                ..HumanSlot::SINGLE_PLAYER
            }],
        );
        let members: Vec<_> = wings
            .mission
            .actors()
            .iter()
            .filter(|a| a.identity().side == FRIENDLY_SIDE)
            .map(|a| (a.id(), a.identity().member, a.identity().is_leader()))
            .collect();
        // The human of wing 1 is member 0; the AI aircraft fly its wing.
        assert_eq!(members, [(1, 1, false), (2, 2, false)]);
        let mut player = player_object([0., 20000., -600.]);
        player.id = 0;
        for _ in 0..5 {
            fly_one_tick(&mut wings, &mut targets, vec![player.clone()]);
        }
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(0));
        // The player is shot down.
        player.alive = false;
        player.destroyed = true;
        let output = fly_one_tick(&mut wings, &mut targets, vec![player.clone()]);
        assert_eq!(output.leadership.len(), 1);
        assert_eq!(output.leadership[0].leader, 1);
        assert_eq!(output.leadership[0].previous, 0);
        assert!(!output.leadership[0].previous_pilot_alive);
        assert_eq!(leads(&wings), [1]);
        assert_eq!(wings.mission.actor(2).unwrap().wing_slot(), 1);
    }

    #[test]
    fn a_human_wingman_is_preferred_to_a_lower_numbered_ai_wingman() {
        let slot = |id, member| HumanSlot {
            id,
            side: launch::Side::Friendly,
            wing: 1,
            member,
        };
        // Humans lead (member 0) and fly member 2; the AI aircraft take
        // members 1 and 3.
        let humans = [slot(20, 0), slot(21, 2)];
        let (mut wings, mut targets) = led_wing(2, &humans);
        let members: Vec<_> = wings
            .mission
            .actors()
            .iter()
            .filter(|a| a.identity().side == FRIENDLY_SIDE)
            .map(|a| (a.id(), a.identity().member))
            .collect();
        assert_eq!(members, [(1, 1), (2, 3)]);
        let object = |id, alive| WorldObject {
            infrared_strength: 1.,
            id,
            alive,
            destroyed: !alive,
            ..player_object([id as f64 * 300., 20000., -600.])
        };
        fly_one_tick(
            &mut wings,
            &mut targets,
            vec![object(20, true), object(21, true)],
        );
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(20));
        let output = fly_one_tick(
            &mut wings,
            &mut targets,
            vec![object(20, false), object(21, true)],
        );
        assert_eq!(
            output.leadership[0].leader, 21,
            "the human, not AI member 1"
        );
        // The AI members follow in member order behind it: 1 then 3.
        assert_eq!(wings.mission.actor(1).unwrap().wing_slot(), 1);
        assert_eq!(wings.mission.actor(2).unwrap().wing_slot(), 2);
        let output = fly_one_tick(
            &mut wings,
            &mut targets,
            vec![object(20, false), object(21, false)],
        );
        assert_eq!(output.leadership[0].leader, 1, "then the lowest AI member");
        assert_eq!(wings.mission.actor(2).unwrap().wing_slot(), 1);
    }

    #[test]
    fn a_leader_that_ejects_hands_over_and_its_living_pilot_is_reported() {
        let (mut wings, mut targets) = led_wing(3, &[]);
        fly_one_tick(&mut wings, &mut targets, vec![]);
        {
            // The synthetic profile has no ejection seat, so the pilot's
            // escape is set up by hand, as `eject` would.
            let flight = wings.mission.actor_mut(1).unwrap().flight_mut();
            flight.escape = Some(tore_sim::ejection::Escape::new(
                flight.position,
                flight.velocity,
                Basis::new(flight.yaw, flight.pitch, flight.bank),
            ));
            flight.systems.pilot.ejected = true;
            flight.crashed = true;
        }
        let output = fly_one_tick(&mut wings, &mut targets, vec![]);
        assert_eq!(output.leadership.len(), 1);
        assert_eq!(output.leadership[0].leader, 2);
        assert!(output.leadership[0].previous_pilot_alive);
        assert_eq!(leads(&wings), [2]);
        // The event reaches the radio with the living previous leader as the
        // speaker, and is journaled there.
        wings.observe_chatter(&output, &[]);
        assert!(wings.chatter.iter().any(|c| matches!(
            c,
            Chatter::Leadership {
                speaker: 1,
                leader: 2,
                previous_pilot_alive: true,
                ..
            }
        )));
    }

    #[test]
    fn only_the_leading_aircraft_can_order_the_wing() {
        use tore_sim::ai::wing::{Formation, PlayerOrder};
        let slot = |id, member| HumanSlot {
            id,
            side: launch::Side::Friendly,
            wing: 1,
            member,
        };
        let (mut wings, mut targets) = led_wing(2, &[slot(20, 0), slot(21, 1)]);
        let object = |id, alive| WorldObject {
            infrared_strength: 1.,
            id,
            alive,
            destroyed: !alive,
            ..player_object([id as f64 * 300., 20000., -600.])
        };
        fly_one_tick(
            &mut wings,
            &mut targets,
            vec![object(20, true), object(21, true)],
        );
        let order = PlayerOrder::Formation(Formation::LineAstern);
        let refused = wings.command(21, order, None, None).unwrap();
        assert!(
            refused.message.contains("not leading"),
            "{}",
            refused.message
        );
        assert!(
            wings
                .command(20, order, None, None)
                .unwrap()
                .message
                .contains("applied")
        );
        // Once the first human is down, the second leads and may order.
        fly_one_tick(
            &mut wings,
            &mut targets,
            vec![object(20, false), object(21, true)],
        );
        assert!(
            wings
                .command(21, order, None, None)
                .unwrap()
                .message
                .contains("applied")
        );
        assert!(
            wings
                .command(20, order, None, None)
                .unwrap()
                .message
                .contains("not leading")
        );
    }

    /// A new aircraft of Friendly wing 2 with no stores.
    fn new_aircraft(wings: &AiWings, id: u32, member: u8) -> ActorInsert {
        ActorInsert {
            id,
            side: launch::Side::Friendly,
            wing: 1,
            member,
            aircraft: AircraftId::F18,
            experience: wings.mission.actor(1).unwrap().experience(),
            flight: flight::State::new(&aircraft(), [3000., 20000., 0.]).unwrap(),
            sensors: None,
            stations: Vec::new(),
            dispensers: Vec::new(),
            warnings: None,
            equipment: EquipmentFaults::default(),
            config: None,
        }
    }

    fn ids(wings: &AiWings) -> Vec<u32> {
        wings.mission.actors().iter().map(AiActor::id).collect()
    }

    #[test]
    fn a_removed_actor_goes_back_as_it_was_and_the_others_keep_their_order() {
        let (mut wings, mut targets) = led_wing(4, &[]);
        // What the roster gives an aircraft: an AI aircraft as built, before
        // it has flown and with no assignment yet.
        wings
            .mission
            .actor_mut(3)
            .unwrap()
            .set_assignment(Default::default());
        let before = wings.mission.actor(3).unwrap().controller().clone();
        fly_one_tick(&mut wings, &mut targets, vec![]);
        // Actor 3 leaves the AI; nothing else moves.
        let stations = wings.mission.actor(3).unwrap().stations().to_vec();
        let flight = wings.mission.actor(3).unwrap().flight().clone();
        let removed = wings.remove_actor(3).expect("a flying actor");
        assert_eq!(ids(&wings), [1, 2, 4, 5]);
        assert_eq!(wings.len(), 4);
        assert!(wings.slot(3).is_none() && wings.mission.actor(3).is_none());
        assert_eq!(removed.slot.id, 3);
        assert_eq!(removed.parts.flight, flight);
        assert_eq!(removed.parts.stations, stations);
        assert!(wings.remove_actor(3).is_none(), "only once");
        // The AI keeps flying without it.
        fly_one_tick(&mut wings, &mut targets, vec![]);

        // Putting it back places it in id order, as at mission start:
        // neutral, the same seed, fresh awareness.
        wings
            .insert_actor(ActorInsert::from_removed(removed))
            .unwrap();
        assert_eq!(ids(&wings), [1, 2, 3, 4, 5]);
        assert_eq!(
            wings.slots().iter().map(|s| s.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        let back = wings.mission.actor(3).unwrap();
        assert!(back.is_neutral());
        assert_eq!(back.controller(), &before);
        assert_eq!(back.wing_slot(), 2);
        assert_eq!(wings.slot(3).unwrap().label(), "Friendly 2-3");
        assert!(
            wings.insert_actor(new_aircraft(&wings, 3, 2)).is_err(),
            "an aircraft the AI already flies cannot join twice"
        );
    }

    #[test]
    fn an_aircraft_joins_the_ai_with_the_seed_and_slot_the_roster_gives_it() {
        let slot = |id, member| HumanSlot {
            id,
            side: launch::Side::Friendly,
            wing: 1,
            member,
        };
        // A human leads (member 0) and two AI aircraft fly members 1 and 2.
        let (mut wings, mut targets) = led_wing(2, &[slot(20, 0)]);
        wings
            .mission
            .actor_mut(1)
            .unwrap()
            .set_assignment(Default::default());
        let original = wings.mission.actor(1).unwrap().controller().clone();
        // Its aircraft goes to a human and back: registered as human while
        // away, then handed back to the AI.
        let removed = wings.remove_actor(1).unwrap();
        wings.humans.push(slot(1, 1));
        wings.humans.retain(|h| h.id != 1);
        wings
            .insert_actor(ActorInsert::from_removed(removed))
            .unwrap();
        assert_eq!(wings.mission.actor(1).unwrap().controller(), &original);

        // A brand new aircraft as member 4 of the wing, after the flight has
        // lost its human leader and re-formed: it takes the rank behind it.
        let object = |alive| WorldObject {
            infrared_strength: 1.,
            id: 20,
            alive,
            destroyed: !alive,
            ..player_object([0., 20000., -600.])
        };
        fly_one_tick(&mut wings, &mut targets, vec![object(true)]);
        fly_one_tick(&mut wings, &mut targets, vec![object(false)]);
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(1));
        wings.insert_actor(new_aircraft(&wings, 30, 4)).unwrap();
        assert_eq!(ids(&wings), [1, 2, 3, 30]);
        let joined = wings.mission.actor(30).unwrap();
        assert!(!joined.identity().is_leader());
        assert_eq!(joined.wing_slot(), 2, "behind the leader and member 2");
        assert_eq!(wings.mission.actor(2).unwrap().wing_slot(), 1);
    }

    #[test]
    fn an_aircraft_with_a_high_member_number_joins_in_a_free_slot_and_the_ai_steps() {
        // Slice R1: an aircraft that joins a wing that has not re-formed
        // with member 12 (a wing that revived and respawned for a while)
        // takes the lowest formation slot no living wingman holds, never
        // its member number, which would pass slot 9 and fail the step.
        let (mut wings, mut targets) = led_wing(4, &[]);
        fly_one_tick(&mut wings, &mut targets, vec![]);
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(1));
        // Member 2 (aircraft 3, slot 2) is shot down; the lead stays.
        targets.iter_mut().find(|t| t.id == 3).unwrap().hp = 0;
        fly_one_tick(&mut wings, &mut targets, vec![]);
        assert!(!wings.mission.actor(3).unwrap().alive());
        wings.insert_actor(new_aircraft(&wings, 30, 12)).unwrap();
        let joined = wings.mission.actor(30).unwrap();
        assert!(!joined.identity().is_leader());
        assert_eq!(joined.wing_slot(), 2, "the dead member's slot");
        // Another with member 13 takes the next free slot, behind the rest.
        wings.insert_actor(new_aircraft(&wings, 31, 13)).unwrap();
        assert_eq!(wings.mission.actor(31).unwrap().wing_slot(), 4);
        let slots: Vec<u8> = [2, 4, 30, 31]
            .map(|id| wings.mission.actor(id).unwrap().wing_slot())
            .to_vec();
        assert_eq!(slots, [1, 3, 2, 4], "no two share a slot");
        targets.push(target(30, [3000., 20000., 0.], 0.));
        targets.push(target(31, [3600., 20000., 0.], 0.));
        targets.sort_by_key(|t| t.id);
        for _ in 0..120 {
            fly_one_tick(&mut wings, &mut targets, vec![]);
        }
    }

    #[test]
    fn new_aircraft_seeds_never_meet_the_missions_or_each_other() {
        // Slice R1: a revival's or respawn's aircraft is seeded by its plane
        // id, apart from every seed the mission's own aircraft get.
        let mut mission = std::collections::BTreeSet::new();
        for side in [launch::Side::Friendly, launch::Side::Enemy] {
            for wing in 0..=u8::MAX {
                for member in 0..=u8::MAX {
                    mission.insert(actor_seed(side, wing, member));
                }
            }
        }
        let new: std::collections::BTreeSet<u64> = (0..10_000).map(new_actor_seed).collect();
        assert_eq!(new.len(), 10_000);
        assert!(new.is_disjoint(&mission));
    }

    #[test]
    fn a_leader_handed_back_to_the_ai_leads_again() {
        let (mut wings, mut targets) = led_wing(3, &[]);
        fly_one_tick(&mut wings, &mut targets, vec![]);
        assert_eq!(wings.mission.wing_leader(FRIENDLY_SIDE, 1), Some(1));
        let removed = wings.remove_actor(1).unwrap();
        assert_eq!(
            wings.mission.wing_leader(FRIENDLY_SIDE, 1),
            Some(1),
            "the aircraft still leads while a human flies it"
        );
        wings
            .insert_actor(ActorInsert::from_removed(removed))
            .unwrap();
        assert_eq!(leads(&wings), [1]);
    }

    #[test]
    fn the_flight_model_setting_puts_every_ai_aircraft_on_the_hybrid_model() {
        let hybrid = |wings: &AiWings| -> Vec<bool> {
            wings
                .mission
                .actors()
                .iter()
                .map(|a| a.flight().research.is_some())
                .collect()
        };
        // Standard is single player as it is: legacy in the air.
        let (mut standard, _) = led_wing(3, &[]);
        assert_eq!(hybrid(&standard), [false; 4]);
        standard.set_flight_model(AiFlightModel::Standard).unwrap();
        assert_eq!(hybrid(&standard), [false; 4]);

        // AllHybrid seeds each aircraft as `--probe-flight-model researched`
        // does, so the two produce the same aircraft.
        let (mut all, mut targets) = led_wing(3, &[]);
        all.set_flight_model(AiFlightModel::AllHybrid).unwrap();
        assert_eq!(hybrid(&all), [true; 4]);
        let (mut probe, _) = led_wing(3, &[]);
        probe.configure_probe(true, None, [0.; 3]).unwrap();
        for id in 1..=4 {
            assert_eq!(
                all.mission.actor(id).unwrap().flight(),
                probe.mission.actor(id).unwrap().flight(),
                "aircraft {id}"
            );
        }
        // It flies, and an aircraft that joins later follows the setting.
        for _ in 0..120 {
            fly_one_tick(&mut all, &mut targets, vec![]);
        }
        assert!(all.mission.actors().iter().all(|a| !a.flight().crashed));
        let removed = all.remove_actor(2).unwrap();
        assert!(removed.parts.flight.research.is_some());
        let mut back = ActorInsert::from_removed(removed);
        back.flight = flight::State::new(&aircraft(), [3000., 20000., 0.]).unwrap();
        all.insert_actor(back).unwrap();
        assert_eq!(hybrid(&all), [true; 4]);

        // Training targets keep their straight-line flight.
        let (mut wings, _) = led_wing(2, &[]);
        wings.mission.actor_mut(2).unwrap().set_dummy();
        wings.set_flight_model(AiFlightModel::AllHybrid).unwrap();
        assert_eq!(hybrid(&wings), [true, false, true]);
    }

    #[test]
    fn infrared_pose_refresh_preserves_rotor_handoff_heat_and_updates_jets() {
        let retained = seeker::Heat::Engine {
            on: true,
            throttle: 0.7,
            afterburner: false,
        };
        for rotor in [true, false] {
            let (mut wings, mut targets) = led_wing_of(1, &[], 3000., || {
                if rotor {
                    crate::test_support::powered_profile(AircraftId::Mi24)
                } else {
                    aircraft()
                }
            });
            targets[0].heat = retained;
            let f = wings.mission.actor_mut(1).unwrap().flight_mut();
            f.engine = true;
            f.throttle = 0.2;
            f.burner = !rotor;
            wings.mirror_pose_out(&mut targets);
            assert_eq!(
                targets[0].heat,
                if rotor {
                    retained
                } else {
                    seeker::Heat::Engine {
                        on: true,
                        throttle: 0.2,
                        afterburner: true,
                    }
                }
            );
        }
    }

    #[test]
    fn a_powered_lift_ai_aircraft_is_trimmed_as_it_joins_the_hybrid_model() {
        // VTOL overhaul decision 8 and acceptance S1: a multiplayer seat is
        // an AI actor until a human takes it, and a revival joins as one, so
        // both begin in trimmed forward flight on every spawn path. Since
        // slice H1 the rotorcraft join it at the build under every AI
        // flight model, and a revival of one joins it whatever the model.
        for id in [
            AircraftId::Ah64,
            AircraftId::Mi24,
            AircraftId::V22,
            AircraftId::Av8,
            AircraftId::Yak141,
        ] {
            let profile = || crate::test_support::powered_profile(id);
            let (mut wings, mut targets) = led_wing_of(1, &[], 3_000., profile);
            let before = wings.mission.actor(1).unwrap().flight().clone();
            let rotorcraft = matches!(id, AircraftId::Ah64 | AircraftId::Mi24 | AircraftId::V22);
            if rotorcraft {
                // Slice H1: the helicopters and the V-22 are built on the
                // hybrid model, trimmed, under every AI flight model.
                assert!(before.research.is_some(), "{id:?}");
                let fresh = flight::State::new(&profile(), before.position).unwrap();
                assert_ne!(before.lift_controls, fresh.lift_controls, "{id:?} trimmed");
                for model in AiFlightModel::ALL {
                    wings.set_flight_model(model).unwrap();
                    assert_eq!(wings.mission.actor(1).unwrap().flight(), &before, "{id:?}");
                }
            } else {
                assert!(before.research.is_none());
                // Standard leaves the start as it is.
                wings.set_flight_model(AiFlightModel::Standard).unwrap();
                assert_eq!(wings.mission.actor(1).unwrap().flight(), &before, "{id:?}");
                wings.set_flight_model(AiFlightModel::AllHybrid).unwrap();
            }
            let flight = wings.mission.actor(1).unwrap().flight().clone();
            assert!(flight.research.is_some());
            if !rotorcraft {
                assert_ne!(flight.lift_controls, before.lift_controls, "{id:?} trimmed");
            }
            assert_eq!(flight.lift_controls.body_rates, [0.; 3]);
            assert!(
                (flight.speed - before.speed).abs() < 1e-9,
                "{id:?} start speed"
            );
            assert!(flight.speed > 60. * 1.687_81, "{id:?} forward flight");
            assert_eq!(flight.nozzle_degrees(), 0.);
            // Hands off for ten seconds it stays where it was put.
            let (height, speed) = (flight.position[1], flight.speed);
            for _ in 0..1200 {
                fly_one_tick(&mut wings, &mut targets, vec![]);
            }
            let after = wings.mission.actor(1).unwrap().flight();
            assert!(!after.crashed, "{id:?}");
            // The wing holds formation with the player's lead, so only the
            // trim itself is judged: no collapse, no climb away.
            assert!(
                (after.position[1] - height).abs() < 300.,
                "{id:?} height {height} to {}",
                after.position[1]
            );
            assert!(after.speed > 0.5 * speed, "{id:?}");

            // An aircraft that joins later (a revival) is trimmed as it joins;
            // a rotorcraft under the Standard model too (slice H1).
            if rotorcraft {
                wings.set_flight_model(AiFlightModel::Standard).unwrap();
            }
            let removed = wings.remove_actor(1).unwrap();
            let mut back = ActorInsert::from_removed(removed);
            back.flight = flight::State::new(&profile(), [3000., 3_000., 0.]).unwrap();
            let fresh = back.flight.lift_controls;
            wings.insert_actor(back).unwrap();
            let joined = wings.mission.actor(1).unwrap().flight();
            assert!(joined.research.is_some());
            assert_ne!(joined.lift_controls, fresh, "{id:?} trimmed on joining");
            assert_eq!(joined.ticks, 0);
        }
    }

    #[test]
    fn a_helicopter_flies_the_vertical_landing_and_ignores_a_home_out_of_reach() {
        // Slice ST-H (fuel states since slice H3, HF2). The runway is 395 nm
        // away: a helicopter, whose tank lasts about two hours at the hover
        // flow against the 2.8 hours the way takes at its cruise, has no
        // fuel state to manage (the flight home is out of reach). Told to
        // land, a jet
        // begins its landing and, since slice H2, so does a helicopter: its
        // vertical one.
        use tore_sim::ai::route::FuelState;
        let runway = runway_view(9, [0., 10., 2_400_000.]);
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![HomeRunway {
                view: runway,
                friendly: true,
                enemy: true,
            }],
            departure: None,
            enemy_departure: None,
        };
        let selections = [
            (launch::Side::Friendly, 1u8, 1usize),
            (launch::Side::Enemy, 0, 1),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        for helicopter in [true, false] {
            let targets: Vec<_> = vec![
                target(1, [600., 3_000., 0.], 0.),
                target(2, [0., 3_000., 900_000.], std::f64::consts::PI),
            ];
            let mut wings = AiWings::build_for(&payload, &targets, &airfields, &[], |_| {
                Ok((
                    if helicopter {
                        crate::test_support::powered_profile(AircraftId::Mi24)
                    } else {
                        aircraft()
                    },
                    None,
                ))
            })
            .unwrap();
            wings.apply_mission_preset(Preset::Hold, [0., 3_000., 0.]);
            wings.set_flight_model(AiFlightModel::AllHybrid).unwrap();
            let mut targets = targets;
            for _ in 0..120 {
                fly_one_tick(&mut wings, &mut targets, vec![]);
            }
            if helicopter {
                assert_eq!(
                    wings.mission.actor(1).unwrap().controller().fuel_state(),
                    Some(FuelState::NoManagement),
                    "out of reach"
                );
            }
            let order = tore_sim::ai::airfield::LandingOrder {
                runway,
                reason: tore_sim::ai::airfield::LandingReason::Ordered,
            };
            wings
                .mission
                .actor_mut(1)
                .unwrap()
                .order(tore_sim::ai::wing::WingRequest::Land(order), 0)
                .unwrap();
            let mut entered = None;
            for _ in 0..1_800 {
                fly_one_tick(&mut wings, &mut targets, vec![]);
                if let Some(sequence) = wings.mission.actor(1).unwrap().airfield() {
                    entered.get_or_insert(sequence.lift());
                }
            }
            let actor = wings.mission.actor(1).unwrap();
            assert!(!actor.flight().crashed);
            let lift = if helicopter {
                tore_sim::ai::airfield::Lift::Rotor
            } else {
                tore_sim::ai::airfield::Lift::Conventional
            };
            assert_eq!(entered, Some(lift), "helicopter {helicopter}");
        }
    }

    /// A friendly Mi-24 (3,307 lb, planned at its cruise flow, about two
    /// hours) with a home runway `home_ft` away, airborne and holding, and an enemy.
    fn fuel_wings(home_ft: f64) -> (AiWings, Vec<live::Target>) {
        let runway = runway_view(9, [0., 10., home_ft]);
        let airfields = Airfields {
            patrol_bounds: None,
            runways: vec![HomeRunway {
                view: runway,
                friendly: true,
                enemy: true,
            }],
            departure: None,
            enemy_departure: None,
        };
        let selections = [
            (launch::Side::Friendly, 1u8, 1usize),
            (launch::Side::Enemy, 0, 1),
        ]
        .map(|(side, index, count)| WingSelection {
            wing: WingId::new(side, index).unwrap(),
            aircraft: AircraftId::F18,
            count,
            skill_level: 1,
        });
        let payload = resolve_wings(&selections, None).unwrap();
        let targets: Vec<_> = vec![
            target(1, [600., 3_000., 0.], 0.),
            target(2, [0., 3_000., 900_000.], std::f64::consts::PI),
        ];
        let mut wings = AiWings::build_for(&payload, &targets, &airfields, &[], |_| {
            Ok((crate::test_support::powered_profile(AircraftId::Mi24), None))
        })
        .unwrap();
        wings.apply_mission_preset(Preset::Hold, [0., 3_000., 0.]);
        wings.set_flight_model(AiFlightModel::AllHybrid).unwrap();
        (wings, targets)
    }

    #[test]
    fn a_helicopter_has_a_fuel_schedule_at_its_own_cruise_and_goes_home_on_bingo() {
        // Slice H3. Home is 16 nm away (100,000 ft). The Mi-24's cruise is
        // 80 percent of its 178 kt top, 240 ft/s: 417 s home.
        use tore_sim::ai::controller::Activity;
        use tore_sim::ai::route::FuelState;
        let (mut wings, mut targets) = fuel_wings(100_000.);
        fly_one_tick(&mut wings, &mut targets, vec![]);
        let actor = wings.mission.actor(1).unwrap();
        let fuel = actor.controller().trace().fuel.expect("a fuel judgement");
        let home = fuel.time_home_s.expect("a time home");
        // 80 percent of the envelope's top at 3,000 ft (a little under the
        // sea-level 178 kt): within 5 percent of the sea-level figure.
        let sea_level = 100_000. / (178. * 1.687_81 * 0.8);
        assert!((home / sea_level - 1.).abs() < 0.05, "home {home}");
        // Slices HF2, HF3: it plans with its rotor's cruise flow, not the
        // record's 2 lb/s: about two hours, not 27 minutes.
        let flight = wings.mission.actor(1).unwrap().flight();
        let flow = flight.endurance_flow_lbs_per_second();
        assert!(flow < 0.7, "cruise flow {flow}");
        assert!(
            (fuel.endurance_s - flight.fuel / flow).abs() < 5.,
            "{}",
            fuel.endurance_s
        );
        assert!(fuel.endurance_s > 100. * 60., "{}", fuel.endurance_s);
        assert_eq!(fuel.state, Some(FuelState::Ok), "not bingo at the start");
        // The reserve is five minutes, the fighters' (slice HF2): bingo
        // under the time home plus 300 s of fuel, caution above it.
        for (margin, expected) in [(350., FuelState::Caution), (250., FuelState::Bingo)] {
            // The planning flow falls with the weight, so settle the tank that
            // lasts `home + margin` at the flow it then burns.
            for _ in 0..8 {
                let now = wings.mission.actor(1).unwrap().flight();
                let at = now.endurance_flow_lbs_per_second();
                wings
                    .mission
                    .actor_mut(1)
                    .unwrap()
                    .set_internal_fuel(at * (home + margin));
            }
            // The judgement is renewed every few seconds.
            let mut state = None;
            for _ in 0..1_200 {
                fly_one_tick(&mut wings, &mut targets, vec![]);
                state = wings.mission.actor(1).unwrap().controller().fuel_state();
                if state == Some(expected) {
                    break;
                }
            }
            assert_eq!(state, Some(expected), "{margin} s over the time home");
        }
        // Short of the reserve it is bingo and turns for home at its cruise.
        wings
            .mission
            .actor_mut(1)
            .unwrap()
            .set_internal_fuel(flow * (home + 100.));
        let start = wings.mission.actor(1).unwrap().flight().position;
        let away = |wings: &AiWings| {
            let at = wings.mission.actor(1).unwrap().flight().position;
            (at[0] - 0.).hypot(at[2] - 100_000.)
        };
        let before = away(&wings);
        let mut state = None;
        for _ in 0..6_000 {
            fly_one_tick(&mut wings, &mut targets, vec![]);
            let actor = wings.mission.actor(1).unwrap();
            state = actor.controller().fuel_state();
            if state == Some(FuelState::Bingo) {
                break;
            }
        }
        assert_eq!(state, Some(FuelState::Bingo));
        for _ in 0..7_200 {
            fly_one_tick(&mut wings, &mut targets, vec![]);
        }
        let actor = wings.mission.actor(1).unwrap();
        assert!(!actor.flight().crashed);
        assert_eq!(actor.activity(), Activity::ReturningToBase);
        assert!(
            away(&wings) < before - 3_000.,
            "{} from {before}",
            away(&wings)
        );
        // At about its cruise, not the fighters' 80 knots.
        let kt = actor.flight().speed / 1.687_81;
        assert!(kt > 100. && kt < 150., "{kt} kt, from {start:?}");
    }

    #[test]
    fn a_helicopter_under_a_missile_warning_breaks_across_the_threat_and_keeps_flying() {
        use tore_sim::combat::missiles::Guidance;
        // Slice ST-H: a warned helicopter flies its own break, not the
        // fighters' jink and dive at corner speed, which sank it.
        for id in [AircraftId::Ah64, AircraftId::Mi24] {
            let profile = || crate::test_support::powered_profile(id);
            let (mut wings, mut targets) = led_wing_of(1, &[], 3_000., profile);
            wings.set_flight_model(AiFlightModel::AllHybrid).unwrap();
            assert!(wings.mission.actor(1).unwrap().is_rotorcraft(), "{id:?}");
            let start = wings.mission.actor(1).unwrap().flight().clone();
            let mut seen = None;
            let (mut lowest, mut slowest) = (f64::MAX, f64::MAX);
            for tick in 0..2_400 {
                // A radar missile from the north-east, always about 5,000 ft
                // out and closing (the fixture renews it each tick).
                let at = wings.mission.actor(1).unwrap().flight().position;
                wings
                    .mission
                    .set_missiles(vec![tore_sim::combat::threats::MissileSnapshot {
                        id: 99,
                        owner: 90,
                        position: [at[0] + 3_500., at[1], at[2] + 3_500.],
                        velocity: [-1_060., 0., -1_060.],
                        guidance: Guidance::Supported,
                        target: Some(1),
                        radar_active: false,
                        radar_acquired: false,
                        supported: true,
                        supporting_radar_position: Some([at[0] + 60_000., 10., at[2] + 60_000.]),
                        alive: true,
                    }]);
                fly_one_tick(&mut wings, &mut targets, vec![]);
                let actor = wings.mission.actor(1).unwrap();
                assert!(!actor.flight().crashed, "{id:?} crashed at tick {tick}");
                lowest = lowest.min(actor.flight().position[1]);
                slowest = slowest.min(actor.flight().speed);
                if seen.is_none() && actor.trace().rotor_break.is_some() {
                    seen = actor.trace().rotor_break;
                }
            }
            let (answer, bearing) = seen.unwrap_or_else(|| panic!("{id:?} never broke"));
            // The bearing is the missile's, north-east; the break is 90
            // degrees off it, on the side nearer the nose.
            let bearing = bearing.expect("a radar warning has a bearing");
            assert!((bearing - 45.).abs() < 10., "{id:?} bearing {bearing}");
            let off = (answer.heading_deg - bearing + 540.) % 360. - 180.;
            assert!(
                (off.abs() - answer.offset_deg).abs() < 1e-6,
                "{id:?} {answer:?}"
            );
            assert_eq!(answer.offset_deg, 90.);
            let actor = wings.mission.actor(1).unwrap();
            let flight = actor.flight();
            // It neither dived nor stalled: it is where a break leaves it,
            // turned toward the beam, high and fast enough.
            assert!(
                lowest > start.position[1] - 1_200.,
                "{id:?} lowest {lowest}"
            );
            assert!(slowest > 0.7 * start.speed, "{id:?} slowest {slowest}");
            assert!(flight.position[1] > 1_000., "{id:?} {}", flight.position[1]);
            let turned = {
                let heading = flight.yaw.to_degrees().rem_euclid(360.);
                let beam = (bearing + 270.) % 360.;
                (heading - beam + 540.) % 360. - 180.
            };
            assert!(
                turned.abs() < 40.,
                "{id:?} heading is {turned} off the beam"
            );
        }
    }

    #[test]
    fn flight_model_names_round_trip_and_reject_unknown_values() {
        for model in AiFlightModel::ALL {
            assert_eq!(model.name().parse(), Ok(model));
        }
        assert_eq!("ALL_HYBRID".parse(), Ok(AiFlightModel::AllHybrid));
        assert_eq!(AiFlightModel::default(), AiFlightModel::Standard);
        assert!("everything-legacy".parse::<AiFlightModel>().is_err());
    }

    pub(super) fn player_object(position: Vector) -> WorldObject {
        WorldObject {
            infrared_strength: 1.,
            id: PLAYER_ID,
            side: FRIENDLY_SIDE,
            position,
            velocity: [0., 0., 800.],
            heading_deg: 0.,
            pitch_deg: 0.,
            speed: ScalarSpeed(800.),
            maximum_speed: ScalarSpeed(1600.),
            is_aircraft: true,
            is_fighter: true,
            human_controlled: true,
            alive: true,
            destroyed: false,
            on_ground: false,
            observable: None,
        }
    }

    fn run(wings: &mut AiWings, targets: &mut [live::Target], ticks: usize) {
        for _ in 0..ticks {
            wings
                .advance(player_object([0., 20000., -5000.]), targets, &flat)
                .unwrap();
        }
    }

    #[test]
    fn formation_trace_records_controls_without_changing_flight() {
        let selections = [WingSelection {
            wing: WingId::new(launch::Side::Friendly, 0).unwrap(),
            aircraft: AircraftId::F18,
            count: 2,
            skill_level: 2,
        }];
        let payload = resolve_wings(&selections, None).unwrap();
        let targets = vec![
            target(1, [512., 20000., -512.], 0.),
            target(2, [-512., 20000., -512.], 0.),
        ];
        let build =
            || AiWings::build_with(&payload, &targets, 0, |_| Ok((aircraft(), None))).unwrap();
        let mut logged = build();
        let mut plain = build();
        assert!(logged.take_formation_trace().rows.is_empty());
        logged.set_formation_trace(true);
        let mut a = targets.clone();
        let mut b = targets;
        let mut rows = Vec::new();
        let mut flushes = Vec::new();
        for tick in 0..120 {
            let player = player_object([0., 20000., 800. * tick as f64 / 120.]);
            logged.advance(player.clone(), &mut a, &flat).unwrap();
            plain.advance(player, &mut b, &flat).unwrap();
            for id in 1..=2 {
                assert_eq!(
                    logged.mission.actor(id).unwrap().flight(),
                    plain.mission.actor(id).unwrap().flight()
                );
            }
            let batch = logged.take_formation_trace();
            if batch.flush {
                flushes.push(tick);
            }
            rows.extend(batch.rows);
        }
        assert_eq!(rows.len(), 20);
        assert_eq!(flushes, [119], "flushed on the whole-second tick");
        for row in &rows {
            assert_eq!(row.yaw_input, 0., "formation commands no rudder");
            assert_eq!(row.tick % 12, 0);
        }
        // With the trace off nothing is kept, and nothing changes.
        logged.set_formation_trace(false);
        logged
            .advance(player_object([0., 20000., 800.]), &mut a, &flat)
            .unwrap();
        assert_eq!(logged.take_formation_trace(), FormationBatch::default());
    }

    #[test]
    fn formation_trace_rows_are_bounded_when_nobody_drains_them() {
        let selections = [WingSelection {
            wing: WingId::new(launch::Side::Friendly, 0).unwrap(),
            aircraft: AircraftId::F18,
            count: 2,
            skill_level: 2,
        }];
        let payload = resolve_wings(&selections, None).unwrap();
        let mut targets = vec![
            target(1, [512., 20000., -512.], 0.),
            target(2, [-512., 20000., -512.], 0.),
        ];
        let mut wings =
            AiWings::build_with(&payload, &targets, 0, |_| Ok((aircraft(), None))).unwrap();
        wings.set_formation_trace(true);
        run(&mut wings, &mut targets, 12 * 2200);
        let batch = wings.take_formation_trace();
        assert_eq!(batch.rows.len(), FORMATION_TRACE_MAX_ROWS);
    }

    #[test]
    fn the_setup_draft_becomes_actors_with_the_right_sides_wings_and_members() {
        let (wings, _) = build(None);
        assert_eq!(wings.len(), 4);
        let labels: Vec<String> = wings.slots().iter().map(Slot::label).collect();
        assert_eq!(
            labels,
            ["Friendly 2-1", "Friendly 2-2", "Enemy 1-1", "Enemy 1-2"]
        );
        let ids: Vec<u32> = wings.slots().iter().map(|s| s.id).collect();
        assert_eq!(ids, [1, 2, 3, 4], "actor ids must be the target ids");
        for slot in wings.slots() {
            let actor = wings.mission().actor(slot.id).unwrap();
            let identity = actor.identity();
            assert_eq!(identity.side, side_of(slot.side));
            assert_eq!(identity.wing + 1, slot.wing_number);
            assert_eq!(identity.member + 1, slot.member_number);
            assert!(!identity.human_controlled, "no AI actor may be the player");
        }
        // The player and every friendly wing share one side, so friendly AI
        // cannot select the human as a target.
        assert_eq!(side_of(launch::Side::Friendly), FRIENDLY_SIDE);
        assert_ne!(side_of(launch::Side::Enemy), FRIENDLY_SIDE);
    }

    #[test]
    fn the_enemy_skill_override_reaches_the_actors_resolved_experience() {
        let (plain, _) = build(None);
        for slot in plain.slots() {
            let resolved = plain
                .mission()
                .actor(slot.id)
                .unwrap()
                .controller()
                .experience();
            let expected = match slot.side {
                launch::Side::Friendly => Experience::Average,
                launch::Side::Enemy => Experience::Ace,
            };
            assert_eq!(resolved.level, expected);
            assert!(matches!(
                resolved.origin,
                ExperienceOrigin::QuickMission { .. }
            ));
        }
        let (forced, _) = build(Some(EnemySkillOverride::AllNovice));
        for slot in forced.slots() {
            let resolved = forced
                .mission()
                .actor(slot.id)
                .unwrap()
                .controller()
                .experience();
            match slot.side {
                // Friendly wings are never touched by the enemy preference.
                launch::Side::Friendly => {
                    assert_eq!(resolved.level, Experience::Average);
                    assert!(matches!(
                        resolved.origin,
                        ExperienceOrigin::QuickMission { .. }
                    ));
                }
                launch::Side::Enemy => {
                    assert_eq!(resolved.level, Experience::Novice);
                    assert_eq!(resolved.origin, ExperienceOrigin::EnemyOverride);
                }
            }
        }
        let (average, _) = build(Some(EnemySkillOverride::AllAverage));
        let enemy = average.slots().iter().find(|s| s.side.is_enemy()).unwrap();
        assert_eq!(
            average
                .mission()
                .actor(enemy.id)
                .unwrap()
                .controller()
                .experience()
                .level,
            Experience::Average
        );
    }

    #[test]
    fn enemy_ai_cheat_changes_enemy_skill_live_and_unchanged_restores_it() {
        let (mut wings, _) = build(None);
        let skill =
            |wings: &AiWings, id| wings.mission().actor(id).unwrap().controller().experience();
        wings.set_enemy_skill(Some(Experience::Novice));
        for slot in wings.slots().to_vec() {
            let resolved = skill(&wings, slot.id);
            match slot.side {
                launch::Side::Friendly => assert_eq!(resolved.level, Experience::Average),
                launch::Side::Enemy => {
                    assert_eq!(resolved.level, Experience::Novice);
                    assert_eq!(resolved.origin, ExperienceOrigin::EnemyOverride);
                }
            }
        }
        wings.set_enemy_skill(Some(Experience::Average));
        wings.set_enemy_skill(None);
        for slot in wings.slots().to_vec() {
            let resolved = skill(&wings, slot.id);
            assert!(matches!(
                resolved.origin,
                ExperienceOrigin::QuickMission { .. }
            ));
            if slot.side.is_enemy() {
                assert_eq!(resolved.level, Experience::Ace, "mission skill restored");
            }
        }
    }

    #[test]
    fn guns_only_cheat_inhibits_every_other_store_and_restores_them() {
        let (mut wings, _) = build(None);
        let slot = wings.slots()[0];
        let stations = wings.mission.actor(slot.id).unwrap().stations().to_vec();
        assert!(stations.len() > 1);
        let mut gun = combat_fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        gun.source = slot.aircraft.gun().unwrap().into();
        wings.weapons.insert((slot.id, stations[0].station.0), gun);
        let inhibited = |wings: &AiWings| -> Vec<bool> {
            wings
                .mission
                .actor(slot.id)
                .unwrap()
                .stations()
                .iter()
                .map(|s| s.store.inhibited)
                .collect()
        };
        wings.set_guns_only(true);
        let on = inhibited(&wings);
        assert!(!on[0], "the gun stays usable");
        assert!(on[1..].iter().all(|i| *i));
        wings.set_guns_only(false);
        assert!(inhibited(&wings).iter().all(|i| !*i));
    }

    /// With `--fixture-wings` this bridge is absent, so a target row only
    /// ever receives the fixture integration. This test states that rule and
    /// the next one shows the bridge breaking it, which is the whole point of
    /// the flag.
    #[test]
    fn the_fixture_path_is_a_straight_line_when_the_bridge_is_absent() {
        let mut targets = spawned();
        let start: Vec<Vector> = targets.iter().map(|t| t.position).collect();
        let velocities: Vec<Vector> = targets.iter().map(|t| t.velocity).collect();
        for _ in 0..600 {
            for t in targets.iter_mut().filter(|t| t.hp > 0) {
                for i in 0..3 {
                    t.position[i] += t.velocity[i] / FIXTURE_TICK_RATE;
                }
            }
        }
        for ((target, from), velocity) in targets.iter().zip(&start).zip(&velocities) {
            for i in 0..3 {
                let expected = from[i] + velocity[i] * 600. / FIXTURE_TICK_RATE;
                assert!(
                    (target.position[i] - expected).abs() < 1e-6,
                    "fixture drifted from its straight line"
                );
            }
            assert_eq!(
                target.velocity, *velocity,
                "fixture velocity must not change"
            );
            assert_eq!(
                target.basis.angles(),
                Basis::new(
                    if velocity[2] < 0. {
                        std::f64::consts::PI
                    } else {
                        0.
                    },
                    0.,
                    0.
                )
                .angles(),
                "fixture attitude must not change"
            );
        }
    }

    #[test]
    fn ai_aircraft_move_and_leave_the_straight_line_fixture_path() {
        let (mut wings, mut targets) = build(None);
        let start = wings.positions();
        let fixture: Vec<Vector> = targets
            .iter()
            .map(|t| {
                std::array::from_fn(|i| t.position[i] + t.velocity[i] * 600. / FIXTURE_TICK_RATE)
            })
            .collect();
        run(&mut wings, &mut targets, 600);
        let after = wings.positions();
        for (index, (from, to)) in start.iter().zip(&after).enumerate() {
            let moved: f64 = (0..3)
                .map(|i| (to[i] - from[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(moved > 1000., "actor {index} barely moved: {moved} feet");
            let straight: f64 = (0..3)
                .map(|i| (to[i] - fixture[index][i]).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                straight > 100.,
                "actor {index} flew the fixture line: {straight} feet apart"
            );
        }
        // The combat world sees the AI pose, not the fixture pose.
        for (slot, target) in wings.slots().iter().zip(&targets) {
            let actor = wings.mission().actor(slot.id).unwrap();
            assert_eq!(target.position, actor.flight().position);
            assert_eq!(target.velocity, actor.flight().velocity);
        }
    }

    #[test]
    fn two_runs_with_the_same_inputs_are_identical() {
        let (mut first, mut first_targets) = build(None);
        let (mut second, mut second_targets) = build(None);
        run(&mut first, &mut first_targets, 900);
        run(&mut second, &mut second_targets, 900);
        assert_eq!(first.positions(), second.positions());
        assert_eq!(first.probe_lines(), second.probe_lines());
    }

    #[test]
    fn every_actor_gets_its_own_seed() {
        let mut seeds = Vec::new();
        for side in [launch::Side::Friendly, launch::Side::Enemy] {
            for wing in 0..3u8 {
                for member in 0..5u8 {
                    seeds.push(actor_seed(side, wing, member));
                }
            }
        }
        let mut sorted = seeds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seeds.len(),
            "two actors share a decision seed"
        );
    }

    /// A destroyed target row stops its actor, and a damaged one is reported as
    /// a hit exactly once per hit point drop.
    #[test]
    fn ejection_mirrors_a_surviving_pilot_once_but_never_revives_a_killed_pilot() {
        let mut targets = spawned();
        let mut wings = AiWings::build_with(&payload(None), &targets, 0, |_| {
            let mut profile = aircraft();
            profile.fields.get_mut("flags").unwrap().value = "16".into();
            Ok((profile, None))
        })
        .unwrap();
        targets[0].hp = 0;
        targets[1].hp = 0;
        targets[1].localized_damage.structural_section = Some(live::DamageSection::Cockpit);
        run(&mut wings, &mut targets, 1200);
        assert!(wings.mission.actor(1).unwrap().flight().escape.is_some());
        assert!(!wings.mission.actor(1).unwrap().alive());
        assert!(wings.mission.actor(2).unwrap().flight().systems.pilot.dead);
        assert!(wings.mission.actor(2).unwrap().flight().escape.is_none());
        assert_eq!(
            wings
                .ejection_events
                .iter()
                .filter(|(id, _, _)| *id == 1)
                .count(),
            1
        );
        assert!(
            wings
                .ejection_events
                .iter()
                .find(|(id, _, _)| *id == 1)
                .unwrap()
                .2
        );
        assert_eq!(targets[0].hp, 0);
        assert_eq!(wings.escapees().count(), 1);
    }

    /// The same rule as the player's for every two-seater (agent decision,
    /// 2026-10-06): an AI two-seater that ejects sends both crew, the second
    /// first, and the radio hears one ejection.
    #[test]
    fn an_ai_two_seater_ejects_both_crew_and_a_single_seater_one() {
        for (flags, crew) in [("16", 0), ("20", 1)] {
            let mut targets = spawned();
            let mut wings = AiWings::build_with(&payload(None), &targets, 0, |_| {
                let mut profile = aircraft();
                profile.fields.get_mut("flags").unwrap().value = flags.into();
                Ok((profile, None))
            })
            .unwrap();
            targets[0].hp = 0;
            targets[1].hp = 0;
            targets[1].localized_damage.structural_section = Some(live::DamageSection::Cockpit);
            run(&mut wings, &mut targets, 1200);
            assert_eq!(wings.escapees().count(), 1, "flags {flags}");
            assert_eq!(wings.crew_escapees().count(), crew, "flags {flags}");
            assert_eq!(
                wings
                    .ejection_events
                    .iter()
                    .filter(|(id, _, _)| *id == 1)
                    .count(),
                1,
                "one ejection call for the aircraft, whatever its crew"
            );
            if crew == 1 {
                let (owner, second) = wings.crew_escapees().next().unwrap();
                let (_, pilot) = wings.escapees().next().unwrap();
                assert_eq!(owner, 1);
                assert!(second.ticks > pilot.ticks, "the second seat left first");
            }
        }
    }

    #[test]
    fn an_ai_aircraft_105_miles_past_the_map_is_lost_and_credits_nobody() {
        let (mut wings, mut targets) = build(None);
        run(&mut wings, &mut targets, 10);
        let mut ledger = tore_sim::combat::ledger::Ledger::default();
        // Enemy 3 was hit by the player earlier.
        ledger.damaged(tore_sim::combat::ledger::Kill {
            owner: 0,
            victim: 3,
            category: 0x8000,
            aircraft: true,
        });
        let far = wings.mission().actor(3).unwrap().flight().position;
        // Everyone is inside the map except actor 3.
        let lost = wings.lose_out_of_bounds(&mut ledger, |x, z| {
            if (x, z) == (far[0], far[2]) {
                105.
            } else {
                99.9
            }
        });
        assert!(lost.is_empty() || lost == [3], "{lost:?}");
        assert_eq!(lost, [3]);
        assert_eq!(
            wings
                .mission()
                .actor(3)
                .unwrap()
                .flight()
                .systems
                .structure
                .cause,
            Some(tore_sim::aircraft_systems::LossCause::OutOfBounds)
        );
        assert!(wings.mission().actor(2).unwrap().alive());
        assert_eq!(ledger.credit(3), None);
        // The next step finishes it: the target row is zero and the actor is lost.
        run(&mut wings, &mut targets, 2);
        assert_eq!(targets[2].hp, 0);
        assert!(!wings.mission().actor(3).unwrap().alive());
        // Nothing is lost twice.
        assert!(
            wings
                .lose_out_of_bounds(&mut ledger, |_, _| 200.)
                .iter()
                .all(|id| *id != 3)
        );
    }

    #[test]
    fn an_ai_aircraft_finished_by_overspeed_or_the_map_edge_credits_nobody() {
        use tore_sim::aircraft_systems::LossCause;
        use tore_sim::combat::ledger::{Kill, Ledger};
        // Enemy 3 was shot at by the player earlier in every case.
        let hit = Kill {
            owner: 0,
            victim: 3,
            category: 0x8000,
            aircraft: true,
        };
        for cause in [LossCause::Overspeed, LossCause::OutOfBounds] {
            let (mut wings, mut targets) = build(None);
            run(&mut wings, &mut targets, 10);
            let mut ledger = Ledger::default();
            ledger.damaged(hit);
            let flight = wings.mission.actor_mut(3).unwrap().flight_mut();
            flight.systems.destroy(cause);
            flight.crashed = true;
            run(&mut wings, &mut targets, 2);
            assert_eq!(targets[2].hp, 0, "{cause:?}");
            assert_eq!(wings.lose_uncredited(&mut ledger), [3], "{cause:?}");
            assert_eq!(ledger.credit(3), None, "{cause:?}");
            // Nothing is recorded twice, and the others are untouched.
            assert!(wings.lose_uncredited(&mut ledger).is_empty());
            assert!(targets.iter().filter(|t| t.hp == 0).count() == 1);
        }
        // An ordinary crash with no cause of its own still goes to the last shooter.
        let (mut wings, mut targets) = build(None);
        run(&mut wings, &mut targets, 10);
        let mut ledger = Ledger::default();
        ledger.damaged(hit);
        wings.mission.actor_mut(3).unwrap().flight_mut().crashed = true;
        run(&mut wings, &mut targets, 2);
        assert!(wings.lose_uncredited(&mut ledger).is_empty());
        assert_eq!(ledger.credit(3), Some(hit));
    }

    #[test]
    fn damage_flows_from_the_combat_world_into_the_actors() {
        let (mut wings, mut targets) = build(None);
        run(&mut wings, &mut targets, 10);
        assert!(wings.mission().actor(3).unwrap().alive());
        targets[2].hp = 0;
        run(&mut wings, &mut targets, 1);
        assert!(!wings.mission().actor(3).unwrap().alive());
        run(&mut wings, &mut targets, 5);
        // A dead actor is frozen: the bridge must not keep flying a wreck.
        let resting = wings.mission().actor(3).unwrap().flight().position;
        run(&mut wings, &mut targets, 60);
        assert_eq!(wings.mission().actor(3).unwrap().flight().position, resting);
    }

    /// AI aircraft always take Realistic damage: the faults combat rolled on
    /// a target reach the aircraft's flight systems, a station fault keeps
    /// that store out through Air combat guns only, a radar fault blinds it,
    /// and a fatal failure loses the aircraft to combat.
    #[test]
    fn system_faults_reach_the_ai_aircraft_and_a_fatal_one_loses_it() {
        let (mut wings, mut targets) = build(None);
        run(&mut wings, &mut targets, 1);
        let id = targets[2].id;
        let config = combat_fixture(false).own().configuration().clone();
        wings.configs.insert(id, config);
        let actor = wings.mission.actor_mut(id).unwrap();
        let mut stations = simple_stations(4, 0, AI_STORE_SPEED);
        stations[0].station = tore_sim::ai::weapon_service::StationId(0);
        actor.set_stations(stations);
        assert!(actor.flight().radar || actor.sensors().is_none());
        targets[2].faults.counts[5] = 1; // engine power reduced
        targets[2].faults.counts[36] = 1; // hardpoint 0: station 0
        targets[2].faults.counts[37] = 1; // hardpoint 1: radar
        run(&mut wings, &mut targets, 1);
        let actor = wings.mission.actor(id).unwrap();
        assert_eq!(actor.flight().systems.counts[5], 1);
        assert!(actor.flight().systems.power_available() < 1.);
        assert!(actor.equipment().radar);
        assert!(!actor.flight().radar);
        assert!(actor.stations()[0].store.inhibited);
        wings.set_guns_only(true);
        wings.set_guns_only(false);
        assert!(
            wings.mission.actor(id).unwrap().stations()[0]
                .store
                .inhibited
        );
        // Delivered once: the counts do not grow on later ticks.
        run(&mut wings, &mut targets, 2);
        assert_eq!(
            wings.mission.actor(id).unwrap().flight().systems.counts[5],
            1
        );
        assert!(targets[2].hp > 0);
        targets[2].faults.counts[26] = 1; // wing destroyed
        run(&mut wings, &mut targets, 2);
        assert_eq!(targets[2].hp, 0);
        assert!(!wings.mission.actor(id).unwrap().alive());
        // Other aircraft fly on.
        assert!(targets.iter().filter(|t| t.hp > 0).count() > 0);
    }

    /// B47: the report goes to the aircraft the missile is aimed at and to
    /// nobody else, wingmen included.
    #[test]
    fn a_threat_report_reaches_only_the_aimed_at_aircraft() {
        let (mut wings, mut targets) = build(None);
        run(&mut wings, &mut targets, 1);
        let shot = live::Projectile {
            id: 7,
            owner: 1,
            weapon: None,
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: None,
            station: 0,
            position: [0., 20000., 30000.],
            previous: [0., 20000., 30000.],
            direction: [0., 0., 1.],
            speed_f8: 2000 * 256,
            launched_t: 0,
            target: Some(3),
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        };
        wings.report_threats(std::slice::from_ref(&shot), |_| Some(SeekerClass::Radar));
        // Only actor 3 was told; the others, its wingman included, were not.
        assert_eq!(
            wings.threat_reports(),
            [(3, 7)],
            "the warning was broadcast"
        );
        // The same projectile is never reported twice.
        wings.report_threats(std::slice::from_ref(&shot), |_| Some(SeekerClass::Radar));
        assert_eq!(wings.threat_reports(), [(3, 7)]);
    }

    #[test]
    fn the_activity_line_is_rate_limited() {
        let (mut wings, _) = build(None);
        wings.announce(&[(1, Activity::Attacking)]);
        assert_eq!(
            wings.take_message().as_deref(),
            Some("Friendly 2-1: Attacking")
        );
        // A second change in the same window is dropped rather than replacing
        // the line the player is still reading.
        wings.announce(&[(2, Activity::Defending)]);
        assert!(wings.take_message().is_none());
        // Repeating the same activity is never a change.
        wings.announce(&[(1, Activity::Attacking)]);
        assert!(wings.take_message().is_none());
    }

    #[test]
    fn the_activity_line_catches_up_with_its_own_aircraft() {
        // Battery finding (2026-09-28): the line kept saying "Defending" for
        // an aircraft destroyed a second later, because the change was dropped.
        let (mut wings, _) = build(None);
        wings.announce(&[(1, Activity::Defending)]);
        assert_eq!(
            wings.take_message().as_deref(),
            Some("Friendly 2-1: Defending")
        );
        wings.announce(&[(1, Activity::Destroyed)]);
        assert!(wings.take_message().is_none());
        // The interval passes (0 reads as "no line posted yet").
        wings.last_message_tick = 0;
        wings.announce(&[]);
        assert_eq!(
            wings.take_message().as_deref(),
            Some("Friendly 2-1: Destroyed")
        );
        // Another aircraft's dropped change is not revived.
        wings.announce(&[(2, Activity::Defending)]);
        assert!(wings.take_message().is_none());
    }
    #[test]
    fn destroyed_airframe_falls_through_combat_bridge_order_and_restart() {
        for _restart in 0..2 {
            let (mut wings, targets) = build(None);
            let mut combat = combat_fixture(false);
            combat.targets = targets;
            combat.targets[2].position[1] = 1000.0;
            combat.targets[2].hp = 0;
            let player = flight::State::new(&aircraft(), [0.0, 20000.0, -5000.0]).unwrap();
            let mut previous = 1000.0;
            for tick in 0..1500 {
                combat.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: crate::combat::launcher(&player),
                    }],
                    flat,
                );
                wings
                    .advance(player_object(player.position), &mut combat.targets, &flat)
                    .unwrap();
                let wreck = &combat.targets[2];
                assert!(wreck.position[1] <= previous, "wreck rose on tick {tick}");
                if tick == 120 {
                    assert!(wreck.position[1] < 990.0);
                }
                previous = wreck.position[1];
            }
            assert_eq!(combat.targets[2].position[1], 0.0);
            assert_eq!(combat.targets[2].velocity, [0.0; 3]);
            assert!(!wings.mission.actor(3).unwrap().alive());
        }
    }

    #[test]
    fn realised_gun_keeps_its_own_record_and_never_becomes_a_player_missile() {
        use tore_sim::ai::weapon_service::{RequestId, StationId};
        let (mut wings, _) = build(None);
        let mut combat = combat_fixture(true);
        let mut gun = combat_fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        gun.source = "SYNTHETIC-GUN.JT".into();
        let event = LaunchEvent {
            actor: 3,
            station: StationId(1),
            target: 0,
            request_id: RequestId(1),
            projectiles: 10,
        };
        let player = flight::State::new(&aircraft(), [0.0, 20000.0, -5000.0]).unwrap();
        wings.realise(
            &event,
            &mut combat.projectiles,
            &gun,
            1,
            &[(PLAYER_ID, player.position)],
            None,
        );
        assert_eq!(combat.projectiles.len(), 10);
        assert!(
            combat
                .projectiles
                .iter()
                .all(|p| p.weapon.as_ref() == Some(&gun) && p.target.is_none())
        );
        // Station 1 does not even exist on the player. Stepping must use the owned record.
        combat.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: crate::combat::launcher(&player),
            }],
            flat,
        );
        assert!(
            combat.projectiles.iter().all(|p| p
                .weapon(combat.own().configuration())
                .seeker
                .signature
                == 0)
        );
    }

    #[test]
    fn physical_gun_debits_only_emitted_rounds_and_respects_barrel_and_store_gates() {
        use tore_sim::ai::{
            gunnery,
            weapon_service::{RequestId, Rounds, StationId},
            wing::{TargetOrder, WingRequest},
        };
        let (mut wings, mut targets) = build(None);
        for actor in wings.mission.actors_mut() {
            actor.set_stations(Vec::new());
        }
        let mut gun = combat_fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        gun.source = "M61.JT".into();
        let actor = wings.mission.actor_mut(3).unwrap();
        actor.set_stations(simple_stations(0, 3, AI_STORE_SPEED));
        actor.set_guns([(1, gun.clone())].into());
        actor.flight_mut().position = [0., 20000., 1000.];
        actor.flight_mut().yaw = std::f64::consts::PI;
        wings
            .mission
            .order(3, WingRequest::TargetAssignment(TargetOrder::FreeSelection))
            .unwrap()
            .unwrap();
        wings
            .advance(player_object([0., 20000., 0.]), &mut targets, &flat)
            .unwrap();
        let actor = wings.mission.actor_mut(3).unwrap();
        let observed = actor.gun_target().expect("current permitted target");
        let aim = gunnery::solve(
            &gun,
            &crate::combat::launcher(actor.flight()),
            [0.; 3],
            observed,
        )
        .unwrap()
        .aim
        .direction;
        actor.flight_mut().yaw = aim[0].atan2(aim[2]);
        actor.flight_mut().pitch = aim[1].asin();
        let aligned = actor.flight().clone();
        assert_eq!(
            actor.rounds_remaining(),
            3,
            "controller intent does not debit"
        );
        let event = LaunchEvent {
            actor: 3,
            station: StationId(1),
            target: observed.id,
            request_id: RequestId(77),
            projectiles: 1,
        };
        let mut rounds = Vec::new();
        assert_eq!(wings.realise(&event, &mut rounds, &gun, 1, &[], Some(0)), 1);
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 2);
        assert_eq!(
            rounds[0].direction,
            Basis::new(aligned.yaw, aligned.pitch, aligned.bank).forward
        );
        assert!(rounds[0].target.is_none() && rounds[0].tracer);
        wings.mission.actor_mut(3).unwrap().flight_mut().yaw += 0.2;
        assert_eq!(wings.realise(&event, &mut rounds, &gun, 1, &[], Some(1)), 0);
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 2);
        *wings.mission.actor_mut(3).unwrap().flight_mut() = aligned.clone();
        let mut full = vec![rounds[0].clone(); MAX_PROJECTILES];
        assert_eq!(wings.realise(&event, &mut full, &gun, 1, &[], Some(1)), 0);
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 2);
        wings.mission.actor_mut(3).unwrap().stations_mut()[0]
            .store
            .inhibited = true;
        assert_eq!(wings.realise(&event, &mut rounds, &gun, 1, &[], Some(1)), 0);
        wings.mission.actor_mut(3).unwrap().stations_mut()[0]
            .store
            .inhibited = false;
        for ordinal in 1..=2 {
            assert_eq!(
                wings.realise(&event, &mut rounds, &gun, 1, &[], Some(ordinal)),
                1
            );
        }
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 0);
        assert_eq!(wings.realise(&event, &mut rounds, &gun, 1, &[], Some(3)), 0);
        wings.mission.actor_mut(3).unwrap().stations_mut()[0]
            .store
            .rounds = Rounds::Finite(1);
        wings.mission.actor_mut(3).unwrap().set_alive(false);
        assert_eq!(wings.realise(&event, &mut rounds, &gun, 1, &[], Some(3)), 0);
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 1);
        wings.mission.actor_mut(3).unwrap().set_alive(true);
        for request in [
            WingRequest::TargetAssignment(TargetOrder::HoldFire),
            WingRequest::FormationSelection(tore_sim::ai::wing::Formation::Echelon),
        ] {
            wings
                .mission
                .order(3, WingRequest::TargetAssignment(TargetOrder::FreeSelection))
                .unwrap()
                .unwrap();
            wings.mission.order(3, request).unwrap().unwrap();
            for _ in 0..120 {
                *wings.mission.actor_mut(3).unwrap().flight_mut() = aligned.clone();
                let output = wings
                    .advance(player_object([0., 20000., 0.]), &mut targets, &flat)
                    .unwrap();
                assert!(output.launches.iter().all(|e| e.actor != 3), "{request:?}");
            }
            assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 1);
        }
    }

    #[test]
    fn a_round_aimed_at_any_human_is_incoming_and_the_humans_register_each_step() {
        let (mut wings, targets) = build(None);
        for actor in wings.mission.actors_mut() {
            actor.set_stations(Vec::new());
        }
        let mut combat = combat_fixture(false);
        combat.targets = targets;
        let mut gun = combat.own().configuration().stations[0].weapon.clone();
        gun.source = "M61.JT".into();
        gun.burst.actual_rounds_per_game = 1;
        gun.burst.game_rounds_in_burst = 1;
        gun.burst.game_burst_t = 1;
        wings.weapons.insert((3, 0), gun);
        wings.pending_guns.insert(
            (3, 0),
            PendingGun {
                groups: VecDeque::from([(5, 1), (4, 1)]),
                next_scaled: wings.mission.tick(),
                ordinal: 0,
            },
        );
        let first = flight::State::new(&aircraft(), [0., 20000., -5000.]).unwrap();
        let second = flight::State::new(&aircraft(), [800., 20000., -5000.]).unwrap();
        let slot = |id, member| HumanSlot {
            id,
            side: launch::Side::Friendly,
            wing: 0,
            member,
        };
        for _ in 0..70 {
            // Handed over out of order: the bridge keeps id order.
            wings
                .step(
                    &mut combat,
                    &[human_at(slot(5, 1), &second), human_at(slot(0, 0), &first)],
                    &world(),
                )
                .unwrap();
        }
        assert_eq!(wings.humans, [slot(0, 0), slot(5, 1)]);
        let rounds: Vec<_> = combat.projectiles.iter().filter(|p| p.owner == 3).collect();
        assert_eq!(
            rounds.iter().map(|p| p.incoming).collect::<Vec<_>>(),
            [Some(5), None],
            "a round at the second human is incoming, one at an AI aircraft is not"
        );
    }

    #[test]
    fn canonical_gun_queue_preserves_fifo_phase_and_stops_for_dead_actor() {
        let (mut wings, targets) = build(None);
        for actor in wings.mission.actors_mut() {
            actor.set_stations(Vec::new());
        }
        let mut combat = combat_fixture(false);
        combat.targets = targets;
        let mut gun = combat.own().configuration().stations[0].weapon.clone();
        gun.source = "M61.JT".into();
        gun.burst.actual_rounds_per_game = 2;
        gun.burst.game_rounds_in_burst = 4;
        gun.burst.game_burst_t = 1;
        wings.weapons.insert((3, 0), gun);
        wings.pending_guns.insert(
            (3, 0),
            PendingGun {
                groups: VecDeque::from([(PLAYER_ID, 2), (4, 2)]),
                next_scaled: wings.mission.tick() * 8,
                ordinal: 0,
            },
        );
        let player = flight::State::new(&aircraft(), [0., 20000., -5000.]).unwrap();
        let mut release_ticks = Vec::new();
        for tick in 0..16 {
            let before = combat.projectiles.len();
            wings
                .step(&mut combat, &[human(&player)], &world())
                .unwrap();
            let count = combat.projectiles.len() - before;
            assert!(count <= 1);
            if count == 1 {
                release_ticks.push(tick);
            }
        }
        assert!(
            release_ticks
                .windows(2)
                .all(|pair| (3..=4).contains(&(pair[1] - pair[0])))
        );
        let rounds: Vec<_> = combat.projectiles.iter().filter(|p| p.owner == 3).collect();
        assert_eq!(rounds.len(), 4);
        assert_eq!(
            rounds
                .iter()
                .map(|p| p.incoming.is_some())
                .collect::<Vec<_>>(),
            [true, true, false, false]
        );
        assert_eq!(
            rounds.iter().map(|p| p.gun_round).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(0), Some(1)]
        );
        assert_eq!(
            rounds.iter().map(|p| p.tracer).collect::<Vec<_>>(),
            [true, false, false, true]
        );
        assert!(wings.pending_guns[&(3, 0)].groups.is_empty());
        assert_eq!(wings.pending_guns[&(3, 0)].ordinal, 4);

        wings.pending_guns.get_mut(&(3, 0)).unwrap().groups = VecDeque::from([(PLAYER_ID, 2)]);
        wings.mission.actor_mut(3).unwrap().set_alive(false);
        let dropped = wings.dropped_launches;
        wings
            .step(&mut combat, &[human(&player)], &world())
            .unwrap();
        assert!(wings.pending_guns[&(3, 0)].groups.is_empty());
        assert_eq!(wings.dropped_launches, dropped + 2);
        assert_eq!(wings.pending_guns[&(3, 0)].ordinal, 4);
    }

    #[test]
    fn accepted_recall_cancels_remaining_physical_gun_burst() {
        use tore_sim::ai::wing::{Formation, PlayerOrder};
        for order in [
            PlayerOrder::Formation(Formation::Echelon),
            PlayerOrder::Disengage,
        ] {
            let mut selections = payload(None);
            selections[0].wing.index = 0;
            let mut wings =
                AiWings::build_with(&selections, &spawned(), 0, |_| Ok((aircraft(), None)))
                    .unwrap();
            for actor in wings.mission.actors_mut() {
                actor.set_stations(Vec::new());
            }
            let mut combat = combat_fixture(false);
            combat.targets = spawned();
            let gun = combat.own().configuration().stations[0].weapon.clone();
            wings.weapons.insert((1, 0), gun);
            wings.pending_guns.insert(
                (1, 0),
                PendingGun {
                    groups: VecDeque::from([(3, 3)]),
                    next_scaled: 0,
                    ordinal: 0,
                },
            );
            let player = flight::State::new(&aircraft(), [0., 20000., -5000.]).unwrap();
            wings
                .step(&mut combat, &[human(&player)], &world())
                .unwrap();
            assert_eq!(combat.projectiles.len(), 1, "{order:?}");
            let rounds = wings.mission.actor(1).unwrap().rounds_remaining();
            let dropped = wings.dropped_launches;
            let report = wings.command(PLAYER_ID, order, None, Some(1)).unwrap();
            assert!(report.message.contains("1 applied"), "{order:?}");
            assert!(!wings.pending_guns.contains_key(&(1, 0)));
            for _ in 0..20 {
                wings
                    .step(&mut combat, &[human(&player)], &world())
                    .unwrap();
            }
            assert_eq!(combat.projectiles.len(), 1, "{order:?}");
            assert_eq!(wings.mission.actor(1).unwrap().rounds_remaining(), rounds);
            assert_eq!(wings.dropped_launches, dropped);
        }
    }

    #[test]
    fn normal_startup_selects_and_arms_canonical_gun() {
        let fixture = combat_fixture(false);
        let mut config = fixture.own().configuration().clone();
        config.stations[0].weapon.source = "M61.JT".into();
        let mut missile = config.stations[0].clone();
        missile.weapon.source = "AIM9M.JT".into();
        config.stations.insert(0, missile);
        let mut state = live::State::new(config, true).unwrap();
        state.own_mut().selected = 0;
        state.own_mut().armed = true;
        crate::combat::apply_startup_weapon_state(state.own_mut(), false, false);
        assert_eq!(state.own().selected, 1);
        assert!(state.own().armed);
    }

    #[test]
    fn guns_only_with_an_empty_gun_starts_on_nav_not_on_a_missile() {
        let fixture = combat_fixture(false);
        let mut config = fixture.own().configuration().clone();
        config.stations[0].weapon.source = "M61.JT".into();
        let mut missile = config.stations[0].clone();
        missile.weapon.source = "AIM9M.JT".into();
        config.stations.insert(0, missile);
        let mut state = live::State::new(config, true).unwrap();
        let own = state.own_mut();
        // The gun (station 1) carries nothing; the missile (station 0) does.
        own.ammo[1] = 0;
        assert!(own.carries(0, false) && !own.carries(1, false));
        // Guns only.
        crate::combat::apply_startup_weapon_state(own, true, false);
        assert!(!own.armed);
        // Without the cheat the same load starts on the loaded missile.
        crate::combat::apply_startup_weapon_state(own, false, false);
        assert!(own.armed && own.selected == 0);
    }

    #[test]
    fn active_ai_launch_uses_owned_guidance_and_cannot_be_decoyed_before_pitbull() {
        use tore_sim::ai::{mission::DeviceEvent, weapon_service::StationId};
        let (mut wings, _) = build(None);
        let mut combat = combat_fixture(true);
        let mut weapon = combat.own().configuration().stations[0].weapon.clone();
        weapon.source = "AIM120.JT".into();
        weapon.flags = 0x41;
        weapon.seeker.signature = 3;
        weapon.seeker.chaff_flare_chance = 100;
        wings.device_effectiveness.insert(3, (100, 100));
        let event = LaunchEvent {
            actor: 1,
            station: StationId(0),
            target: 3,
            request_id: RequestId(1),
            projectiles: 1,
        };
        assert_eq!(
            wings.realise(&event, &mut combat.projectiles, &weapon, 0, &[], None),
            1
        );
        assert!(combat.projectiles[0].guidance.is_some());
        assert!(combat.projectiles[0].motion.is_some());
        let release = DeviceEvent {
            actor: 3,
            class: SeekerClass::Radar,
            released: 1,
        };
        wings.realise_device(&release, &mut combat).unwrap();
        assert_eq!(combat.projectiles[0].target, Some(3));
        let flight = combat.projectiles[0].guidance.as_mut().unwrap();
        flight.enabled = true;
        flight.seeker.acquired = true;
        flight.seeker.observation = Some(seeker::Observation {
            id: 3,
            position: [0.; 3],
            velocity: [0.; 3],
            quality: 1.,
            off_axis: 0.,
            range: 1000.,
        });
        wings.realise_device(&release, &mut combat).unwrap();
        assert_eq!(combat.projectiles[0].target, None);
        assert!(combat.projectiles[0].guidance.is_none());
        // The physical body remains visible after a successful decoy.
        assert_eq!(
            combat
                .missile_snapshots(&[(
                    0,
                    crate::combat::launcher(
                        &flight::State::new(&aircraft(), [0., 20000., 0.]).unwrap()
                    ),
                )])
                .len(),
            1
        );
        wings.weapon_rules = Rules::Compatibility;
        let mut compatibility = Vec::new();
        wings.realise(&event, &mut compatibility, &weapon, 0, &[], None);
        assert!(compatibility[0].guidance.is_none());
    }

    #[test]
    fn a_decoy_reads_the_weapon_of_the_round_owners_own_ownship() {
        use tore_sim::ai::mission::DeviceEvent;
        let (mut wings, _) = build(None);
        let mut combat = combat_fixture(true);
        wings.device_effectiveness.insert(3, (100, 100));
        // A second human-flown plane, 50, whose station 0 carries an
        // infrared missile that any flare fools; plane 0's does not.
        let mut config = combat.own().configuration().clone();
        assert!(
            config.stations[0].weapon.seeker.signature != 2
                || config.stations[0].weapon.seeker.chaff_flare_chance != 100
        );
        config.stations[0].weapon.seeker.signature = 2;
        config.stations[0].weapon.seeker.chaff_flare_chance = 100;
        let side = combat.own().side;
        combat
            .add_ownship(live::Ownship::new(50, side, config, true).unwrap())
            .unwrap();
        let mut rounds = Vec::new();
        let weapon = combat.own().configuration().stations[0].weapon.clone();
        wings.realise(
            &LaunchEvent {
                actor: 1,
                station: tore_sim::ai::weapon_service::StationId(0),
                target: 3,
                request_id: tore_sim::ai::weapon_service::RequestId(1),
                projectiles: 1,
            },
            &mut rounds,
            &weapon,
            0,
            &[],
            None,
        );
        let mut round = rounds.remove(0);
        round.weapon = None;
        round.station = 0;
        round.owner = 50;
        round.target = Some(3);
        round.guidance = None;
        combat.projectiles = vec![round];
        wings
            .realise_device(
                &DeviceEvent {
                    actor: 3,
                    class: SeekerClass::Infrared,
                    released: 1,
                },
                &mut combat,
            )
            .unwrap();
        assert_eq!(
            combat.projectiles[0].target, None,
            "decoyed by the flare, as plane 50's own missile"
        );
        // The roll names the flare it was against: the device just released.
        let rolls = wings.decoy_rolls();
        assert_eq!(rolls.len(), 1);
        assert!(rolls[0].device > 0 && rolls[0].device == combat.devices.released());
    }

    #[test]
    fn live_devices_decoy_only_matching_missiles_targeting_the_releaser() {
        use tore_sim::ai::{
            mission::DeviceEvent,
            weapon_service::{RequestId, StationId},
        };
        let (mut wings, _) = build(None);
        let mut combat = combat_fixture(true);
        let mut weapon = combat.own().configuration().stations[0].weapon.clone();
        weapon.seeker.signature = 2;
        weapon.seeker.chaff_flare_chance = 100;
        wings.device_effectiveness.insert(3, (100, 100));
        for target in [3, 4] {
            wings.realise(
                &LaunchEvent {
                    actor: 1,
                    station: StationId(0),
                    target,
                    request_id: RequestId(u64::from(target)),
                    projectiles: 1,
                },
                &mut combat.projectiles,
                &weapon,
                0,
                &[],
                None,
            );
        }
        let mut radar = combat.projectiles[0].clone();
        radar.weapon.as_mut().unwrap().seeker.signature = 3;
        combat.projectiles.push(radar);
        combat.take_sound_events();
        wings
            .realise_device(
                &DeviceEvent {
                    actor: 3,
                    class: SeekerClass::Infrared,
                    released: 1,
                },
                &mut combat,
            )
            .unwrap();
        assert_eq!(combat.projectiles[0].target, None);
        assert_eq!(combat.projectiles[1].target, Some(4));
        assert_eq!(combat.projectiles[2].target, Some(3));
        assert_eq!(combat.devices.flares.len(), 2);
        // The flare is heard from the releasing aircraft, not the player.
        let sounds = combat.take_sound_events();
        assert_eq!(sounds.len(), 1);
        assert_eq!(
            (sounds[0].kind, sounds[0].position, sounds[0].own),
            (
                tore_sim::acoustics::Kind::Flare,
                wings.mission().actor(3).unwrap().flight().position,
                false
            )
        );
    }
    #[test]
    fn imported_decoy_chances_above_100_count_as_certain() {
        // Battery finding (2026-09-28): an imported missile whose chaff and
        // flare chance is above 100 stopped the whole mission with "decoy
        // percentages exceed 100" when an AI aircraft released a flare at it.
        use tore_sim::ai::{
            mission::DeviceEvent,
            weapon_service::{RequestId, StationId},
        };
        let (mut wings, _) = build(None);
        let mut combat = combat_fixture(true);
        let mut weapon = combat.own().configuration().stations[0].weapon.clone();
        weapon.seeker.signature = 2;
        weapon.seeker.chaff_flare_chance = 150;
        wings.device_effectiveness.insert(3, (200, 100));
        wings.realise(
            &LaunchEvent {
                actor: 1,
                station: StationId(0),
                target: 3,
                request_id: RequestId(3),
                projectiles: 1,
            },
            &mut combat.projectiles,
            &weapon,
            0,
            &[(0, [0.0; 3])],
            None,
        );
        wings
            .realise_device(
                &DeviceEvent {
                    actor: 3,
                    class: SeekerClass::Infrared,
                    released: 1,
                },
                &mut combat,
            )
            .unwrap();
        assert_eq!(combat.projectiles[0].target, None);
    }

    #[test]
    fn finite_missile_depletion_is_followed_by_actor_owned_gun_fire() {
        use tore_sim::ai::wing::{TargetOrder, WingRequest};
        let (mut wings, mut targets) = build(None);
        wings
            .mission
            .order(3, WingRequest::TargetAssignment(TargetOrder::FreeSelection))
            .unwrap()
            .unwrap();
        for actor in wings.mission.actors_mut() {
            actor.set_stations(Vec::new());
        }
        wings
            .mission
            .actor_mut(3)
            .unwrap()
            .set_stations(simple_stations(1, 20, AI_STORE_SPEED));
        let mut fixed: Vec<_> = wings
            .mission
            .actors()
            .iter()
            .map(|a| (a.id(), a.flight().clone()))
            .collect();
        for (id, flight) in &mut fixed {
            flight.position = [
                if *id == 2 || *id == 4 { 50000.0 } else { 0.0 },
                20000.0,
                if *id >= 3 { 2000.0 } else { 0.0 },
            ];
        }
        let missile = combat_fixture(true).own().configuration().stations[0]
            .weapon
            .clone();
        let gun = combat_fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        let mut realised = Vec::new();
        let mut stations = Vec::new();
        for _ in 0..10000 {
            for (id, flight) in &fixed {
                *wings.mission.actor_mut(*id).unwrap().flight_mut() = flight.clone();
            }
            let output = wings
                .advance(player_object([0.0, 20000.0, 0.0]), &mut targets, &flat)
                .unwrap();
            for event in output.launches.iter().filter(|e| e.actor == 3) {
                stations.push(event.station.0);
                let weapon = if event.station.0 == 0 { &missile } else { &gun };
                wings.realise(
                    event,
                    &mut realised,
                    weapon,
                    usize::from(event.station.0),
                    &[(PLAYER_ID, [0.0, 20000.0, 0.0])],
                    None,
                );
            }
            if wings.mission.actor(3).unwrap().rounds_remaining() == 0 {
                break;
            }
        }
        assert_eq!(stations, [0, 1, 1]);
        assert_eq!(realised.len(), 21);
        assert!(
            realised[1..]
                .iter()
                .all(|p| p.weapon.as_ref() == Some(&gun) && p.target.is_none())
        );
        assert_eq!(wings.mission.actor(3).unwrap().rounds_remaining(), 0);
    }
    #[test]
    fn targetless_protect_me_releases_only_the_addressed_wingman() {
        use tore_sim::ai::{
            engagement::{Role, Stance},
            wing::PlayerOrder,
        };
        let mut selections = payload(None);
        selections[0].wing.index = 0;
        let mut wings =
            AiWings::build_with(&selections, &spawned(), 0, |_| Ok((aircraft(), None))).unwrap();
        let report = wings
            .command(PLAYER_ID, PlayerOrder::ProtectMe, None, Some(1))
            .unwrap();
        assert!(report.message.contains("1 applied"));
        assert!(!wings.mission.actor(1).unwrap().is_neutral());
        assert!(wings.mission.actor(2).unwrap().is_neutral());
        assert!(wings.mission.actor(3).unwrap().is_neutral());
        let assignment = wings.mission.actor(1).unwrap().assignment();
        assert_eq!(assignment.role, Role::Escort);
        assert_eq!(assignment.stance, Stance::ProtectAssigned);
        assert_eq!(assignment.protected_ids, [PLAYER_ID]);
    }

    #[test]
    fn attack_on_contact_releases_after_recall_but_rejected_engage_does_not() {
        use tore_sim::ai::engagement::Assignment;
        use tore_sim::ai::wing::{Formation, PlayerOrder};
        let mut selections = payload(None);
        selections[0].wing.index = 0;
        let mut wings =
            AiWings::build_with(&selections, &spawned(), 0, |_| Ok((aircraft(), None))).unwrap();
        let rejected = wings
            .command(PLAYER_ID, PlayerOrder::EngageMyTarget, Some(999), Some(1))
            .unwrap();
        assert!(rejected.message.contains("no valid hostile target"));
        assert!(wings.mission.actor(1).unwrap().is_neutral());
        wings
            .command(PLAYER_ID, PlayerOrder::AttackOnContact, None, Some(1))
            .unwrap();
        assert!(!wings.mission.actor(1).unwrap().is_neutral());
        assert_eq!(
            wings.mission.actor(1).unwrap().assignment(),
            &Assignment::default()
        );
        wings
            .command(
                PLAYER_ID,
                PlayerOrder::Formation(Formation::Echelon),
                None,
                Some(1),
            )
            .unwrap();
        assert!(wings.mission.actor(1).unwrap().is_neutral());
        wings
            .command(PLAYER_ID, PlayerOrder::AttackOnContact, None, Some(1))
            .unwrap();
        assert!(!wings.mission.actor(1).unwrap().is_neutral());
        assert!(wings.mission.actor(2).unwrap().is_neutral());
        assert!(wings.mission.actor(3).unwrap().is_neutral());
    }

    #[test]
    fn alt_t_cycles_echelon_line_abreast_line_astern() {
        use tore_sim::ai::wing::{Formation, PlayerOrder};
        let mut selections = payload(None);
        selections[0].wing.index = 0;
        let mut wings =
            AiWings::build_with(&selections, &spawned(), 0, |_| Ok((aircraft(), None))).unwrap();
        // Wings start in the mission's echelon.
        assert_eq!(
            wings.next_formation(PLAYER_ID, None),
            Formation::LineAbreast
        );
        for (ordered, next) in [
            (Formation::LineAbreast, Formation::LineAstern),
            (Formation::LineAstern, Formation::Echelon),
            (Formation::Echelon, Formation::LineAbreast),
        ] {
            wings
                .command(PLAYER_ID, PlayerOrder::Formation(ordered), None, None)
                .unwrap();
            assert_eq!(wings.next_formation(PLAYER_ID, None), next);
        }
    }

    #[test]
    fn player_commands_report_acceptance_cancel_and_stay_in_the_addressed_wing() {
        use tore_sim::ai::wing::{PlayerApproach, PlayerBreak, PlayerOrder as O};
        let mut selections = payload(None);
        selections[0].wing.index = 0;
        let mut bridge =
            AiWings::build_with(&selections, &spawned(), 0, |_| Ok((aircraft(), None))).unwrap();
        let before = bridge.mission.actor(1).unwrap().flight().clone();
        let report = bridge
            .command(PLAYER_ID, O::EngageMyTarget, Some(3), Some(1))
            .unwrap();
        assert!(report.message.contains("1 applied"));
        // The player's call is immediate; the reply is a delayed radio event.
        assert_eq!(
            report.radio,
            bridge.assignment_stems(1, Some(3), Some(1), 0)
        );
        assert_eq!(report.radio[0], "^NUM02", "wingman 1 is Two");
        let engage = Chatter::Engage {
            speaker: 1,
            aircraft: true,
        };
        assert_eq!(std::mem::take(&mut bridge.chatter), [engage]);
        assert_eq!(
            bridge.mission.actor(1).unwrap().controller().target(),
            Some(3)
        );
        assert_eq!(bridge.mission.actor(2).unwrap().controller().target(), None);
        assert_eq!(
            *bridge.mission.actor(1).unwrap().flight(),
            before,
            "delivery cannot move an aircraft"
        );
        assert!(
            bridge
                .command(PLAYER_ID, O::EngageMyTarget, Some(2), None)
                .unwrap()
                .radio
                .is_empty()
        );
        let report = bridge
            .command(PLAYER_ID, O::EngageMyTarget, Some(3), Some(2))
            .unwrap();
        assert_eq!(
            report.radio,
            bridge.assignment_stems(2, Some(3), Some(2), 0)
        );
        assert_eq!(report.radio[0], "^NUM03", "wingman 2 is Three");
        assert!(
            bridge.chatter.is_empty(),
            "only the first living wingman replies"
        );
        let report = bridge
            .command(
                PLAYER_ID,
                O::Approach(PlayerApproach::Left),
                Some(3),
                Some(1),
            )
            .unwrap();
        assert_eq!(report.radio, ["^APPRCLF"]);
        let report = bridge
            .command(PLAYER_ID, O::Break(PlayerBreak::Right), None, Some(1))
            .unwrap();
        assert_eq!(report.radio, ["^BREAKRT"]);
        assert!(report.message.contains("1 applied"));
        bridge.command(PLAYER_ID, O::Spacing, None, None).unwrap();
        for id in [1, 2] {
            assert_eq!(
                bridge
                    .mission
                    .actor(id)
                    .unwrap()
                    .controller()
                    .wing_settings()
                    .1,
                Some(2048)
            );
        }
        bridge
            .command(PLAYER_ID, O::Stacking, None, Some(2))
            .unwrap();
        assert_eq!(
            bridge
                .mission
                .actor(2)
                .unwrap()
                .controller()
                .wing_settings()
                .2,
            Some(512)
        );
        assert_eq!(
            bridge
                .mission
                .actor(1)
                .unwrap()
                .controller()
                .wing_settings()
                .2,
            None
        );
        let report = bridge.command(PLAYER_ID, O::Disengage, None, None).unwrap();
        assert_eq!(report.radio, ["^DISENG"]);
        assert_eq!(bridge.mission.actor(1).unwrap().controller().target(), None);
        assert_eq!(bridge.mission.actor(2).unwrap().controller().target(), None);
        assert_eq!(
            bridge
                .mission
                .actor(3)
                .unwrap()
                .controller()
                .wing_settings(),
            (None, None, None)
        );
        use tore_sim::ai::wing::{TargetId, TargetOrder, WingRequest};
        bridge
            .mission
            .order(
                3,
                WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(PLAYER_ID))),
            )
            .unwrap()
            .unwrap();
        bridge.chatter.clear();
        let protected = bridge.command(PLAYER_ID, O::ProtectMe, None, None).unwrap();
        assert_eq!(protected.radio, ["^CLRMY6"]);
        assert_eq!(
            std::mem::take(&mut bridge.chatter),
            [Chatter::Showtime { speaker: 1 }]
        );
        assert_eq!(
            bridge.mission.actor(1).unwrap().assignment().protected_ids,
            [PLAYER_ID]
        );
        bridge.mission.actor_mut(1).unwrap().set_alive(false);
        assert_eq!(
            bridge
                .command(PLAYER_ID, O::EngageMyTarget, Some(3), None)
                .unwrap()
                .radio,
            // From the first wingman left alive.
            bridge.assignment_stems(2, Some(3), None, 0)
        );
        assert_eq!(
            bridge.chatter,
            [Chatter::Engage {
                speaker: 2,
                aircraft: true,
            }]
        );
    }
    #[test]
    fn unobserved_target_is_rejected_without_reply_or_control_changes() {
        use tore_sim::ai::wing::PlayerOrder;
        let mut selections = payload(None);
        selections[0].wing.index = 0;
        let sensors = sensors::SensorProfiles {
            aircraft: AircraftId::F18,
            radar: None,
            infrared: None,
            visual: None,
            jammer: None,
            signature: sensors::SignatureProfile::default(),
        };
        let mut bridge = AiWings::build_with(&selections, &spawned(), 0, |_| {
            Ok((aircraft(), Some(sensors.clone())))
        })
        .unwrap();
        let report = bridge
            .command(PLAYER_ID, PlayerOrder::EngageMyTarget, Some(3), None)
            .unwrap();
        assert!(report.message.contains("0 applied, 2 rejected"));
        // The call is still voiced: the rejection comes back as a reply.
        assert_eq!(report.radio, bridge.assignment_stems(1, Some(3), None, 0));
        assert!(report.reached.is_empty(), "a rejected order reaches no one");
        assert!(bridge.chatter.is_empty(), "a rejected order has no reply");
        for id in [1, 2] {
            assert!(
                bridge
                    .mission
                    .actor(id)
                    .unwrap()
                    .controller()
                    .target()
                    .is_none()
            );
            assert_eq!(
                bridge
                    .mission
                    .actor(id)
                    .unwrap()
                    .controller()
                    .wing_settings(),
                (None, None, None)
            );
        }
    }

    #[test]
    fn the_bridge_keeps_its_last_output_and_hands_over_the_ai_records() {
        let (mut wings, targets) = build(None);
        let mut combat = combat_fixture(false);
        combat.targets = targets;
        let player = flight::State::new(&aircraft(), [0., 20000., -5000.]).unwrap();
        assert!(wings.last_output().activities.is_empty());
        for _ in 0..3 {
            wings
                .step(&mut combat, &[human(&player)], &world())
                .unwrap();
        }
        let last_tick = wings.mission.tick() - 1;
        assert_eq!(wings.last_output().activities.len(), wings.mission.len());
        for slot in wings.slots() {
            assert_eq!(
                wings.actor_trace(slot.id).unwrap().tick,
                Some(last_tick),
                "{}",
                slot.label()
            );
            assert_eq!(
                wings.controller_trace(slot.id).unwrap().tick,
                Some(last_tick)
            );
        }
        assert!(wings.actor_trace(PLAYER_ID).is_none());
        assert_eq!(wings.take_ai_journal().dropped, 0);
        assert!(
            wings.take_ai_journal().entries.is_empty(),
            "a drained journal starts empty"
        );
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "ai_wings_checkpoint.rs"]
mod checkpoint;
