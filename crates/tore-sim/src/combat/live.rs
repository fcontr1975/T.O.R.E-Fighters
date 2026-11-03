//! Explicit development live-fire adapter. Source configuration and recovered scalar
//! kernels are combined with authored scheduling, guidance and swept-sphere contacts.
//! This is NOT the diagnostic native-parity update or a retail AI implementation.
use super::ledger::{Kill, Resolution, ShotKind};
use super::missiles::{
    self, Flight, Guidance, LaunchMode, Motion, Rules, TargetRole,
    seeker::{self, Heat, Seeker, Status},
};
use super::{
    EnginePhase, FallState, PlayerTrigger, axial_speed, commanded_speed, engine_phase,
    launch_speed, removal_due, unload,
};
use crate::attitude::{Basis, Vector, cross, dot, unit};
use crate::models::strike_class::StrikeClass;
use crate::sensors::{self, Observable, Observer, Sensors, Support, passive};
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::{
    Result,
    aircraft::{Aircraft, AircraftId},
    weapons::Weapon,
};

fn draw(state: &mut u32, bound: u16) -> u16 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state % u32::from(bound)) as u16
}

mod agm;
#[cfg(test)]
mod agm_tests;
pub use agm::{CueEmitter, MAX_HARM_CUE};
#[cfg(test)]
mod bomb_tests;
mod broad;
#[cfg(test)]
mod cluster_tests;
mod cockpit;
#[cfg(test)]
mod cockpit_tests;
pub use cockpit::{
    AiCockpit, GUIDED_LATE_SHARE, ReleaseAim, StrikeCockpit, StrikeRelease, StrikeTarget,
    guided_due, is_strike_store,
};
mod collateral;
#[cfg(test)]
mod delivery_tests;
mod ground_sensing;
#[cfg(test)]
mod ground_sensing_tests;
pub use ground_sensing::GroundListing;
mod energy;
mod guided;
#[cfg(test)]
pub(crate) mod guided_tests;
pub use guided::{GuidedFlight, Mode as GuidedMode, SECONDS_REFRESH_TICKS};
#[cfg(test)]
mod gun_key_tests;
mod gunship_fire;
#[cfg(test)]
mod gunship_impact_tests;
#[cfg(test)]
mod gunship_tests;
pub use gunship_fire::{AiGunRound, GunTrain, NoShot, ai_gun_due, ai_gun_slow, ai_gun_train};
mod handoff;
mod jettison;
#[cfg(test)]
mod kept_tests;
mod observation;
#[cfg(test)]
mod observation_reference;
mod sighting;
#[cfg(test)]
mod sighting_tests;
mod store_signature;
#[cfg(test)]
mod strafe_tests;
#[cfg(test)]
mod worker_tests;
pub use handoff::{AiHandback, AiPose, AiStores, Kept};
mod parked;
#[cfg(test)]
mod parked_tests;
pub mod rewind;
pub use parked::{ParkedAircraft, ParkedSite};
mod surface;
pub use surface::{
    AIR_TARGET_FLAG, GroundLook, Refused, SURFACE_PROJECTILE_ID_BASE, SURFACE_PROJECTILE_RESERVE,
    SURFACE_TARGET_FLAG, SurfaceRound, SurfaceShot, engages_aircraft, engages_surface,
    flak_explosion, is_flak, opposed, surface_tracer, ticks_to_range,
};

/// Rounds one loaded store holds: a rocket pod's rockets, else 1 (retail's
/// `HARDPodHack` multiplies a loaded station's count by it, for the player,
/// the AI and every wing member alike).
pub fn pod_rounds(w: &Weapon) -> u32 {
    if super::ballistics::is_pod(w) {
        u32::from(w.burst.projectiles_in_pod.unsigned_abs())
    } else {
        1
    }
}

/// Rounds, missiles and bombs in flight at once. John, 2026-10-09: 5,000,
/// up from 256, so a crowded gunfight never loses a burst. What a full sky
/// costs is measured in `docs/baselines/projectile-cap-2026-10-09.md`.
pub const MAX_PROJECTILES: usize = 5000;
pub use crate::ai::targeting::Side;
/// The side of nothing: ground objects, fixtures and rounds nobody owns. It is
/// never friendly to anything, so friendly fire never spares it.
pub const NO_SIDE: Side = Side(0);
/// The side an ownship takes when the host names none, as single player's
/// friendly side.
pub const DEFAULT_OWNSHIP_SIDE: Side = Side(1);
/// Whether rounds hurt aircraft of their shooter's own side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FriendlyFire {
    /// Every round can hit any aircraft its shooter's rules allow, whatever
    /// side: single player's behaviour.
    #[default]
    On,
    /// No round damages an aircraft of its shooter's side, the shooter
    /// included. Collisions still destroy whoever is in them.
    Off,
}
/// The owner of the diagnostic incoming round ([`Command::Incoming`]): no
/// aircraft, so it can hit any ownship. It carries the selected station's
/// weapon record.
pub const INCOMING_OWNER: u32 = u32::MAX;
pub const MAX_EFFECTS: usize = 64;
pub const MAX_HIT_RECORDS: usize = 128;
/// The puff a dud bomb raises: the original's bullet-on-land explosion, its
/// smallest dust burst (explosion type 15; docs/spec/explosions.md).
const DUD_PUFF: u8 = 15;
/// How long after its motor lights a drop-launched store keeps sparing its
/// launcher, in the record's quarter-second steps: 1 s (fitted, agent
/// decision 2026-10-20; a lofting AIM-54 is clear of the launcher's
/// 128 ft box about 0.6 s after it lights).
const DROP_SEPARATION_T: u64 = 4;
/// Fitted: a trigger press waits this long, 3 seconds, for the bay to open.
const BAY_RELEASE_TICKS: u64 = 360;
/// Fitted: the bay stays open 1 second after a release so the weapon clears.
const BAY_HOLD_TICKS: u64 = 120;

/// Exact category switch at FA 0x411470; category is not a bitmask here.
pub fn damage_class(category: u16) -> usize {
    match category {
        0x40 | 0x200 | 0x800 | 0x1000 => 4,
        0x100 => 2,
        0x400 => 3,
        0x2000 => 1,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Safe,
    BayClosed,
    LauncherLost,
    StationFailed,
    Empty,
    Capacity,
    NoTarget,
    TargetDestroyed,
    WrongTarget,
    NoRadar,
    RadarOff,
    RadarFailed,
    RadarCoverage,
    RadarSearchOnly,
    RadarAcquiring,
    MinimumRange,
    MaximumRange,
    Altitude,
    FieldOfView,
    GunArc,
    GunSlewing,
    GroupEmpty,
    /// The gun's own airframe (wing, nacelles, skin) is in the line of fire.
    GunObscured,
    /// Terrain blocks the gun aim or a surface missile's launch sight line.
    TerrainMask,
    /// An anti-radiation missile in BORE has no emitter in its cone to fire
    /// at: retail releases a HARM only at an emitting target.
    NoEmitter,
    /// A surface radar weapon (AGM-84A, AM-39, AS-16, AS-7, AS-15) has no
    /// A-G track on its target: the A-G page is off or does not list it.
    TrackLost,
}
impl Readiness {
    /// Whether an AC-130 gun with this readiness releases rounds. Only the
    /// states that make a shot impossible block it: safe, launcher lost,
    /// failed, empty, the projectile limit, an empty group, and the gun's
    /// own airframe in the way. Every other state is advisory: the rounds
    /// leave along the actual barrel whether or not it is solved.
    pub fn gun_may_fire(self) -> bool {
        !matches!(
            self,
            Self::Safe
                | Self::LauncherLost
                | Self::StationFailed
                | Self::Empty
                | Self::Capacity
                | Self::GroupEmpty
                | Self::GunObscured
        )
    }
    /// Player launch advisories do not inhibit a missile release. Hardware,
    /// ammunition, terrain and surface-role restrictions still apply.
    pub fn player_missile_may_fire(self, w: &Weapon) -> bool {
        if self == Self::Ready {
            return true;
        }
        if is_gun(w) || w.seeker.signature == 0 {
            return false;
        }
        if matches!(self, Self::MaximumRange | Self::RadarAcquiring) {
            return true;
        }
        let free_search = missiles::Profile::for_weapon(w).is_some_and(|p| {
            p.role == TargetRole::Aircraft
                || matches!(p.guidance, Guidance::Active | Guidance::Supported)
        });
        free_search
            && matches!(
                self,
                Self::NoTarget
                    | Self::NoRadar
                    | Self::RadarOff
                    | Self::RadarFailed
                    | Self::RadarCoverage
                    | Self::RadarSearchOnly
                    | Self::FieldOfView
                    | Self::TrackLost
            )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Safe => "SAFE",
            Self::BayClosed => "OPENING BAY",
            Self::LauncherLost => "LAUNCHER LOST",
            Self::StationFailed => "STATION FAILED",
            Self::Empty => "EMPTY",
            Self::Capacity => "PROJECTILE LIMIT",
            Self::NoTarget => "NO TARGET",
            Self::WrongTarget => "TARGET TYPE",
            Self::TargetDestroyed => "TARGET DESTROYED",
            Self::NoRadar => "NO RADAR",
            Self::RadarOff => "RADAR OFF",
            Self::RadarFailed => "RADAR FAILED",
            Self::RadarCoverage => "RADAR COVERAGE",
            Self::RadarSearchOnly => "RWS SEARCH ONLY",
            Self::RadarAcquiring => "ACQUIRING",
            Self::MinimumRange => "MIN RANGE",
            Self::MaximumRange => "MAX RANGE",
            Self::Altitude => "ALTITUDE LIMIT",
            Self::FieldOfView => "SEEKER FOV",
            Self::GunArc => "CANNOT BEAR",
            Self::GunSlewing => "SLEWING",
            Self::GroupEmpty => "GROUP EMPTY",
            Self::GunObscured => "NO LINE OF FIRE",
            Self::TerrainMask => "TERRAIN MASK",
            Self::NoEmitter => "NO EMITTER",
            Self::TrackLost => "NO TRACK",
        }
    }
}

/// Authored manual-range commands. Apply at tick boundaries for deterministic replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    NextWeapon,
    NextGunGroup,
    ToggleGunGroup,
    NextSelection,
    PreviousSelection,
    SelectNav,
    /// The selected station ran dry: move to the next loaded one, or NAV.
    AdvanceFromEmpty,
    ToggleSeekerMode,
    CompatibilityWeapons,
    TargetHeat(u8),
    TargetDistance(u32),
    ClearRange,
    ToggleTargetRadar,
    /// T: the next radar contact, nearest first.
    Designate,
    /// Shift-T: the previous radar contact.
    DesignatePrevious,
    /// Enter: the visible sensor contact nearest the nose.
    DesignateVisual,
    /// Persistent selection of one current contact by its stable identity.
    DesignateTarget(u32),
    /// A human screen pick, validated against this ownship's visual contacts.
    DesignateVisualTarget(u32),
    ClearDesignation,
    /// Ctrl+R or Shift+R: the radar's air-to-ground page on or off. It turns
    /// the air radar off and never touches the designated target
    /// (docs/spec/air-to-ground.md, "The air-to-ground radar").
    ToggleAirGround,
    /// Ctrl+X: the nearest hostile ground object, only with the Easy
    /// targeting cheat on (a retail cheat key).
    NearestGround,
    /// Ctrl+L: the laser on or off by hand, on an aircraft with a laser
    /// designator (docs/spec/air-to-ground.md, "The laser").
    ToggleLaser,
    /// `/`: the targeting sight tracks the next visible ground object.
    SightNext,
    /// Tab goes down: the gun fires, whatever station is selected, until
    /// [`Command::GunKeyUp`] (docs/spec/air-to-ground.md, "The gun key").
    GunKeyDown,
    /// Tab comes up.
    GunKeyUp,
    /// Backslash on an aircraft with a targeting sight (the AC-130's
    /// gunsight included): track the object under the crosshair, else pin
    /// the ground there.
    SightDesignate,
    /// Shift+Backslash on an aircraft with a targeting sight: pin the ground
    /// under the crosshair.
    SightPinGround,
    ToggleArm,
    Jettison,
    JettisonTanks,
    ReplaceTarget,
    CycleClass,
    FailStation,
    DamagePlayer,
    Incoming,
    ToggleTargetJammer,
    /// Release one chaff cartridge against radar missiles guiding on the player.
    ReleaseChaff,
    /// Release one flare against infrared missiles guiding on the player.
    ReleaseFlare,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRecord {
    pub tick: u64,
    pub target: u32,
    pub station: usize,
    pub class: usize,
    pub nominal: i32,
    pub applied: i32,
    pub hp_after: i32,
}
/// One projectile that damaged the player or a target, kept for the radio
/// hit, kill and "I'm hit" calls (docs/spec/radio-chatter.md). The host drains
/// the list each tick with [`State::take_strikes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strike {
    /// Who fired it: an ownship's aircraft or an AI actor.
    pub owner: u32,
    /// The damaged aircraft or ground object.
    pub victim: u32,
    /// The weapon's type flags: 0x1 guided, 0x10 bomb, 0x80 bullet.
    pub weapon_flags: u32,
    /// The damage left the victim with no hit points.
    pub destroyed: bool,
    /// The hit points the hit took from the victim, at most what it had
    /// left (a networked game's damage tally reads it: docs/ARCHITECTURE.md,
    /// "Scoring").
    pub amount: i32,
}
/// Strikes kept between drains; older ones are dropped first.
pub const MAX_STRIKES: usize = 64;
/// What happened to the released chaff and flares, kept for a mission
/// recording, which rebuilds the devices from it. Write-only: nothing in
/// combat reads it back; the host drains the list with
/// [`State::take_device_notes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceNote {
    Released(DeviceRelease),
    /// A range reset removed every device and restarted their numbering,
    /// after the step of this combat tick.
    Cleared(u64),
}
/// One chaff cartridge or flare leaving an aircraft.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceRelease {
    /// The releasing aircraft: an ownship's or an AI actor's.
    pub owner: u32,
    /// [`EffectKind::Chaff`] or [`EffectKind::Flare`].
    pub kind: EffectKind,
    pub release: super::countermeasures::Release,
    /// The device's number among this flight's releases, from 1. It chose
    /// the device's look.
    pub number: u64,
    /// The combat tick after whose step the device left; the next step is
    /// its first.
    pub tick: u64,
    /// The player's devices of this kind left afterwards; `None` for an AI
    /// aircraft, whose dispensers combat does not hold.
    pub left: Option<u8>,
}
/// One missile's roll against one of the player's chaff cartridges or
/// flares, kept for a mission recording. Write-only; the host drains the
/// list with [`State::take_decoy_rolls`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecoyRoll {
    /// The ownship whose chaff or flare the missile rolled against.
    pub aircraft: u32,
    pub projectile: u32,
    /// [`EffectKind::Chaff`] or [`EffectKind::Flare`].
    pub kind: EffectKind,
    /// The number of the chaff cartridge or flare the missile rolled
    /// against: [`DeviceRelease::number`] of its release.
    pub device: u64,
    /// The missile's decoy susceptibility, percent.
    pub susceptibility: u8,
    /// The dispenser's effectiveness, percent.
    pub effectiveness: u8,
    /// The chance numerator in `bound` units, not necessarily whole percent.
    pub threshold: u16,
    /// Draw resolution: 100 for legacy/IR, 10000 for Energy chaff.
    pub bound: u16,
    /// The draw in 0..bound; below threshold the store follows the decoy.
    pub roll: u16,
    pub decoyed: bool,
}
/// Device notes and decoy rolls kept between drains; older ones are dropped
/// first, so a host that never drains them still uses bounded memory.
pub const MAX_RELEASE_RECORDS: usize = 256;
#[derive(Clone, Debug)]
pub struct Station {
    pub weapon: Weapon,
    /// The source record's position, feet right, up and forward: a paired
    /// record's right-hand pylon ([`super::hardpoints::Rack`]).
    pub mount: Vector,
    pub count: u16,
    pub internal: bool,
    /// Its source location and pairing (B3).
    pub rack: super::hardpoints::Rack,
}
impl Station {
    /// Where the store this station releases next leaves, feet right, up
    /// and forward of the aircraft, when `units_after` whole releases remain
    /// once it has gone: where that store hangs, on its side of a paired
    /// record, on its own pylon of a two-pylon side and at its place on a
    /// rack, 2 ft under the pylon's surface
    /// ([`super::hardpoints::Rack::release_point`]), so it leaves from where
    /// it is drawn. A rocket leaves the pod that is firing (the outboard one
    /// first).
    pub fn release_point(&self, units_after: u32) -> Vector {
        let rack = self.rack;
        let left =
            rack.release_mount(self.mount, units_after)[0] < 0. && rack.two_sided(self.mount);
        let side_after = rack.sides(self.mount, u16::try_from(units_after).unwrap_or(u16::MAX))
            [usize::from(left)];
        let debit = u32::from(self.weapon.burst.actual_rounds_per_game).max(1);
        let slot = u32::from(side_after) * debit / pod_rounds(&self.weapon).max(1);
        rack.release_point(
            self.mount,
            self.count,
            units_after,
            u16::try_from(slot).unwrap_or(u16::MAX),
            self.size(),
        )
    }
    /// Where a gun pod's rounds leave, feet right, up and forward of the
    /// aircraft: the nose of the pod as it hangs on its station, half its
    /// length ahead of its middle ([`Self::release_point`]), so a pod fires
    /// from where it is drawn.
    pub fn muzzle(&self) -> Vector {
        let mut at = self.release_point(0);
        at[2] += self.size().length / 2.;
        at
    }
    /// Where this gun station's rounds, and its sight's solution, start:
    /// the record's mount, except a gun pod's [`Self::muzzle`].
    pub fn gun_mount(&self) -> Vector {
        if is_gun_pod(&self.weapon) {
            self.muzzle()
        } else {
            self.mount
        }
    }
    /// How deep the pylon the store [`Self::release_point`] names hangs from
    /// is, feet from the model's underside to the store's lugs
    /// ([`super::hardpoints::Rack::store_depth`]); `None` for a wingtip rail
    /// or an internal store.
    pub fn release_depth(&self, units_after: u32) -> Option<f64> {
        let rack = self.rack;
        let left =
            rack.release_mount(self.mount, units_after)[0] < 0. && rack.two_sided(self.mount);
        let side_after = rack.sides(self.mount, u16::try_from(units_after).unwrap_or(u16::MAX))
            [usize::from(left)];
        let debit = u32::from(self.weapon.burst.actual_rounds_per_game).max(1);
        let slot = u32::from(side_after) * debit / pod_rounds(&self.weapon).max(1);
        rack.store_depth(
            self.mount,
            self.count,
            usize::from(left),
            u16::try_from(slot).unwrap_or(u16::MAX),
            self.size(),
        )
    }
    /// The size its store hangs by ([`super::store_sizes::StoreSize`]).
    pub fn size(&self) -> super::store_sizes::StoreSize {
        super::store_sizes::StoreSize::of_weapon(&self.weapon)
    }
    /// Where the store this station lets go next hangs now, while it holds
    /// `rounds` (before the release's debit): [`Self::release_point`] with
    /// the whole releases left once that store has gone. The bomb and rocket
    /// sights predict from here, for a human and the AI alike (B3 follow-up,
    /// John 2026-10-12), so the pipper shows where the next store goes.
    pub fn next_release_point(&self, rounds: u32) -> Vector {
        let debit = u32::from(self.weapon.burst.actual_rounds_per_game).max(1);
        self.release_point(super::hardpoints::units(
            rounds.saturating_sub(debit),
            debit,
        ))
    }
}
/// One selected external tank type, kept separate from weapon ammunition.
#[derive(Clone, Debug)]
pub struct TankStore {
    pub source: String,
    pub name: String,
    pub tank: tore_formats::weapons::Tank,
}
impl TankStore {
    pub fn parse(source: &str, bytes: &[u8]) -> Result<Self> {
        if !source.ends_with(".GAS") {
            return Err(super::invalid("expected tank resource"));
        }
        let brf = tore_formats::aircraft::Brf::parse(bytes)?;
        let names = brf.strings("si_names")?;
        if names.len() != 3 || !names[2].eq_ignore_ascii_case(source) {
            return Err(super::invalid(
                "tank identity differs from selected resource",
            ));
        }
        Ok(Self {
            source: source.into(),
            name: names[0].clone(),
            tank: tore_formats::weapons::Tank::parse(bytes)?,
        })
    }
    pub fn full_mass_lbs(&self) -> f64 {
        f64::from(self.tank.empty_weight) + f64::from(self.tank.fuel_weight)
    }
}
#[derive(Clone, Debug)]
pub struct TankStation {
    pub hardpoint: usize,
    pub mount: Vector,
    /// The model's underside over its pylons (PY2); the location and
    /// pairing the drawing uses are worked out from the tanks loaded.
    pub rack: super::hardpoints::Rack,
    pub store: Option<TankStore>,
    pub quantity: u16,
}
/// Reviewed external gun pod hardware and its separate ammunition budget.
#[derive(Clone, Debug)]
pub struct GunPod {
    pub station: usize,
    pub quantity: u16,
    pub rounds_per_pod: u16,
    pub weight_lbs: i32,
}
impl GunPod {
    /// The pod of `quantity` units of `weapon` (an SUU-16 record) on weapon
    /// station `station`: the host interpretation of the record's 1200
    /// projectiles as `projsInPod / actualRoundsPerGame` logical units a pod,
    /// and the hardware mass of its object record (docs/spec/aircraft-variety.md,
    /// "F-4 family contract").
    pub fn of(
        station: usize,
        quantity: u16,
        weapon: &Weapon,
        equipment: &tore_formats::aircraft::Equipment,
    ) -> Result<Self> {
        let real_rounds = i32::from(weapon.burst.projectiles_in_pod);
        let multiplier = i32::from(weapon.burst.actual_rounds_per_game);
        if real_rounds <= 0 || multiplier <= 0 || real_rounds % multiplier != 0 {
            return Err(super::invalid("unreviewed gun pod ammunition contract"));
        }
        Ok(Self {
            station,
            quantity,
            rounds_per_pod: u16::try_from(real_rounds / multiplier)
                .map_err(|_| super::invalid("gun pod round count exceeds word"))?,
            weight_lbs: equipment.object["weight"].number()?,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Configuration {
    pub ecm: tore_formats::weapons::Countermeasures,
    pub system_damage: [u8; 45],
    pub damage_capacity: i32,
    pub fragment_offsets: [Vector; 2],
    pub afterburner_available: bool,
    pub hardpoint_slots: Vec<Option<usize>>,
    pub radar_hardpoint: Option<usize>,
    pub visual_hardpoint: usize,
    pub infrared_hardpoint: Option<usize>,
    pub rwr_hardpoint: Option<usize>,
    pub ecm_hardpoint: Option<usize>,
    pub aircraft: AircraftId,
    /// How the aircraft bombs, read from its imported record
    /// ([`crate::models::strike_class`]).
    pub strike_class: StrikeClass,
    pub stations: Vec<Station>,
    pub hit_points: i32,
    pub target_category: u16,
    /// Source fixed external hardware, excluding selectable tanks and gun pods.
    pub fixed_external_equipment_lbs: i32,
    pub tanks: Vec<TankStation>,
    pub gun_pods: Vec<GunPod>,
    pub external_equipment_lbs: i32,
    pub external_fuel_lbs: [f64; 9],
    pub engines: u8,
    pub wreck_power: crate::wreck::Power,
    /// Imported sensor capability, resolved by parsed record channel.
    pub sensors: sensors::SensorProfiles,
    /// The targeting sight of the aircraft's own laser or FLIR record
    /// ([`super::sight::fixed_sight`]); [`Configuration::sight_sensor`] puts
    /// a loaded designator pod first.
    pub sight: Option<super::sight::SightSensor>,
    /// Laser designator pods loaded on weapon stations (decision 7).
    pub designator_pods: Vec<super::sight::DesignatorPod>,
}
impl Configuration {
    /// The gun pod on weapon station `station`, when that station's weapon
    /// is the pod: a row that can take a pod keeps its (empty) entry while it
    /// carries something else, which is no pod.
    pub fn gun_pod(&self, station: usize) -> Option<&GunPod> {
        self.stations
            .get(station)
            .filter(|s| is_gun_pod(&s.weapon))
            .and_then(|_| self.gun_pods.iter().find(|pod| pod.station == station))
    }
    /// Convert installed quantities to logical rounds: a gun pod's rounds,
    /// and a rocket pod's rockets, `projsInPod` a pod (retail multiplies a
    /// loaded station's count by it at mission start, `HARDPodHack`; two
    /// LAU-61 pods hold 38 rockets). The high ammo bit remains reserved for
    /// station failure, so overflow is rejected.
    pub fn ammunition(&self, quantities: &[u16]) -> Result<Vec<u16>> {
        if quantities.len() != self.stations.len() {
            return Err(super::invalid("invalid ammunition station count"));
        }
        quantities
            .iter()
            .enumerate()
            .map(|(station, quantity)| {
                let multiplier = self
                    .gun_pods
                    .iter()
                    .find(|pod| pod.station == station)
                    .filter(|_| self.stations[station].weapon.source == "SUU16.JT")
                    .map_or_else(
                        || pod_rounds(&self.stations[station].weapon),
                        |pod| u32::from(pod.rounds_per_pod),
                    );
                let rounds = u32::from(*quantity) * multiplier;
                if rounds >= 32767 {
                    return Err(super::invalid(
                        "gun pod ammunition exceeds the 15-bit station limit",
                    ));
                }
                Ok(rounds as u16)
            })
            .collect()
    }
    /// Derive shell/fuel totals from explicit installed tank quantities.
    pub fn refresh_tanks(&mut self) -> Result<()> {
        if self.tanks.len() > 9 || self.fixed_external_equipment_lbs < 0 {
            return Err(super::invalid("invalid tank configuration"));
        }
        let mut mass = i64::from(self.fixed_external_equipment_lbs);
        for pod in &self.gun_pods {
            if pod.station >= self.stations.len() || pod.weight_lbs < 0 || pod.rounds_per_pod == 0 {
                return Err(super::invalid("invalid gun pod configuration"));
            }
            mass += i64::from(pod.quantity) * i64::from(pod.weight_lbs);
        }
        for pod in &self.designator_pods {
            if pod.station >= self.stations.len() || pod.store.weight_lbs < 0 {
                return Err(super::invalid("invalid designator pod configuration"));
            }
            mass += i64::from(pod.store.weight_lbs);
        }
        let mut fuel = [0.; 9];
        let mut used = [false; 9];
        for station in &self.tanks {
            let bank = tore_formats::aircraft::stations::fuel_bank(station.hardpoint)
                .ok_or_else(|| super::invalid("invalid tank fuel bank"))?;
            if station.mount.iter().any(|v| !v.is_finite()) {
                return Err(super::invalid("invalid tank station"));
            }
            if std::mem::replace(&mut used[bank], true) {
                return Err(super::invalid("duplicate tank station"));
            }
            if let Some(store) = &station.store {
                if store.tank.fuel_weight < 0 {
                    return Err(super::invalid("negative tank fuel capacity"));
                }
                let count = i64::from(station.quantity);
                mass = mass
                    .checked_add(
                        count
                            * (i64::from(store.tank.empty_weight)
                                + i64::from(store.tank.fuel_weight)),
                    )
                    .ok_or_else(|| super::invalid("tank mass overflow"))?;
                fuel[bank] += f64::from(station.quantity) * f64::from(store.tank.fuel_weight);
            } else if station.quantity != 0 {
                return Err(super::invalid("tank quantity has no selected type"));
            }
        }
        self.external_equipment_lbs =
            i32::try_from(mass).map_err(|_| super::invalid("tank mass exceeds bounds"))?;
        self.external_fuel_lbs = fuel;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let mut equipment = self.clone();
        equipment.refresh_tanks()?;
        if equipment.external_equipment_lbs != self.external_equipment_lbs
            || equipment.external_fuel_lbs != self.external_fuel_lbs
        {
            return Err(super::invalid(
                "installed hardware mass or fuel is inconsistent",
            ));
        }
        if self.stations.len() > 32
            || self.damage_capacity <= 0
            || !self
                .fragment_offsets
                .iter()
                .flatten()
                .all(|v| v.is_finite())
            || self.hit_points <= 0
            || self.external_equipment_lbs < 0
            || self.fixed_external_equipment_lbs < 0
            || !(1..=4).contains(&self.engines)
            || self
                .external_fuel_lbs
                .iter()
                .any(|fuel| !fuel.is_finite() || *fuel < 0.)
            || self.external_fuel_lbs.iter().sum::<f64>() > f64::from(self.external_equipment_lbs)
        {
            return Err(super::invalid("invalid live configuration bounds"));
        }
        for s in &self.stations {
            if let Some(profile) = missiles::Profile::for_weapon(&s.weapon) {
                profile.validate()?;
            }
            let m = &s.weapon.movement;
            if s.count >= 32767
                || s.mount.iter().any(|v| !v.is_finite())
                || m.minimum_speed < 0
                || m.maximum_speed < m.minimum_speed
                || !(0..=i32::MAX / 256).contains(&m.acceleration)
                || !(0..=i32::MAX / 256).contains(&m.deceleration)
                || s.weapon.burst.actual_rounds_per_game == 0
            {
                return Err(super::invalid("invalid live station bounds"));
            }
        }
        Ok(())
    }
    pub fn from_source(
        a: &Aircraft,
        mut read: impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let hardpoints = &a.hardpoints;
        let mut stations = Vec::new();
        let mut gun_pods = Vec::new();
        let mut external_equipment_lbs = 0i32;
        let mut external_fuel_lbs = [0.; 9];
        for (index, h) in hardpoints
            .iter()
            .enumerate()
            .filter(|(_, h)| h.flags & 8 == 0)
        {
            if let Some(name) = h.store.as_deref() {
                let weight = if name.ends_with(".GAS") {
                    let tank = tore_formats::weapons::Tank::parse(&read(name)?)?;
                    if let Some(fuel) = external_fuel_lbs.get_mut(index) {
                        *fuel = f64::from(tank.fuel_weight) * f64::from(h.count);
                    }
                    i32::from(tank.empty_weight).checked_add(tank.fuel_weight)
                } else if name.ends_with(".SEE") {
                    let e = tore_formats::aircraft::Equipment::parse(name, &read(name)?)?;
                    Some(
                        e.fields
                            .get("weight")
                            .ok_or_else(|| super::invalid("missing equipment weight"))?
                            .number()?,
                    )
                } else {
                    Some(0)
                }
                .ok_or_else(|| super::invalid("external equipment mass overflow"))?;
                if weight < 0 {
                    return Err(super::invalid("negative external equipment weight"));
                }
                external_equipment_lbs = external_equipment_lbs
                    .checked_add(
                        weight
                            .checked_mul(h.count)
                            .ok_or_else(|| super::invalid("external mass overflow"))?,
                    )
                    .ok_or_else(|| super::invalid("external mass overflow"))?;
            }
        }
        // The clean model, for the underside each pylon hangs from (PY2). A
        // missing or unreadable shape leaves stores at their records'
        // heights.
        let model = read(&a.shape)
            .ok()
            .and_then(|bytes| tore_formats::shape::Shape::parse(&bytes).ok())
            .map(|shape| super::hardpoints::Underside::of(&shape, super::debris::scale(a.id)));
        // Source record `index`'s station: where the stores hang, at the
        // real aircraft's station where T.O.R.E moves it (the F-14's).
        let place = |index: usize, h: &tore_formats::aircraft::Hardpoint| {
            super::hardpoints::placed_on(a.id, index, h, model.as_ref())
        };
        let mut tanks = Vec::new();
        for (index, h) in hardpoints.iter().enumerate() {
            let default = h.store.as_deref().filter(|name| name.ends_with(".GAS"));
            let retained_equipment = h
                .store
                .as_deref()
                .is_some_and(|name| name.ends_with(".SEE") || name.ends_with(".ECM"));
            if h.flags & 8 == 0
                && !retained_equipment
                && (h.flags & 0x200 != 0 || default.is_some())
            {
                let (mount, mut rack) = place(index, h);
                // Replacing the Eagle's old missile row with a tank retains
                // its under-pylon position, not the missiles' shoulder height.
                if a.id == AircraftId::F15 && index == 5 {
                    rack.location = super::hardpoints::Location::Wing.byte();
                }
                tanks.push(TankStation {
                    hardpoint: index,
                    mount,
                    rack,
                    store: default
                        .map(|name| TankStore::parse(name, &read(name)?))
                        .transpose()?,
                    quantity: if default.is_some() {
                        u16::try_from(h.count).map_err(|_| super::invalid("invalid tank count"))?
                    } else {
                        0
                    },
                });
            }
        }
        let tank_mass: i64 = tanks
            .iter()
            .filter_map(|s| {
                s.store.as_ref().map(|store| {
                    i64::from(s.quantity)
                        * (i64::from(store.tank.empty_weight) + i64::from(store.tank.fuel_weight))
                })
            })
            .sum();
        let fixed_external_equipment_lbs =
            i32::try_from(i64::from(external_equipment_lbs) - tank_mass)
                .map_err(|_| super::invalid("invalid fixed external hardware mass"))?;
        for (index, h) in hardpoints.iter().enumerate() {
            let Some(name) = h.store.as_deref().filter(|n| n.ends_with(".JT")) else {
                continue;
            };
            let mut weapon = Weapon::parse(name, &read(name)?)?;
            super::gunship::apply_tore_record(&mut weapon);
            if name == "SUU16.JT" {
                let equipment = tore_formats::aircraft::Equipment::parse(name, &read(name)?)?;
                gun_pods.push(GunPod::of(
                    stations.len(),
                    h.count as u16,
                    &weapon,
                    &equipment,
                )?);
            }
            // Restrict the live adapter to the actual default stations of the
            // reviewed aircraft. Catalog import never makes another type flyable.
            let permitted = match a.id {
                AircraftId::Mig29 => ["AA8.JT", "GSH301.JT"].contains(&name),
                AircraftId::Su27 => ["AA11.JT", "AA12.JT", "GSH301.JT"].contains(&name),
                AircraftId::Mig21 => ["AA2.JT", "GSH23.JT"].contains(&name),
                AircraftId::Su25 => ["AA8.JT", "AS7.JT", "B13.JT", "GSH301.JT"].contains(&name),
                AircraftId::Mig23 => ["AS7.JT", "B8.JT", "GSH6_30.JT"].contains(&name),
                AircraftId::Su35 => ["AA11B.JT", "AA12.JT", "AAML.JT", "GSH301.JT"].contains(&name),
                AircraftId::F22 | AircraftId::F22n | AircraftId::Faxx => {
                    ["AGM65G.JT", "AIM120.JT", "AIM9X.JT", "M61.JT"].contains(&name)
                }

                AircraftId::F18 => ["M61.JT", "AIM120.JT", "AGM65G.JT", "AIM9M.JT"].contains(&name),
                AircraftId::F14 => ["M61.JT", "AIM54C.JT", "AIM120.JT", "AIM9M.JT"].contains(&name),
                AircraftId::A4E => ["MK12.JT", "MK82.JT", "LAU61.JT"].contains(&name),
                AircraftId::X31 => ["M61.JT", "AIM120.JT", "AGM65G.JT", "AIM9X.JT"].contains(&name),
                AircraftId::Rafale => {
                    ["DEFA.JT", "AGM65G.JT", "MICA.JT", "R530.JT", "R550.JT"].contains(&name)
                }
                AircraftId::C130 => false,
                AircraftId::Ac130 => ["C_105.JT", "C_25.JT", "C_40.JT"].contains(&name),
                AircraftId::E3 => false,
                AircraftId::Il76 => false,
                AircraftId::E2 => false,
                AircraftId::Av8 => ["AGM65G.JT", "AIM9M.JT", "GAU12.JT"].contains(&name),
                AircraftId::Yak141 => ["AA10.JT", "AA11.JT", "GSH30.JT"].contains(&name),
                AircraftId::V22 => ["T30_1.JT"].contains(&name),
                AircraftId::Ah64 => ["AIM9M.JT", "LAU61.JT", "M61.JT"].contains(&name),
                AircraftId::Mi24 => ["AT2.JT", "T12_4.JT"].contains(&name),
                AircraftId::Ch47 => false,
                AircraftId::Mig17 => ["GSH23.JT", "GSH30.JT"].contains(&name),
                AircraftId::F4B => ["AIM9B.JT", "MK82.JT"].contains(&name),
                AircraftId::F4J => ["AIM7E.JT", "AIM9B.JT", "SUU16.JT"].contains(&name),
                AircraftId::F4E => ["AGM65G.JT", "AIM7.JT", "M61.JT"].contains(&name),
                AircraftId::F4G => ["AGM88.JT", "AIM7.JT", "M61.JT"].contains(&name),
                AircraftId::A7 => ["AGM65G.JT", "AIM9M.JT", "M61.JT", "MK82.JT"].contains(&name),
                AircraftId::F15 => ["AIM120.JT", "AIM9M.JT", "M61.JT"].contains(&name),
                AircraftId::F16C => {
                    ["AGM65G.JT", "AIM120.JT", "AIM9M.JT", "M61.JT"].contains(&name)
                }
                AircraftId::F104 => ["AIM7.JT", "AIM9M.JT", "M61.JT"].contains(&name),
                AircraftId::A10 => ["AGM65G.JT", "GAU8.JT"].contains(&name),
                AircraftId::B747 => false,
                AircraftId::A310 => false,
            };
            if weapon.movement.acceleration > i32::MAX / 256
                || weapon.movement.deceleration > i32::MAX / 256
            {
                return Err(super::invalid("live acceleration exceeds fixed8 domain"));
            }
            if !permitted
                || h.count <= 0
                || h.count > 32766
                || weapon.burst.actual_rounds_per_game == 0
            {
                return Err(super::invalid("unreviewed live-fire station"));
            }
            let (mount, rack) = place(index, h);
            stations.push(Station {
                weapon,
                mount,
                rack,
                count: h.count as u16,
                // Source bit 8 fixes the loading slot; the reviewed SUU16 pod
                // is still separate external hardware rather than an internal gun.
                internal: h.flags & 8 != 0 && name != "SUU16.JT",
            });
        }
        // Preserve every existing default-JT index. Additional source rows
        // retain a real compatible selection at zero quantity, never a fake store.
        let mut hardpoint_slots = vec![None; hardpoints.len()];
        let mut slot = 0;
        for (hardpoint, h) in hardpoints.iter().enumerate() {
            if h.store.as_deref().is_some_and(|name| name.ends_with(".JT")) {
                hardpoint_slots[hardpoint] = Some(slot);
                slot += 1;
            }
        }
        let default_weapons: Vec<_> = stations
            .iter()
            .map(|station| station.weapon.clone())
            .collect();
        for (hardpoint, h) in hardpoints.iter().enumerate() {
            if hardpoint_slots[hardpoint].is_some() || h.flags & 8 != 0 {
                continue;
            }
            let source_station = super::loading::Station::from_source(h)?;
            let compatible = |weapon: &Weapon| {
                // The added Eagle carrier retains an empty bomb selection,
                // even though its laser-bomb (0x80) and SARH (0x40) bits also match
                // missiles; Loadout::capacity applies the same bomb-only rule.
                (a.id != AircraftId::F15
                    || hardpoint != 6
                    || super::ballistics::is_bomb(weapon)
                    || super::guided::is_glide_bomb(weapon))
                    && weapon.source != "SUU16.JT"
                    && (1..32767).contains(
                        &source_station.allowed_count(super::loading::Store::weapon(weapon), false),
                    )
            };
            let mut selected = default_weapons
                .iter()
                .find(|weapon| compatible(weapon))
                .cloned();
            if selected.is_none() {
                let fallback = match (a.id, hardpoint) {
                    (AircraftId::Mig23, 5) => Some("AIM9M.JT"),
                    (AircraftId::Mig17, 3) | (AircraftId::F15, 6) => Some("MK82.JT"),
                    _ => None,
                };
                if let Some(name) = fallback {
                    let weapon = Weapon::parse(name, &read(name)?)?;
                    if compatible(&weapon) {
                        selected = Some(weapon);
                    }
                }
            }
            if let Some(weapon) = selected {
                hardpoint_slots[hardpoint] = Some(stations.len());
                let (mount, rack) = place(hardpoint, h);
                stations.push(Station {
                    weapon,
                    mount,
                    rack,
                    count: 0,
                    internal: false,
                });
            }
        }
        // A source row that takes a gun pod in place of its stores (the
        // F-4B's centreline) carries an empty pod entry: the pod is loaded
        // when that station's weapon is the SUU-16 and not otherwise
        // (`Loadout::refresh_equipment`).
        if let Some(slot) = tore_formats::aircraft::stations::gun_pod_row(a.id)
            .and_then(|row| hardpoint_slots.get(row).copied().flatten())
            && !gun_pods.iter().any(|pod| pod.station == slot)
            && let Ok(bytes) = read("SUU16.JT")
        {
            let weapon = Weapon::parse("SUU16.JT", &bytes)?;
            let equipment = tore_formats::aircraft::Equipment::parse("SUU16.JT", &bytes)?;
            gun_pods.push(GunPod::of(slot, 0, &weapon, &equipment)?);
        }
        let hit_points = a
            .object
            .get("hitPoints")
            .ok_or_else(|| super::invalid("missing aircraft hit points"))?
            .number()?;
        if hit_points <= 0 {
            return Err(super::invalid("invalid live-fire aircraft configuration"));
        }
        // Sensors resolve by parsed record channel, so a missing device is an
        // explicit state rather than another aircraft's radar.
        let profiles = sensors::SensorProfiles::from_source(a, &mut read)?;
        let sight = super::sight::fixed_sight(a, &mut read)?;
        let station = |record: Option<&String>| {
            record.and_then(|record| {
                hardpoints.iter().position(|h| {
                    h.store
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(record))
                })
            })
        };
        let radar_hardpoint = station(profiles.radar.as_ref().map(|r| &r.record));
        let visual_hardpoint = station(profiles.visual.as_ref().map(|v| &v.record))
            .ok_or_else(|| super::invalid("missing reviewed visual sensor"))?;
        let infrared_hardpoint = station(profiles.infrared.as_ref().map(|i| &i.record));
        let ecm_hardpoint = hardpoints
            .iter()
            .position(|h| h.store.as_deref().is_some_and(|n| n.ends_with(".ECM")));
        let ecm = if let Some(index) = ecm_hardpoint {
            let name = hardpoints[index].store.as_deref().unwrap();
            tore_formats::weapons::Countermeasures::parse(name, &read(name)?)?
        } else {
            tore_formats::weapons::Countermeasures::NONE
        };
        let mut rwr_hardpoint = None;
        for (index, h) in hardpoints.iter().enumerate() {
            if let Some(name) = h.store.as_deref().filter(|name| name.ends_with(".SEE")) {
                let equipment = tore_formats::aircraft::Equipment::parse(name, &read(name)?)?;
                if equipment
                    .fields
                    .get("sig")
                    .map(|v| v.number())
                    .transpose()?
                    == Some(4)
                {
                    rwr_hardpoint = Some(index);
                }
            }
        }
        let mut system_damage = [0; 45];
        for (i, out) in system_damage.iter_mut().enumerate() {
            *out = a
                .fields
                .get(&format!("systemDamage[{i}]"))
                .ok_or_else(|| super::invalid("missing system damage"))?
                .number()? as u8;
        }
        let damage_capacity = hit_points
            .checked_mul(2)
            .filter(|v| *v <= i32::from(i16::MAX))
            .ok_or_else(|| super::invalid("native player damage capacity"))?;
        let engines = a
            .fields
            .get("engines")
            .map(|v| v.number())
            .transpose()?
            .unwrap_or(1)
            .clamp(1, 4) as u8;
        let fuel = a.number("internalFuel") + external_fuel_lbs.iter().sum::<f64>();
        external_equipment_lbs = i32::try_from(
            i64::from(external_equipment_lbs)
                + gun_pods
                    .iter()
                    .map(|pod| i64::from(pod.quantity) * i64::from(pod.weight_lbs))
                    .sum::<i64>(),
        )
        .map_err(|_| super::invalid("gun pod equipment mass exceeds bounds"))?;
        let mass = a
            .object
            .get("weight")
            .map(|v| v.number())
            .transpose()?
            .unwrap_or(1) as f64
            + a.number("internalFuel")
            + f64::from(external_equipment_lbs)
            + stations
                .iter()
                .filter(|s| !s.internal && s.weapon.source != "SUU16.JT")
                .map(|s| f64::from(s.weapon.weight) * f64::from(s.count))
                .sum::<f64>();
        let wreck_power = crate::wreck::Power::symmetric(
            engines,
            a.number("thrust") * 0.7 / mass.max(1.) * 32.174,
            fuel / (a.number("fuelConsumption") * 0.7).max(0.001),
        );
        Self {
            fragment_offsets: [
                super::debris::attachment(a.id, 0, &mut read)?,
                super::debris::attachment(a.id, 1, &mut read)?,
            ],
            ecm,
            system_damage,
            damage_capacity,
            afterburner_available: a
                .fields
                .get("aftThrust")
                .ok_or_else(|| super::invalid("missing afterburner thrust"))?
                .number()?
                != 0,
            hardpoint_slots,
            visual_hardpoint,
            radar_hardpoint,
            infrared_hardpoint,
            rwr_hardpoint,
            ecm_hardpoint,
            sensors: profiles,
            sight,
            designator_pods: vec![],
            fixed_external_equipment_lbs,
            tanks,
            gun_pods,
            external_equipment_lbs,
            external_fuel_lbs,
            wreck_power,
            engines,
            aircraft: a.id,
            strike_class: StrikeClass::of_aircraft(a),
            stations,
            hit_points,
            target_category: a
                .object
                .get("obj_class")
                .ok_or_else(|| super::invalid("missing object category"))?
                .number()? as u16,
        }
        .with_default_load(a, &mut read)
    }
    /// The aircraft's default quantities where T.O.R.E's default load
    /// differs from the source records' ([`super::hardpoints::default_records`]:
    /// the A-4E's real three tanks): the rows' weapons and capacities stay the
    /// source's, only what starts loaded changes.
    fn with_default_load(
        mut self,
        a: &Aircraft,
        read: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let records = super::hardpoints::default_records(a);
        let mut changed = false;
        for (hardpoint, (source, record)) in a.hardpoints.iter().zip(&records).enumerate() {
            if source.store == record.store && source.count == record.count {
                continue;
            }
            changed = true;
            let weapon = record.store.as_deref().filter(|n| n.ends_with(".JT"));
            if let Some(Some(slot)) = self.hardpoint_slots.get(hardpoint) {
                self.stations[*slot].count = if weapon.is_some() {
                    u16::try_from(record.count).map_err(|_| super::invalid("invalid count"))?
                } else {
                    0
                };
            }
            if let Some(tank) = self.tanks.iter_mut().find(|t| t.hardpoint == hardpoint) {
                match record.store.as_deref().filter(|n| n.ends_with(".GAS")) {
                    Some(name) => {
                        tank.store = Some(TankStore::parse(name, &read(name)?)?);
                        tank.quantity = u16::try_from(record.count)
                            .map_err(|_| super::invalid("invalid tank count"))?;
                    }
                    None => {
                        tank.store = None;
                        tank.quantity = 0;
                    }
                }
            }
        }
        // Shared by human and AI default loads. Keep source records intact;
        // every physical two-sided carrier loads whole pairs.
        for station in &mut self.stations {
            let policy = super::loading::StorePairs::of(station.rack, station.mount);
            station.count = policy.maximum(i32::from(station.count)) as u16;
        }
        for tank in &mut self.tanks {
            let quantity = super::loading::StorePairs::of(tank.rack, tank.mount)
                .maximum(i32::from(tank.quantity)) as u16;
            changed |= quantity != tank.quantity;
            tank.quantity = quantity;
        }
        if changed {
            self.refresh_tanks()?;
        }
        Ok(self)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub aircraft: Option<AircraftId>,
    pub role: TargetRole,
    pub heat: Heat,
    pub radar_emitting: bool,
    pub id: u32,
    pub position: Vector,
    /// Ground-relative velocity, also used for the notch projection.
    pub velocity: Vector,
    pub basis: Basis,
    pub configuration: sensors::Configuration,
    pub signature: sensors::SignatureProfile,
    pub jammer: Option<sensors::JammerProfile>,
    pub jammer_active: bool,
    /// Physical airborne presence. Hit points reaching zero does not clear it.
    pub airborne: bool,
    /// Parked or rolling on a runway. Radar cannot see it (manual p.208);
    /// other sensors are unchanged. See [`sensors::Observable::on_ground`].
    pub on_ground: bool,
    pub wreck: Option<crate::wreck::Wreck>,
    pub wreck_power: crate::wreck::Power,
    pub radius: f64,
    pub hp: i32,
    pub initial_hp: i32,
    pub fragment_offsets: [Vector; 2],
    pub fragment_released: bool,
    pub localized_damage: LocalizedDamage,
    /// System faults hits have caused; always Realistic, whatever the
    /// player's Damage cheat says.
    pub faults: SystemFaults,
    pub category: u16,
    /// The side of an aircraft, set by the host when the row is added;
    /// [`NO_SIDE`] for anything else.
    pub side: Side,
}

/// An aircraft target's system faults, rolled on each hit by the same rules
/// as the player's. The host hands the counts to the aircraft's own systems.
/// A target without a fault table, such as a ground object, never rolls.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemFaults {
    table: [u8; 45],
    afterburner_available: bool,
    /// Damage taken, not capped at the hit points.
    damage: i32,
    pub counts: [u8; 45],
}
impl Default for SystemFaults {
    fn default() -> Self {
        Self {
            table: [0; 45],
            afterburner_available: false,
            damage: 0,
            counts: [0; 45],
        }
    }
}
impl SystemFaults {
    pub fn new(config: &Configuration) -> Self {
        Self {
            table: config.system_damage,
            afterburner_available: config.afterburner_available,
            ..Self::default()
        }
    }
    fn hit(&mut self, amount: i32, capacity: i32, roll: impl FnMut(u16) -> u16) {
        if self.table.iter().all(|entry| entry & 15 == 0) {
            return;
        }
        self.damage = self.damage.saturating_add(amount.max(0));
        for index in super::systems::hit_faults(
            &self.table,
            &self.counts,
            self.damage,
            capacity,
            amount,
            self.afterburner_available,
            roll,
        ) {
            self.counts[index] = self.counts[index].saturating_add(1);
        }
    }
}

pub const DAMAGE_SECTIONS: usize = 6;
/// Contact sphere for an aircraft, feet.
pub const AIRCRAFT_RADIUS_FT: f64 = 28.;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageSection {
    Nose = 0,
    Cockpit = 1,
    Core = 2,
    LeftWing = 3,
    RightWing = 4,
    Tail = 5,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalizedDamage {
    pub amounts: [i32; DAMAGE_SECTIONS],
    /// First reviewed A/C breakup pair whose local threshold was crossed.
    pub structural_variant: Option<usize>,
    pub structural_section: Option<DamageSection>,
}
impl LocalizedDamage {
    pub fn fractions(&self, initial_hp: i32) -> [f64; DAMAGE_SECTIONS] {
        self.amounts
            .map(|amount| (f64::from(amount) / f64::from(initial_hp.max(1))).clamp(0., 1.))
    }
    pub fn section(position: Vector, target: &Target) -> DamageSection {
        let offset = sub(position, target.position);
        let forward = crate::attitude::dot(offset, target.basis.forward) / target.radius.max(1.);
        let right = crate::attitude::dot(offset, target.basis.right) / target.radius.max(1.);
        let up = crate::attitude::dot(offset, target.basis.up) / target.radius.max(1.);
        if forward > 0.28 && up > 0.18 && right.abs() < 0.28 {
            DamageSection::Cockpit
        } else if forward > 0.45 {
            DamageSection::Nose
        } else if forward < -0.48 {
            DamageSection::Tail
        } else if right < -0.38 {
            DamageSection::LeftWing
        } else if right > 0.38 {
            DamageSection::RightWing
        } else {
            DamageSection::Core
        }
    }
    pub fn section_segment(from: Vector, to: Vector, target: &Target) -> DamageSection {
        Self::contact(from, to, target, 1.)
            .map(|(_, section)| section)
            .unwrap_or_else(|| Self::section(to, target))
    }
    /// `scale` enlarges every section, as Easy aiming does for the player's rounds.
    fn contact(
        from: Vector,
        to: Vector,
        target: &Target,
        scale: f64,
    ) -> Option<(f64, DamageSection)> {
        aircraft_contact(
            from,
            to,
            target.position,
            target.basis,
            target.radius * scale,
        )
    }

    fn record(&mut self, section: DamageSection, amount: i32, initial_hp: i32) {
        let slot = section as usize;
        self.amounts[slot] = self.amounts[slot].saturating_add(amount.max(0));
        let threshold = (initial_hp.max(1) * 3 + 3) / 4;
        if self.amounts[slot] >= threshold && self.structural_variant.is_none() {
            self.structural_variant = match section {
                DamageSection::Nose | DamageSection::Cockpit | DamageSection::Core => Some(0),
                DamageSection::LeftWing | DamageSection::RightWing | DamageSection::Tail => Some(1),
            };
            self.structural_section = Some(section);
        }
    }
}
impl Target {
    /// A destroyed aircraft remains a solid body until its wreck disappears.
    pub fn body_present(&self) -> bool {
        self.hp > 0 || (self.role == TargetRole::Aircraft && self.airborne)
    }
    pub fn damage_fraction(&self) -> f64 {
        (1. - f64::from(self.hp.max(0)) / f64::from(self.initial_hp.max(1))).clamp(0., 1.)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub id: u32,
    /// The aircraft that fired this round: an ownship's aircraft id or an AI
    /// actor's. Score counters are attributed with it, so an AI aircraft
    /// killing another AI aircraft credits no ownship.
    pub owner: u32,
    /// Actor-owned weapon for AI releases; player shots use the configured station.
    pub weapon: Option<Weapon>,
    pub guidance: Option<Flight>,
    pub motion: Option<Motion>,
    pub guidance_ticks: Option<u64>,
    pub age: u64,
    /// The ownship this round was aimed at when it was released, if any: aim
    /// metadata, fixed then and never updated. It no longer decides who the
    /// round can hit. The ledger's aim and the warnings read it, and the
    /// diagnostic incoming round is always treated as illuminated.
    pub incoming: Option<u32>,
    pub station: usize,
    pub position: Vector,
    pub previous: Vector,
    pub direction: Vector,
    pub speed_f8: i32,
    pub launched_t: u16,
    pub target: Option<u32>,
    pub fall: FallState,
    /// Physical gun-round position within the source representative debit.
    pub gun_round: Option<u8>,
    /// Fitted presentation marker: every third physical gun round.
    pub tracer: bool,
}

/// The same measured aircraft volume for predicted and live gun contact.
pub fn aircraft_contact(
    from: Vector,
    to: Vector,
    position: Vector,
    basis: Basis,
    radius: f64,
) -> Option<(f64, DamageSection)> {
    let local = |point: Vector| {
        let offset = sub(point, position);
        let radius = radius.max(1.);
        [
            crate::attitude::dot(offset, basis.right) / radius,
            crate::attitude::dot(offset, basis.up) / radius,
            crate::attitude::dot(offset, basis.forward) / radius,
        ]
    };
    let a = local(from);
    let b = local(to);
    let boxes = [
        (
            DamageSection::Cockpit,
            [-0.22, 0.08, 0.08],
            [0.22, 0.48, 0.48],
        ),
        (
            DamageSection::Core,
            [-0.28, -0.28, -0.38],
            [0.28, 0.22, 0.18],
        ),
        (
            DamageSection::Nose,
            [-0.32, -0.30, 0.42],
            [0.32, 0.32, 0.92],
        ),
        (
            DamageSection::LeftWing,
            [-0.92, -0.18, -0.28],
            [-0.25, 0.18, 0.38],
        ),
        (
            DamageSection::RightWing,
            [0.25, -0.18, -0.28],
            [0.92, 0.18, 0.38],
        ),
        (
            DamageSection::Tail,
            [-0.34, -0.25, -0.92],
            [0.34, 0.40, -0.34],
        ),
    ];
    boxes
        .into_iter()
        .filter_map(|(section, lo, hi)| segment_box_fraction(a, b, lo, hi).map(|at| (at, section)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// One actor's current fire-control answer. The host replaces these snapshots
/// every fixed tick. A projectile can consume only the entry matching its owner
/// and the observation matching its retained target identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorSupport {
    pub owner: u32,
    pub observation: Option<seeker::Observation>,
    pub supported: bool,
    pub radar_position: Vector,
    pub radar_emitting: bool,
}
impl Projectile {
    /// Physical bodies can remain after their ability to threaten has ended.
    pub fn threatening(&self) -> bool {
        !self
            .motion
            .and_then(|m| m.energy)
            .is_some_and(|e| e.flight.defeated || e.decoy.pursuit.is_some_and(|p| p.passed))
    }

    pub fn weapon<'a>(&'a self, config: &'a Configuration) -> &'a Weapon {
        self.weapon
            .as_ref()
            .unwrap_or_else(|| &config.stations[self.station].weapon)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    Flare,
    Chaff,
    Launch,
    Hit,
    Destroyed,
    Ground,
    DebrisImpact,
    /// A flak shell's burst in the air: its record's explosion, no tracer
    /// before it, a flash of light (docs/spec/surface-defenses.md, "Flak").
    Flak,
}
/// A round that went off on the ground with a splash this step: where, how
/// far its collateral reaches, and the percent of the record's damage it
/// passes on at the burst (its record's percent times the ground share).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundBurst {
    pub position: Vector,
    pub radius: f64,
    pub percent: i32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub position: Vector,
    pub kind: EffectKind,
    pub ticks: u16,
    /// The original explosion type drawn and heard, after its variety
    /// roll; `None` for launches, decoys, debris landing and effects from
    /// recordings older than explosion types.
    pub blast: Option<u8>,
    /// How big the explosion is drawn against its type's own size, percent
    /// ([`super::blast_scale`]): the weapon's for an impact, full size for
    /// everything else, [`super::blast_scale::INERT`] for a dud's puff.
    pub scale: u8,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// An ownship fired a round from one of its stations.
    Fired {
        aircraft: u32,
        station: usize,
    },
    SeekerActivated(u32),
    Pitbull(u32),
    Hit(u32),
    Destroyed(u32),
    Airburst(u32),
    Ground,
    TrackLost(u32),
    /// A hit took hit points from an ownship.
    OwnshipDamaged {
        aircraft: u32,
        amount: i32,
    },
    /// A hit or accumulated damage faulted one of an ownship's subsystems.
    SubsystemDamaged {
        aircraft: u32,
        index: usize,
    },
    OwnshipDestroyed {
        aircraft: u32,
    },
    PilotKilled {
        aircraft: u32,
    },
    OwnshipGroundImpact {
        aircraft: u32,
    },
    /// A jammer defeated a missile or bomb aimed at this aircraft.
    Defeated(u32),
    /// A missile or bomb burst on an aircraft; the host knocks it around.
    Jolt(Jolt),
}
/// Blast on an aircraft, ownship or not. `strength` is the warhead's damage
/// against that aircraft over 100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jolt {
    pub target: u32,
    pub from: Vector,
    pub strength: f64,
    /// The share of the knock the aircraft takes, after its strength's
    /// clamp: 1, or [`FLAK_JOLT_SHARE`] for a flak burst.
    pub share: f64,
}
/// A flak burst knocks an aircraft about half as hard as another blast of
/// its strength; its damage is unchanged (opinionated, John, 2026-10-14:
/// "flak pushes aircraft a lot, maybe cut push forces by 50%, damage can
/// stay the same").
pub const FLAK_JOLT_SHARE: f64 = 0.5;
/// Mounted weapon audio state, independent of playback and rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeekerTone {
    pub strength: f64,
    pub ground: bool,
    pub radar: bool,
    pub locked: bool,
}
impl SeekerTone {
    /// Fitted HUD-percentage curve with the recovered half-volume search rule.
    pub fn ir_strength(percent: u8, locked: bool) -> f64 {
        (0.15 + 0.85 * f64::from(percent.min(100)) / 100.) * if locked { 1. } else { 0.5 }
    }
}

#[derive(Clone, Copy, Debug)]
struct RangeEstimate {
    station: usize,
    target: u32,
    mode: LaunchMode,
    minimum: f64,
    maximum: f64,
    favorable: Option<missiles::FiringBand>,
}

/// The combat state of one human-flown aircraft: its stores and selection, its
/// seeker and sensors, its hit points and system faults, its countermeasures
/// and warnings, its score. Everything only that aircraft has lives here; what
/// every aircraft shares (projectiles, targets, effects, the ledger, the random
/// stream, the mission settings) stays in [`State`], which holds the ownships in
/// aircraft id order.
#[derive(Clone, Debug)]
pub struct Ownship {
    /// The aircraft this is: its id in projectile owners, ledger keys and events.
    pub aircraft: u32,
    /// The side the aircraft flies for. Set by the host.
    pub side: Side,
    config: Configuration,
    /// External stores are fitted.
    external: bool,
    pub release_readiness: Readiness,
    /// The strike cockpit: the selected station and armed state, the seeker
    /// mode, the bomb sight's mode, the targeting sight, the laser and the
    /// target (the designated object, copied from the sensors each step).
    /// The ownship reads as it (`own.selected` is `own.cockpit.selected`);
    /// a kept cockpit and an AI striker hold the same record
    /// ([`cockpit`], slice ST-C).
    pub cockpit: StrikeCockpit,
    pub mounted: Seeker,
    pub heat: Heat,
    energy_hit: Option<u32>,
    /// Provisional bore return for HUD estimates only, never a designation or lock.
    pub bore_observation: Option<seeker::Observation>,
    mounted_key: Option<(usize, LaunchMode, Option<u32>)>,
    range_estimate: Option<RangeEstimate>,
    pub ammo: Vec<u16>,
    /// Stations that held something at the start of the mission. A station
    /// emptied in flight keeps its row in the weapons window; one that was
    /// never loaded has none. See `note_loaded`.
    ever_loaded: Vec<bool>,
    /// AC-130 fixed-tick barrel angles and linked membership.
    pub gunship: Option<super::gunship::State>,
    /// The selected guided air-to-ground weapon's sight (aim, reach and
    /// advisory), worked out every step for the cockpit; never read back by
    /// combat, so a checkpoint does not carry it.
    pub guided_sight: Option<super::guided::GuidedSight>,
    pub sensors: Sensors,
    /// Easy targeting's memory of the last selection, kept after sensor
    /// loss for the HUD square only; never grants weapon support.
    hud_selection: Option<u32>,
    /// Without Easy targeting, the last selection while the pilot can still
    /// see it after the sensors drop it. Only the flight views follow it.
    sight_hold: Option<u32>,
    /// Friendly aircraft identities, which T and Enter skip. Set by the host.
    pub friendlies: std::collections::BTreeSet<u32>,
    /// The radars painting this aircraft (the RWR's bright squares), set by
    /// the host every tick: the HARM boresight candidate prefers them.
    painted_by: std::collections::BTreeSet<u32>,
    /// The hidden shoulder-fired SAM teams this pilot has spotted, or seen
    /// launch ([`GroundListing::team`]): only these reach its sensors' pages
    /// and map (T.O.R.E opinionated, John 2026-10-29).
    spotted_teams: std::collections::BTreeSet<u32>,
    /// The HARM boresight cue's emitters, worked out with the observations
    /// each step; for the HUD only, never a designation or a lock.
    harm_cue: Vec<CueEmitter>,
    /// Passive emitters received this step, for the exposure instrument.
    pub emitters: Vec<passive::Emitter>,
    pub missile_threats: super::threats::ThreatService,
    pub hp: i32,
    pub damage: i32,
    pub subsystem_counts: [u8; 45],
    pub last_subsystem: Option<usize>,
    pub radar_failed: bool,
    pub visual_failed: bool,
    pub infrared_failed: bool,
    pub rwr_failed: bool,
    pub ecm_failed: bool,
    pub chaff: u8,
    pub flares: u8,
    /// Regional damage; public so a handoff can carry it over.
    pub localized_damage: LocalizedDamage,
    fragment_released: bool,
    explosion_reported: bool,
    /// Rounds fired.
    pub shots: u32,
    pub hits: u32,
    pub kills: u32,
    pending_damage: bool,
    previous_position: Option<Vector>,
    triggers: Vec<PlayerTrigger>,
    gun_cadence: Vec<GunCadence>,
    /// Tab is down: the gun fires whatever station is selected. Not part of
    /// a checkpoint; a restored flight starts with the key up.
    gun_key: bool,
    /// A trigger press waiting for the weapon bay to open: the station and
    /// the tick it was pressed.
    bay_release: Option<(usize, u64)>,
    /// The bay stays open until this tick after a bay release.
    bay_hold_until: u64,
    /// The bomb or rocket sight's solution for the selected station, worked
    /// out every step from that step's launcher ([`super::delivery::solve`]).
    /// Derived each step, so a checkpoint does not carry it.
    pub delivery: Option<super::delivery::Delivery>,
}

/// Combat: what every aircraft shares, and the [`Ownship`] of each
/// human-flown aircraft.
#[derive(Clone, Debug)]
pub struct State {
    /// The weapon rules of the mission.
    pub weapon_rules: Rules,
    /// Human-flown aircraft, in aircraft id order.
    ownships: Vec<Ownship>,
    pub projectiles: Vec<Projectile>,
    /// Stores confirmed inert in the latest combat tick, even if removed.
    ended_missile_threats: Vec<u32>,
    pub targets: Vec<Target>,
    /// The targets the other ownships make of their aircraft, as the last step
    /// left them; empty with fewer than two ownships.
    ownship_rows: Vec<Target>,
    /// Fire-control answers by owner and the target each observes: an owner
    /// may guide missiles at several targets (a ship's two SAM systems).
    actor_support: BTreeMap<(u32, u32), ActorSupport>,
    /// Ground contact volumes keyed by stable target ID. Aircraft remain spheres.
    ground_bounds: BTreeMap<u32, crate::airport::OrientedBox>,
    pub effects: Vec<Effect>,
    /// Craters and crash-site fires, oldest first. Presentation only.
    pub marks: Vec<super::blast::Mark>,
    /// Aircraft already given a crash site, so a crash is marked once.
    crashed: std::collections::BTreeSet<u32>,
    marks_made: u64,
    blast_rolls: super::blast::Rolls,
    /// Presentation events retain emission positions independently of visual life.
    /// Bounded even when a headless host never drains them.
    sound_events: Vec<crate::acoustics::Emission>,
    pub smoke: super::smoke::Smoke,
    /// Released chaff and flares, presentation only.
    pub devices: super::countermeasures::Devices,
    pub energy_devices: missiles::decoys::Scene,
    energy_seed: u64,
    energy_predictions: missiles::prediction::Service,
    /// Offline scripted probes may omit advisory cue work. AI worlds must keep this on.
    pub energy_predictions_enabled: bool,
    /// Write-only development timing, never serialized or used by decisions.
    pub measure_prediction_cost: bool,
    /// Optional host CPU clock for profiling only, never a work-budget input.
    pub prediction_cpu_clock: Option<fn() -> Option<u64>>,
    pub prediction_cpu_cost_ns: Option<u64>,
    pub prediction_cost_ns: u64,
    prediction_request_ns: u64,
    prediction_request_cpu_ns: u64,
    /// Profiling only: readable hit, readable miss, unavailable launch answers.
    pub prediction_gate_samples: [u64; 3],
    /// Player HUD point answers: hit, miss, unavailable, only while a cue exists.
    pub prediction_player_samples: [u64; 3],
    /// Every release since the host last drained them, for recordings.
    device_log: std::collections::VecDeque<DeviceNote>,
    /// The ownships' decoy rolls since the last drain, for recordings.
    decoy_log: std::collections::VecDeque<DecoyRoll>,
    /// This step's ground detonations with a splash, for scenery that only
    /// needs to know what they knocked flat (the city fill); the host drains
    /// it after each step. Not checkpointed.
    pub ground_bursts: Vec<GroundBurst>,
    pub debris: Vec<super::debris::Piece>,
    /// Launches, outcomes and kills by shooter, for the debrief.
    pub ledger: super::ledger::Ledger,
    pub target_jammer: bool,
    /// Combat's one random stream: damage spread, decoy and fault rolls,
    /// jammer deception, whichever aircraft they concern.
    rng: u32,
    pub history: Vec<HitRecord>,
    strikes: Vec<Strike>,
    pub range_category: u16,
    next_target_id: u32,
    /// The number of the next round an ownship fires; one counter for all of
    /// them, so their projectile numbers never meet.
    next_shot: u32,
    tick: u64,
    service_remainder: u16,
    /// Mission settings from the host, including the Damage setting.
    pub cheats: crate::cheats::Cheats,
    /// The mission's friendly-fire setting.
    pub friendly_fire: FriendlyFire,
    /// The last second of every aircraft's hit volume, for rewound gun
    /// rounds. Mission state: a checkpoint carries it.
    volumes: rewind::History,
    /// The rewind, in ticks, of every round in flight that has one, by
    /// projectile number: gun rounds a human fired with a view. Mission state
    /// like the rounds themselves.
    rewinds: BTreeMap<u32, u16>,
    /// What each surface unit's round in flight carries beyond an aircraft's
    /// round, by projectile number ([`State::fire_surface`]).
    surface_rounds: BTreeMap<u32, SurfaceRound>,
    /// The number of the next surface unit's round.
    next_surface_shot: u32,
    /// The path of each bomb in flight, by projectile number: bomb records
    /// fall on the closed-form bomb equation ([`super::ballistics`]), so a
    /// bomb keeps its release rather than a velocity. Mission state.
    bombs: BTreeMap<u32, super::ballistics::BombMotion>,
    jettisoned: BTreeMap<u32, bool>,
    /// Where each jettisoned store and each guided bomb falling blind has
    /// got to along its stored fall, by projectile number, so a fall steps
    /// once a tick rather than again from its release. Not mission state:
    /// a checkpoint leaves it out and it is rebuilt from `bombs` and
    /// `guided` with the identical result.
    falls: super::ballistics::FallCursors,
    /// The guidance of each guided air-to-ground weapon an ownship released
    /// and is in flight, by projectile number ([`guided`]). Mission state.
    guided: BTreeMap<u32, guided::GuidedFlight>,
    /// The tick each aircraft's last guided air-to-ground weapon ended (hit
    /// or went away), for the targeting page's impact cue.
    guided_impacts: BTreeMap<u32, u64>,
    /// What each human left set in the cockpit of an aircraft it gave back
    /// to the AI, by aircraft, until a human takes it again
    /// ([`State::keep`]). The sight and the laser step on while the AI flies
    /// it, for the weapons the human left guiding. Mission state.
    kept: BTreeMap<u32, Kept>,
    /// The strike cockpits of the AI-flown aircraft no human left, by
    /// aircraft ([`State::ai_cockpit_mut`]): what an AI striker attacks
    /// with. Their sights and lasers step as the kept ones do. Mission
    /// state.
    ai_cockpits: BTreeMap<u32, AiCockpit>,
    /// The AI-flown aircraft (a kept cockpit, an AI striker's own) with a
    /// laser weapon selected and armed, as the host last said
    /// ([`State::set_ai_laser_armed`]): their lasers run the automatic rule
    /// for it, so the spot is up before the release. Mission state.
    ai_laser_armed: BTreeSet<u32>,
    /// The aircraft flown by the AI from a cockpit a human left whose
    /// tactics want the laser on (a laser pass, the hold, a lase duty), as
    /// the host last said ([`State::set_ai_laser_on`]): their lasers run as
    /// the automatic rule would, and the human's hand setting is never
    /// written (John's decision 11; ST-INT). A human's hand off still wins.
    /// Mission state.
    ai_laser_on: BTreeSet<u32>,
    /// How each ground object that has a unit record explodes when destroyed.
    ground_looks: BTreeMap<u32, GroundLook>,
    /// The parked aircraft among the targets, by id, with their crash sites
    /// ([`State::add_parked_aircraft`]).
    parked: BTreeMap<u32, ParkedSite>,
    /// The type facts the ground pages read, by ground object
    /// ([`State::set_ground_listing`]).
    ground_listing: BTreeMap<u32, GroundListing>,
    /// The aircraft in an airfield sequence ([`State::set_airfield_attached`]).
    airfield: std::collections::BTreeSet<u32>,
    /// The weather's sensor effects ([`State::set_sensor_weather`]).
    sensor_weather: sensors::Weather,
    /// Host setup, not state: the highest the step's `ground` reaches over
    /// each square ([`State::set_ground_ceiling`]). A segment wholly above it
    /// is not sampled.
    ground_ceiling: Option<std::sync::Arc<crate::ground_ceiling::GroundCeiling>>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GunCadence {
    pending: u16,
    next_scaled: u64,
    ordinal: u64,
}
#[derive(Clone, Copy)]
pub struct Launcher {
    /// Cockpit power switch, independent of active sensor channel/transmission.
    pub radar_power: bool,
    pub position: Vector,
    pub basis: Basis,
    pub speed_fps: f64,
    pub velocity: Vector,
    pub bay_ready: bool,
    /// Radar actually transmitting. Selecting infrared stops the emission.
    pub radar: bool,
    pub jammer: bool,
    pub alive: bool,
    /// Includes a falling wreck, but excludes a body after impact or airburst.
    pub body_present: bool,
    /// Player sensor controls, applied as an input at each step so replay
    /// reproduces channel, scope range and history changes.
    pub controls: sensors::Controls,
}
/// What one ownship is given for a combat tick: whether its trigger is held
/// and its aircraft's launcher.
#[derive(Clone, Copy)]
pub struct OwnshipInput {
    pub aircraft: u32,
    pub held: bool,
    pub launcher: Launcher,
}
/// One ownship for a step: where it is in the ownship list, its launcher, where
/// the launcher was last step, and its hit-test target.
struct OwnRow {
    index: usize,
    launcher: Launcher,
    previous: Vector,
    target: Target,
}
/// What a projectile's first contact was.
#[derive(Clone, Copy)]
enum Hit {
    /// An ownship, by its place in the step's rows.
    Ownship(usize),
    /// A target row.
    Target(usize),
}
/// The configuration whose stations a projectile without its own weapon record
/// indexes: its owner's. A round nobody owns uses the first ownship's.
fn owner_configuration(ships: &[Ownship], owner: u32) -> &Configuration {
    &owner_ownship(ships, owner)
        .expect("a projectile without a weapon needs an ownship")
        .config
}
/// The weapon record of a projectile: its own, or a station of its owner's.
fn projectile_weapon<'a>(ships: &'a [Ownship], p: &'a Projectile) -> &'a Weapon {
    p.weapon
        .as_ref()
        .unwrap_or_else(|| &owner_configuration(ships, p.owner).stations[p.station].weapon)
}
fn owner_ownship(ships: &[Ownship], owner: u32) -> Option<&Ownship> {
    ships
        .iter()
        .find(|own| own.aircraft == owner)
        .or(ships.first())
}
/// The rows of the other ownships, as seen from the ownship at `index`.
/// The AC-130's gunsight and guns for one tick: the sight moves and finds
/// its aim point among every target and the other ownships' aircraft, then
/// the guns train on it. The radar selection follows the sight's track, so
/// the HUD, the scope and the guns never disagree.
fn step_gunship(
    own: &mut Ownship,
    index: usize,
    launcher: Launcher,
    targets: &[Target],
    rows: &[OwnRow],
    ground: &impl Fn(f64, f64) -> f64,
    tick: u64,
) {
    let Some(group) = own.gunship.as_mut() else {
        return;
    };
    let objects: Vec<super::gunship::SightObject> = targets
        .iter()
        .chain(peers(rows, index))
        .map(|t| super::gunship::SightObject {
            id: t.id,
            position: t.position,
            velocity: t.velocity,
            alive: t.hp > 0,
            airborne: t.airborne,
            friendly: own.friendlies.contains(&t.id),
        })
        .collect();
    let before = group.target();
    let (aim, led) = group.step_sight(&own.config, launcher, &objects, ground, tick);
    // A gun lock lays the guns on its object or point while the sight slews
    // on (slice nr-cona).
    let (aim, led) = group.locked_aim(&objects).unwrap_or((aim, led));
    group.update(&own.config, launcher, Some(aim), |from, to| {
        terrain_hit(from, to, ground).is_none()
    });
    // The pipper, from the train the fire loop below releases on: a round
    // fired this step leaves on the tick before the step advanced the clock.
    let candidate = group.slot(own.cockpit.selected);
    let wanted = std::array::from_fn(|slot| group.included[slot] || candidate == Some(slot));
    group.evaluate_impacts(
        &own.config,
        launcher,
        wanted,
        led.then_some(aim),
        tick - 1,
        ground,
    );
    let after = group.target();
    if after != before {
        // The sight's track designates its object, whatever the channels see,
        // for as long as it holds it (docs/spec/air-to-ground.md, "Visual and
        // sight designation").
        match after {
            Some(id) => own.sensors.hold(id),
            None => own.sensors.clear_selection(),
        }
        own.hud_selection = own.designated();
    }
}
fn peers(rows: &[OwnRow], index: usize) -> impl Iterator<Item = &Target> + Clone {
    rows.iter()
        .filter(move |r| r.index != index)
        .map(|r| &r.target)
}
/// What the sensors are told of a target row.
fn observable_of(t: &Target) -> Observable {
    Observable {
        id: t.id,
        position: t.position,
        velocity: t.velocity,
        basis: t.basis,
        configuration: t.configuration,
        signature: t.signature,
        jammer: t.jammer.clone(),
        jammer_active: t.jammer_active,
        radar_emitting: t.radar_emitting,
        airborne: t.airborne,
        destroyed: t.hp <= 0,
    }
    .on_ground(t.on_ground)
}
fn view<'a>(state: &'a State, own: &'a Ownship) -> OwnshipView<'a> {
    OwnshipView { state, own }
}
/// An ownship as a hit-test target, from its launcher.
fn ownship_target(own: &Ownship, launcher: Launcher) -> Target {
    Target {
        aircraft: Some(own.config.aircraft),
        role: TargetRole::Aircraft,
        heat: own.heat,
        // The A-G page does not emit for anti-radiation seekers (retail).
        radar_emitting: launcher.radar && !own.sensors.air_ground(),
        id: own.aircraft,
        position: launcher.position,
        velocity: launcher.velocity,
        basis: launcher.basis,
        // What it carries outside: the same count the RCS page and an AI's
        // view of a human seat use.
        configuration: sensors::Configuration::carrying(own.external_stores()),
        signature: own.config.sensors.signature,
        jammer: None,
        jammer_active: false,
        airborne: launcher.body_present,
        on_ground: false,
        radius: AIRCRAFT_RADIUS_FT,
        hp: if launcher.alive { own.hp } else { 0 },
        initial_hp: own.config.damage_capacity,
        fragment_offsets: own.config.fragment_offsets,
        wreck: None,
        wreck_power: crate::wreck::Power::default(),
        fragment_released: own.fragment_released,
        localized_damage: own.localized_damage.clone(),
        faults: Default::default(),
        category: own.config.target_category,
        side: own.side,
    }
}
impl Ownship {
    /// The combat state of `aircraft`, fresh from its stores and hit points.
    pub fn new(aircraft: u32, side: Side, config: Configuration, external: bool) -> Result<Self> {
        config.validate()?;
        let quantities: Vec<u16> = config
            .stations
            .iter()
            .enumerate()
            .map(|(station, s)| {
                if !s.internal && !external {
                    return 0;
                }
                if s.weapon.source == "SUU16.JT" {
                    config
                        .gun_pods
                        .iter()
                        .find(|pod| pod.station == station)
                        .map_or(s.count, |pod| pod.quantity)
                } else {
                    s.count
                }
            })
            .collect();
        let ammo = config.ammunition(&quantities)?;
        let ever_loaded = ammo.iter().map(|a| a & 0x7fff != 0).collect();
        let triggers = vec![PlayerTrigger::default(); config.stations.len()];
        let gun_cadence = vec![GunCadence::default(); config.stations.len()];
        let sensors = Sensors::new(config.sensors.clone());
        let gunship = super::gunship::State::new(&config);
        let cockpit = StrikeCockpit::new(&config);
        Ok(Self {
            aircraft,
            side,
            release_readiness: Readiness::Safe,
            cockpit,
            mounted: Seeker::default(),
            bore_observation: None,
            mounted_key: None,
            hud_selection: None,
            sight_hold: None,
            friendlies: Default::default(),
            painted_by: Default::default(),
            spotted_teams: Default::default(),
            harm_cue: Vec::new(),
            chaff: config.ecm.chaff[0],
            flares: config.ecm.flare[0],
            hp: config.damage_capacity,
            damage: 0,
            subsystem_counts: [0; 45],
            last_subsystem: None,
            radar_failed: false,
            visual_failed: false,
            infrared_failed: false,
            rwr_failed: false,
            ecm_failed: false,
            pending_damage: false,
            previous_position: None,
            external,
            range_estimate: None,
            energy_hit: None,
            heat: Heat::Unknown,
            config,
            ammo,
            ever_loaded,
            gunship,
            guided_sight: None,
            sensors,
            emitters: vec![],
            missile_threats: super::threats::ThreatService::new(aircraft),
            fragment_released: false,
            explosion_reported: false,
            localized_damage: LocalizedDamage::default(),
            shots: 0,
            hits: 0,
            kills: 0,
            triggers,
            gun_cadence,
            gun_key: false,
            bay_release: None,
            bay_hold_until: 0,
            delivery: None,
        })
    }
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub fn gun_group_label(&self) -> Option<String> {
        let group = self.gunship.as_ref()?;
        let members: Vec<_> = super::gunship::NAMES
            .into_iter()
            .zip(group.included)
            .filter_map(|(name, on)| on.then_some(name))
            .collect();
        let candidate = group
            .slot(self.selected)
            .map_or("NONE", |slot| super::gunship::NAMES[slot]);
        Some(format!(
            "Gun group: {}. Candidate: {candidate}",
            if members.is_empty() {
                "EMPTY".into()
            } else {
                members.join("+")
            }
        ))
    }
    fn gun_readiness(&self, slot: usize) -> Readiness {
        let Some(group) = &self.gunship else {
            return Readiness::Safe;
        };
        let Some(index) = group.stations[slot] else {
            return Readiness::Empty;
        };
        if !self.armed {
            Readiness::Safe
        } else if self.hp <= 0 {
            Readiness::LauncherLost
        } else if self.ammo[index] & 0x8000 != 0 {
            Readiness::StationFailed
        } else if self.rounds(index) == 0 {
            Readiness::Empty
        } else {
            group.status[slot]
        }
    }
    fn gun_group_readiness(&self) -> Option<Readiness> {
        let group = self.gunship.as_ref()?;
        let candidate = group.slot(self.selected)?;
        if group.fire_stations().is_empty() {
            return Some(Readiness::GroupEmpty);
        }
        let selected_readiness = self.gun_readiness(candidate);
        if selected_readiness != Readiness::Ready {
            return Some(selected_readiness);
        }
        let mut status = Readiness::GroupEmpty;
        for slot in (0..3).filter(|slot| group.included[*slot] && group.stations[*slot].is_some()) {
            let readiness = self.gun_readiness(slot);
            if readiness == Readiness::Ready {
                return Some(readiness);
            }
            if status == Readiness::GroupEmpty {
                status = readiness;
            }
        }
        Some(status)
    }
    pub fn damage_section(&self) -> Option<DamageSection> {
        self.localized_damage.structural_section
    }
    pub fn damage_regions(&self) -> [f64; DAMAGE_SECTIONS] {
        self.localized_damage.fractions(self.config.damage_capacity)
    }
    /// The regional damage as whole amounts, in section order: the exact
    /// values [`Ownship::damage_regions`] divides. Read-only, for mission
    /// recordings.
    pub fn damage_amounts(&self) -> [i32; DAMAGE_SECTIONS] {
        self.localized_damage.amounts
    }
    /// Whether the selected weapon sits behind bay doors that are not open yet.
    fn bay_waits(&self, launcher: Launcher) -> bool {
        !launcher.bay_ready
            && self
                .config
                .stations
                .get(self.selected)
                .is_some_and(|s| !s.internal && s.rack.location().bay())
    }
    /// The automatic bay request: a trigger press waiting on the doors, or the
    /// brief hold open after a bay release so the weapon clears them.
    pub fn bay_demand(&self, tick: u64) -> bool {
        self.bay_release.is_some() || tick < self.bay_hold_until
    }
    pub fn release(&mut self) {
        self.bay_release = None;
        for t in &mut self.triggers {
            t.release();
        }
        for cadence in &mut self.gun_cadence {
            cadence.pending = 0;
        }
    }
    pub fn rounds(&self, station: usize) -> u16 {
        self.ammo.get(station).copied().unwrap_or(0) & 0x7fff
    }
    /// Ticks (120 Hz) before a gun station's next round may leave its barrel
    /// at combat tick `tick`, 0 when it may now: what a "ready in" readout
    /// for the AC-130's slow guns (the 105 cycles every 6 seconds) would show.
    pub fn gun_ready_in(&self, station: usize, tick: u64) -> u64 {
        let (Some(cadence), Some(s)) = (
            self.gun_cadence.get(station),
            self.config.stations.get(station),
        ) else {
            return 0;
        };
        let burst = &s.weapon.burst;
        let physical = u64::from(burst.game_rounds_in_burst.max(1))
            * u64::from(burst.actual_rounds_per_game.max(1));
        cadence
            .next_scaled
            .saturating_sub(tick.saturating_mul(physical))
            .div_ceil(physical)
    }
    pub fn designated(&self) -> Option<u32> {
        self.sensors.selected()
    }
    pub fn external_fuel_lbs(&self) -> [f64; 9] {
        if self.external {
            self.config.external_fuel_lbs
        } else {
            [0.; 9]
        }
    }
    pub fn payload_lbs(&self) -> f64 {
        let absent_pods: f64 = self
            .config
            .gun_pods
            .iter()
            .filter(|pod| !self.was_loaded(pod.station))
            .map(|pod| f64::from(pod.quantity) * f64::from(pod.weight_lbs))
            .sum();
        f64::from(if self.external {
            self.config.external_equipment_lbs
        } else {
            0
        }) - if self.external { absent_pods } else { 0. }
            + self
                .config
                .stations
                .iter()
                .zip(&self.ammo)
                .filter(|(s, _)| !s.internal && s.weapon.source != "SUU16.JT")
                .map(|(s, count)| {
                    // A rocket pod's weight is the full pod's; each rocket
                    // fired takes its share (fitted: retail's store weight
                    // for a part-empty pod is not read).
                    s.weapon.carried_weight_lb() * f64::from(*count & 0x7fff)
                        / f64::from(pod_rounds(&s.weapon))
                })
                .sum::<f64>()
    }
}
impl Ownship {
    /// Selection ring: NAV, then each configured weapon station that carries
    /// something (an empty station is not on the aircraft) and that Guns only
    /// allows.
    fn cycle_selection(&mut self, forward: bool, guns_only: bool, unlimited_ammo: bool) {
        let count = self.ammo.len();
        let current = if self.armed { self.selected + 1 } else { 0 };
        let mut next = current;
        // Air combat guns only skips every station but the gun.
        loop {
            next = if forward {
                (next + 1) % (count + 1)
            } else {
                (next + count) % (count + 1)
            };
            if next == 0
                || next == current
                || (self.station_allowed(next - 1, guns_only)
                    && self.carries(next - 1, unlimited_ammo))
            {
                break;
            }
        }
        self.set_selection(next);
    }
    /// Move to station `next - 1`, or to NAV for 0, dropping any release in
    /// progress and the mounted seeker.
    fn set_selection(&mut self, next: usize) {
        self.release();
        self.bore_observation = None;
        self.mounted = Seeker::default();
        self.mounted_key = None;
        self.launch_mode = LaunchMode::Cued;
        self.armed = next != 0;
        if self.armed {
            self.selected = next - 1;
            if let Some(group) = &mut self.gunship {
                group.solo(self.cockpit.selected);
            }
        }
    }
    /// The mission's starting load is what `ammo` holds now: only those
    /// stations count as loaded.
    pub fn start_load(&mut self) {
        self.ever_loaded = self.ammo.iter().map(|a| a & 0x7fff != 0).collect();
    }
    /// Remember which stations hold something now (every tick, so a station
    /// that later runs dry is known to have been loaded).
    pub fn note_loaded(&mut self) {
        for (loaded, ammo) in self.ever_loaded.iter_mut().zip(&self.ammo) {
            *loaded |= ammo & 0x7fff != 0;
        }
    }
    /// Whether an ordinary station was loaded at mission start. A gun pod
    /// remains present when empty, until explicit jettison clears this flag.
    pub fn was_loaded(&self, station: usize) -> bool {
        self.ever_loaded.get(station).copied().unwrap_or(false)
    }
    /// When the selected station has run dry, move to the next station that
    /// carries something (gun then missiles in ring order), or to NAV when
    /// nothing is left. Does nothing while the selection still carries.
    pub fn advance_from_empty(&mut self, guns_only: bool, unlimited_ammo: bool) {
        if !self.armed || self.carries(self.selected, unlimited_ammo) {
            return;
        }
        if self.gunship.as_ref().is_some_and(|group| {
            group.slot(self.selected).is_some()
                && group
                    .fire_stations()
                    .into_iter()
                    .any(|index| self.carries(index, unlimited_ammo))
        }) {
            return;
        }
        let count = self.ammo.len();
        let next = (1..count)
            .map(|step| (self.selected + step) % count)
            .find(|i| self.station_allowed(*i, guns_only) && self.carries(*i, unlimited_ammo));
        self.set_selection(next.map_or(0, |i| i + 1));
    }
    /// Whether the guns only cheat lets this station be selected.
    pub fn station_allowed(&self, station: usize, guns_only: bool) -> bool {
        self.config
            .stations
            .get(station)
            .is_some_and(|s| !guns_only || is_gun(&s.weapon))
    }
    /// A station the selection ring may stop on: one that carries something
    /// (an empty station is not on the aircraft), or a loaded one that has
    /// run dry under unlimited ammunition.
    ///
    /// Unlimited ammunition only keeps a station that was loaded: a retained
    /// selection station that never held anything cannot fire, so it is never
    /// a stop however the cheat is set.
    pub fn carries(&self, station: usize, unlimited_ammo: bool) -> bool {
        self.ammo
            .get(station)
            .is_some_and(|ammo| ammo & 0x7fff != 0 || (unlimited_ammo && self.was_loaded(station)))
    }
    /// A station that holds something now, or held something this mission
    /// (it ran dry): it keeps its place in the selection ring. A retained
    /// selection station that was never loaded does not.
    fn on_aircraft(&self, station: usize) -> bool {
        self.ammo
            .get(station)
            .is_some_and(|ammo| ammo & 0x7fff != 0 || self.was_loaded(station))
    }
    /// Guns only turned on with a missile selected moves to the gun, or to
    /// NAV when the aircraft has none.
    fn enforce_guns_only(&mut self, guns_only: bool) {
        if !self.armed || self.station_allowed(self.selected, guns_only) {
            return;
        }
        self.release();
        self.bore_observation = None;
        self.mounted = Seeker::default();
        self.mounted_key = None;
        self.launch_mode = LaunchMode::Cued;
        match (0..self.ammo.len()).find(|i| self.station_allowed(*i, guns_only)) {
            Some(gun) => self.selected = gun,
            None => self.armed = false,
        }
    }
    pub fn select_next(&mut self) {
        if self.ammo.is_empty() {
            self.armed = false;
            return;
        }
        self.release();
        self.bore_observation = None;
        self.mounted = Seeker::default();
        self.mounted_key = None;
        // Skip a retained selection station that never held anything; with
        // nothing else to stop on, stay where we are.
        let count = self.ammo.len();
        self.selected = (1..=count)
            .map(|step| (self.selected + step) % count)
            .find(|i| self.on_aircraft(*i))
            .unwrap_or(self.selected);
        if let Some(group) = &mut self.gunship {
            group.solo(self.cockpit.selected);
        }
        if missiles::Profile::for_weapon(&self.config.stations[self.selected].weapon)
            .is_none_or(|p| !p.supports_boresight())
        {
            self.launch_mode = LaunchMode::Cued;
        }
    }
    fn designate_next(&mut self, forward: bool) {
        let friendlies = &self.friendlies;
        self.sensors.cycle(forward, |id| friendlies.contains(&id));
        self.selection_changed();
    }
    fn designate_visual(&mut self, launcher: Launcher) {
        let friendlies = &self.friendlies;
        self.sensors
            .select_visual(launcher.position, launcher.basis, |id| {
                friendlies.contains(&id)
            });
        self.selection_changed();
    }
    fn selection_changed(&mut self) {
        self.hud_selection = self.designated();
        if self.designated().is_some() {
            self.launch_mode = LaunchMode::Cued;
        }
        self.sight_follows_designation();
    }
    /// On the AC-130 a radar or visual designation (T, Enter, a scope click)
    /// becomes the gunsight's track, which then outlives the radar contact.
    fn sight_follows_designation(&mut self) {
        if let (Some(group), Some(id)) = (&mut self.gunship, self.sensors.selected()) {
            group.track(id);
        }
    }
    /// The AC-130 behaves as if Easy targeting were always on: its sight is
    /// an aircraft capability, not a cheat (John, 2026-10-09).
    fn easy_targeting(&self, cheat: bool) -> bool {
        cheat || self.gunship.is_some()
    }
}
/// One ownship read against the shared state: what its cockpit shows and
/// plays. A view only reads; commands and steps go through [`State`].
#[derive(Clone, Copy)]
pub struct OwnshipView<'a> {
    state: &'a State,
    own: &'a Ownship,
}
impl<'a> OwnshipView<'a> {
    pub fn ownship(&self) -> &'a Ownship {
        self.own
    }
    pub fn designated(&self) -> Option<u32> {
        self.own.designated()
    }
    /// A target row by its id: a target of the state, or another ownship's
    /// aircraft. An ownship is never a contact of itself.
    pub fn contact(&self, id: u32) -> Option<&'a Target> {
        let state = self.state;
        state.targets.iter().find(|t| t.id == id).or_else(|| {
            state
                .ownship_rows
                .iter()
                .find(|t| t.id == id && t.id != self.own.aircraft)
        })
    }
    pub fn guidance_available(&self, launcher: Launcher) -> bool {
        self.own.guidance_available(launcher)
    }
    /// The target the HUD square and target camera follow: the selection, or
    /// with Easy targeting the last selection after the sensors lose it.
    pub fn display_target(&self) -> Option<&'a Target> {
        // The AC-130's sight track, air or ground, at any range.
        if let Some(group) = &self.own.gunship {
            return self
                .contact(group.target()?)
                .filter(|target| target.body_present());
        }
        let id = if self.state.cheats.easy_targeting {
            self.own.designated().or(self.own.hud_selection)
        } else {
            self.own.designated()
        }?;
        self.contact(id).filter(|target| target.body_present())
    }
    /// The target the flight views follow: the display target, or without
    /// Easy targeting a dropped selection the pilot can still see.
    pub fn view_target(&self) -> Option<&'a Target> {
        self.display_target().or_else(|| {
            let id = self.own.sight_hold?;
            self.contact(id).filter(|target| target.body_present())
        })
    }
    /// The view for one launcher, which remembers what it works out.
    pub fn at(&self, launcher: Launcher) -> LauncherView<'a> {
        LauncherView {
            view: *self,
            launcher,
            ground: None,
            observation: Default::default(),
            solution: Default::default(),
            terrain_solution: Default::default(),
            readiness: Default::default(),
            solution_readiness: Default::default(),
        }
    }
    pub fn readiness(&self, launcher: Launcher) -> Readiness {
        self.at(launcher).readiness()
    }

    pub fn can_lock(&self, launcher: Launcher) -> bool {
        self.at(launcher).can_lock()
    }

    pub fn seeker_tone(&self, launcher: Launcher) -> Option<SeekerTone> {
        self.at(launcher).seeker_tone()
    }

    pub fn weapon_observation(&self, launcher: Launcher) -> Option<seeker::Observation> {
        self.at(launcher).weapon_observation()
    }

    pub fn mounted_solution(&self, launcher: Launcher) -> Option<missiles::Solution> {
        self.at(launcher).mounted_solution()
    }

    pub fn estimated_max_range(&self, launcher: Launcher) -> Option<f64> {
        self.at(launcher).estimated_max_range()
    }

    pub fn favorable_firing_band(&self, launcher: Launcher) -> Option<missiles::FiringBand> {
        self.at(launcher).favorable_firing_band()
    }

    pub fn in_estimated_range(&self, launcher: Launcher) -> bool {
        self.at(launcher).in_estimated_range()
    }

    pub fn estimated_hit_percent(&self, launcher: Launcher) -> u8 {
        self.at(launcher).estimated_hit_percent()
    }
    /// Any current observation of this object, on the selected scope channel
    /// or visually. Channels are never collapsed into one another.
    pub fn detects(&self, target: &Target) -> bool {
        self.own.sensors.observation(target.id).is_some()
    }
}
/// One ownship read for one launcher, so what the cockpit shows of the selected
/// weapon is worked out once: the observation, the firing solution and the
/// readiness are each computed on first use and kept. [`OwnshipView`] answers
/// each question by making one of these for the launcher it is given.
pub struct LauncherView<'a> {
    view: OwnshipView<'a>,
    launcher: Launcher,
    ground: Option<&'a dyn Fn(f64, f64) -> f64>,
    observation: std::cell::OnceCell<Option<seeker::Observation>>,
    solution: std::cell::OnceCell<Option<missiles::Solution>>,
    terrain_solution: std::cell::OnceCell<Option<missiles::Solution>>,
    readiness: std::cell::OnceCell<Readiness>,
    solution_readiness: std::cell::OnceCell<Readiness>,
}
impl<'a> LauncherView<'a> {
    /// Recheck surface launch visibility at this position, including between
    /// sensor updates. No terrain enters the energy-only range envelope.
    pub fn with_terrain(mut self, ground: &'a dyn Fn(f64, f64) -> f64) -> Self {
        self.ground = Some(ground);
        self.terrain_solution = Default::default();
        self.readiness = Default::default();
        self.solution_readiness = Default::default();
        self
    }

    fn terrain_masked(&self, point: Vector) -> bool {
        self.ground.is_some_and(|ground| {
            !missiles::launch_line_clear(self.launcher.position, point, ground)
        })
    }

    /// Whether [`Self::terrain_solution`] flies the terrain-aware fly-out
    /// (a cruise profile under the specified rules, with terrain to read)
    /// rather than answering with [`Self::mounted_solution`]. That fly-out
    /// samples the terrain on every step, so a caller that asks every frame
    /// keeps its answer for a few ticks (review M1).
    pub fn flies_terrain_solution(&self) -> bool {
        self.ground.is_some()
            && self.view.state.weapon_rules == Rules::Spec
            && self
                .view
                .own
                .config
                .stations
                .get(self.view.own.selected)
                .is_some_and(|s| missiles::Cruise::for_weapon(&s.weapon).is_some())
    }

    /// HUD fly-out time follows the same cruise clearance as live guidance.
    /// Launch authority and IN RNG continue to use the energy-only solution.
    /// Worked out once per view, like the other solutions.
    pub fn terrain_solution(&self) -> Option<missiles::Solution> {
        *self
            .terrain_solution
            .get_or_init(|| self.compute_terrain_solution())
    }

    fn compute_terrain_solution(&self) -> Option<missiles::Solution> {
        if self.view.state.weapon_rules == Rules::Energy || !self.flies_terrain_solution() {
            return self.mounted_solution();
        }
        let w = &self
            .view
            .own
            .config
            .stations
            .get(self.view.own.selected)?
            .weapon;
        let profile = missiles::Profile::for_weapon(w)?;
        let observed = self.weapon_observation()?;
        missiles::intercept_with_terrain(
            &w.movement,
            Motion::launch(w, self.launcher.velocity, self.launcher.position[1]),
            self.launcher.position,
            self.launcher.basis.forward,
            observed.position,
            observed.velocity,
            0,
            profile.guidance_ticks,
            self.ground,
        )
    }

    fn compute_readiness(&self) -> Readiness {
        let launcher = self.launcher;
        if !launcher.alive || self.view.own.hp <= 0 {
            return Readiness::LauncherLost;
        }
        if !self.view.own.armed {
            return Readiness::Safe;
        }
        if let Some(readiness) = self.view.own.gun_group_readiness() {
            return if self.view.state.projectiles.len() >= MAX_PROJECTILES {
                Readiness::Capacity
            } else {
                readiness
            };
        }
        if self.view.own.ammo[self.view.own.selected] & 0x8000 != 0 {
            return Readiness::StationFailed;
        }
        if self.view.own.rounds(self.view.own.selected) == 0 {
            return Readiness::Empty;
        }
        if self.view.state.projectiles.len() >= MAX_PROJECTILES {
            return Readiness::Capacity;
        }
        let solution = self.launch_solution();
        // A closed bay only delays a shot: the trigger opens it, so the
        // closed bay shows while a release waits on the doors.
        if solution
            .player_missile_may_fire(&self.view.own.config.stations[self.view.own.selected].weapon)
            && self.view.own.bay_waits(launcher)
            && self.view.own.bay_release.is_some()
        {
            return Readiness::BayClosed;
        }
        solution
    }

    pub fn readiness(&self) -> Readiness {
        *self.readiness.get_or_init(|| self.compute_readiness())
    }

    fn compute_launch_solution(&self) -> Readiness {
        if self.view.own.config.stations.is_empty() {
            return Readiness::Safe;
        }
        let launcher = self.launcher;
        let w = &self.view.own.config.stations[self.view.own.selected].weapon;
        // The guided air-to-ground weapons (docs/spec/air-to-ground.md,
        // "Warnings"): only a JDAM with nothing to fly to and an AT-2 with no
        // target are held back. A laser weapon goes with or without a spot
        // (it falls blind, and may find one); NO LASER, MASK and OUT OF
        // REACH are advisories in `Ownship::guided_sight`.
        if self.view.state.weapon_rules.uses_seeker()
            && let Some(kind) = super::guided::kind(w)
        {
            let designated = self
                .view
                .designated()
                .and_then(|id| self.view.contact(id))
                .filter(|t| !t.airborne);
            return match kind {
                Guidance::Coordinate
                    if designated.is_none_or(|t| t.hp <= 0)
                        && self
                            .view
                            .own
                            .sight
                            .is_none_or(|sight| sight.held_point().is_none()) =>
                {
                    Readiness::NoTarget
                }
                Guidance::Command => match designated {
                    None => Readiness::NoTarget,
                    Some(t) if t.hp <= 0 => Readiness::TargetDestroyed,
                    Some(_) => Readiness::Ready,
                },
                _ => Readiness::Ready,
            };
        }
        if w.seeker.signature == 0 {
            return Readiness::Ready;
        }
        let profile = (self.view.state.weapon_rules.uses_seeker())
            .then(|| missiles::Profile::for_weapon(w))
            .flatten();
        if profile.is_some_and(|p| !p.guidance_available(launcher.radar_power)) {
            return Readiness::Ready;
        }
        if profile.is_some_and(|p| p.supports_boresight())
            && self.view.own.launch_mode == LaunchMode::Boresight
        {
            // An air-to-ground missile in BORE fires only at the candidate its
            // seeker holds (agent): retail releases a HARM only at an emitting
            // target and a Maverick only at a designated one.
            let surface = profile.is_some_and(|p| p.role == TargetRole::Surface);
            let bore = self.view.own.bore_observation;
            if surface && bore.is_none() {
                return if profile.is_some_and(|p| p.holds_on_angles()) {
                    Readiness::NoEmitter
                } else {
                    Readiness::NoTarget
                };
            }
            if surface && bore.is_some_and(|o| self.terrain_masked(o.position)) {
                return Readiness::TerrainMask;
            }
            if surface
                && (self.view.own.mounted.target != bore.map(|o| o.id)
                    || self.view.own.mounted.status != Status::Locked)
            {
                return Readiness::RadarAcquiring;
            }
            return Readiness::Ready;
        }
        let Some(t) = self.view.designated().and_then(|id| self.view.contact(id)) else {
            return Readiness::NoTarget;
        };
        if profile.is_some_and(|p| !p.accepts(t)) {
            return Readiness::WrongTarget;
        }
        if !t.body_present() {
            return Readiness::TargetDestroyed;
        }
        if profile.is_some_and(|p| p.role == TargetRole::Surface) && self.terrain_masked(t.position)
        {
            return Readiness::TerrainMask;
        }
        if w.seeker.signature == 3 && profile.is_some_and(|p| p.needs_air_ground()) {
            // A surface radar weapon is released on the A-G track (retail).
            if self.view.own.radar_failed {
                return Readiness::RadarFailed;
            }
            if !self.view.own.ground_track(t.id) {
                return Readiness::TrackLost;
            }
        } else if w.seeker.signature == 3 {
            // Equipment state answers immediately, before the shared support
            // result, so a failure reported between steps is not stale.
            if self.view.own.radar_failed {
                return Readiness::RadarFailed;
            }
            if !launcher.radar {
                return Readiness::RadarOff;
            }
            // One shared support answer for this specific target. The weapon
            // keeps its own envelope test below.
            match self.view.own.sensors.support(t.id) {
                Support::Tracked => {}
                Support::RadarFailed => return Readiness::RadarFailed,
                Support::RadarOff => return Readiness::RadarOff,
                Support::Unavailable => return Readiness::NoRadar,
                Support::SearchOnly => return Readiness::RadarSearchOnly,
                Support::Acquiring => return Readiness::RadarAcquiring,
                Support::TrackCoverage => return Readiness::RadarCoverage,
                Support::NotSelected | Support::NoObservation => return Readiness::NoTarget,
            }
        }
        if let Some(profile) = profile {
            if !missiles::geometry(
                &missiles::launch_geometry(w),
                launcher.position,
                launcher.basis,
                t.position,
                None,
            ) {
                let old = zone_readiness(
                    &missiles::launch_geometry(w),
                    launcher.position,
                    launcher.basis.forward,
                    t.position,
                );
                return if old == Readiness::Ready {
                    Readiness::FieldOfView
                } else {
                    old
                };
            }
            if matches!(profile.guidance, Guidance::Infrared | Guidance::Emitter)
                && (self.view.own.mounted.target != Some(t.id)
                    || self.view.own.mounted.status != Status::Locked)
            {
                return Readiness::RadarAcquiring;
            }
            if self.mounted_solution().is_none()
                && missiles::length(sub(t.position, launcher.position))
                    >= f64::from(w.seeker.zones[1].minimum_range)
            {
                return Readiness::MaximumRange;
            }
            return Readiness::Ready;
        }
        zone_readiness(
            &tore_formats::weapons::Zone {
                minimum_range: 0,
                ..w.seeker.zones[1]
            },
            launcher.position,
            launcher.basis.forward,
            t.position,
        )
    }

    fn launch_solution(&self) -> Readiness {
        *self
            .solution_readiness
            .get_or_init(|| self.compute_launch_solution())
    }

    pub fn can_lock(&self) -> bool {
        if self.ground.is_some() && self.launch_solution() == Readiness::TerrainMask {
            return false;
        }
        if self.view.own.config.stations.is_empty() {
            return false;
        }
        let launcher = self.launcher;
        if self.view.state.weapon_rules.uses_seeker() && !self.view.own.guidance_available(launcher)
        {
            return false;
        }
        if self.view.state.weapon_rules.uses_seeker()
            && missiles::Profile::for_weapon(
                &self.view.own.config.stations[self.view.own.selected].weapon,
            )
            .is_some_and(|p| p.independent())
        {
            return self.view.own.mounted.target == self.view.own.designated()
                && self.view.own.mounted.target.is_some()
                && matches!(
                    self.view.own.mounted.status,
                    Status::Locked | Status::Pitbull
                );
        }
        self.view.own.config.stations[self.view.own.selected]
            .weapon
            .seeker
            .signature
            != 0
            && self.launch_solution() == Readiness::Ready
    }

    /// The seeker tone plays only while the seeker is actively tracking:
    /// silence with nothing in it (John, 2026-09-23).
    pub fn seeker_tone(&self) -> Option<SeekerTone> {
        let launcher = self.launcher;
        let w = &self
            .view
            .own
            .config
            .stations
            .get(self.view.own.selected)?
            .weapon;
        let profile = missiles::Profile::for_weapon(w)?;
        if !self.view.state.weapon_rules.uses_seeker()
            || !self.view.own.guidance_available(launcher)
            || !self.view.own.armed
            || !launcher.alive
            || self.view.own.hp <= 0
            || self.view.own.rounds(self.view.own.selected) == 0
            || self.view.own.ammo[self.view.own.selected] & 0x8000 != 0
            || profile.guidance == Guidance::Emitter
        {
            return None;
        }
        let radar = matches!(profile.guidance, Guidance::Active | Guidance::Supported);
        // A radar missile goes quiet with its target inside minimum range.
        let minimum = f64::from(w.seeker.zones[1].minimum_range.max(0));
        let too_close = |o: seeker::Observation| {
            radar && missiles::length(sub(o.position, launcher.position)) < minimum
        };
        // A radar missile in boresight sounds its lock tone on the bore return,
        // with or without a designated target (John, 2026-09-23).
        if radar && self.view.own.launch_mode == LaunchMode::Boresight {
            let o = self.view.own.bore_observation.filter(|o| !too_close(*o))?;
            return Some(SeekerTone {
                strength: 0.4 + 0.6 * o.quality.clamp(0., 1.),
                ground: false,
                radar,
                locked: true,
            });
        }
        let bore_ir = if self.view.own.launch_mode == LaunchMode::Boresight {
            // Tone follows the actual seeker return, not the HUD fly-out
            // estimate. Energy prediction cannot silence a visible IR target.
            let observed = self.view.own.bore_observation?;
            let tracked = self.view.own.mounted.observation?;
            if tracked.id != observed.id {
                return None;
            }
            Some(observed)
        } else {
            None
        };
        // Otherwise sound only while the mounted seeker is tracking.
        if self.view.own.mounted.observation.is_none_or(too_close) {
            return None;
        }
        let locked = matches!(
            self.view.own.mounted.status,
            Status::Locked | Status::Pitbull
        ) && bore_ir.is_none_or(|o| self.view.own.mounted.target == Some(o.id));
        Some(SeekerTone {
            strength: if radar {
                self.view.own.mounted.tone()
            } else {
                SeekerTone::ir_strength(self.estimated_hit_percent(), locked)
            },
            ground: !radar && w.flags & 0x10000 == 0,
            radar,
            locked,
        })
    }

    /// Current observation used by the display, separate from launch authority.
    fn compute_weapon_observation(&self) -> Option<seeker::Observation> {
        weapon_observation(
            self.view.own,
            self.launcher,
            self.view.state.weapon_rules,
            |id| self.view.contact(id),
        )
    }

    pub fn weapon_observation(&self) -> Option<seeker::Observation> {
        *self
            .observation
            .get_or_init(|| self.compute_weapon_observation())
    }

    /// The exact physical forecast used by the player HUD: a miss is an
    /// available answer, distinct from a pending forecast or absent track.
    pub fn energy_prediction(&self) -> Option<missiles::energy_estimate::Answer> {
        let w = &self
            .view
            .own
            .config
            .stations
            .get(self.view.own.selected)?
            .weapon;
        self.view.state.player_energy_answer(
            self.view.own.aircraft,
            self.view.own.selected,
            w,
            self.launcher,
            self.weapon_observation()?,
        )
    }

    fn compute_mounted_solution(&self) -> Option<missiles::Solution> {
        let launcher = self.launcher;
        let w = &self
            .view
            .own
            .config
            .stations
            .get(self.view.own.selected)?
            .weapon;
        let profile = missiles::Profile::for_weapon(w)?;
        let observed = self.weapon_observation()?;
        if self.view.state.weapon_rules == Rules::Energy {
            let answer = self.energy_prediction()?;
            return answer.hit.then(|| missiles::Solution {
                seconds: answer.seconds,
                point: std::array::from_fn(|i| {
                    observed.position[i] + observed.velocity[i] * answer.seconds
                }),
            });
        }
        // Repeated readers of exactly this launcher and observation share the
        // fly-out. An interpolated frame launcher remains a separate key.
        missiles::intercept_kept(
            &w.movement,
            Motion::launch(w, launcher.velocity, launcher.position[1]),
            launcher.position,
            launcher.basis.forward,
            observed.position,
            observed.velocity,
            0,
            profile.guidance_ticks,
        )
    }

    pub fn mounted_solution(&self) -> Option<missiles::Solution> {
        *self
            .solution
            .get_or_init(|| self.compute_mounted_solution())
    }

    pub fn estimated_min_range(&self) -> Option<f64> {
        let observed = self.weapon_observation()?;
        self.view
            .own
            .range_estimate
            .filter(|e| {
                e.station == self.view.own.selected
                    && e.target == observed.id
                    && e.mode == self.view.own.launch_mode
            })
            .map(|e| e.minimum)
    }
    pub fn estimated_max_range(&self) -> Option<f64> {
        let observed = self.weapon_observation()?;
        self.view
            .own
            .range_estimate
            .filter(|e| {
                e.station == self.view.own.selected
                    && e.target == observed.id
                    && e.mode == self.view.own.launch_mode
            })
            .map(|e| e.maximum)
    }

    pub fn favorable_firing_band(&self) -> Option<missiles::FiringBand> {
        let observed = self.weapon_observation()?;
        self.view
            .own
            .range_estimate
            .filter(|e| {
                e.station == self.view.own.selected
                    && e.target == observed.id
                    && e.mode == self.view.own.launch_mode
            })
            .and_then(|e| e.favorable)
    }

    /// Physical range validity is independent of rounded probability text.
    pub fn in_estimated_range(&self) -> bool {
        let Some(o) = self.weapon_observation() else {
            return false;
        };
        let min = f64::from(
            self.view.own.config.stations[self.view.own.selected]
                .weapon
                .seeker
                .zones[1]
                .minimum_range,
        );
        let surface = missiles::Profile::for_weapon(
            &self.view.own.config.stations[self.view.own.selected].weapon,
        )
        .is_some_and(|p| p.role == TargetRole::Surface);
        // Terrain is not range (CL-T): a surface weapon keeps IN RNG behind
        // a ridge, but every other readiness still turns it off (review L2).
        let ready = match self.readiness() {
            Readiness::Ready => true,
            Readiness::TerrainMask => surface,
            _ => false,
        };
        if self.view.state.weapon_rules == Rules::Energy {
            return ready
                && self
                    .estimated_min_range()
                    .zip(self.estimated_max_range())
                    .is_some_and(|(inner, outer)| {
                        outer > inner && (inner.max(min)..=outer).contains(&o.range)
                    })
                && self.mounted_solution().is_some();
        }
        ready
            && self
                .estimated_max_range()
                .is_some_and(|max| max > min && (min..=max).contains(&o.range))
            && self.mounted_solution().is_some()
    }

    pub fn estimated_hit_percent(&self) -> u8 {
        let Some(observation) = self.weapon_observation() else {
            return 0;
        };
        let w = &self.view.own.config.stations[self.view.own.selected].weapon;
        let Some(profile) = missiles::Profile::for_weapon(w) else {
            return 0;
        };
        let mut zone = w.seeker.zones[1];
        zone.maximum_range = self.estimated_max_range().unwrap_or(0.).floor() as _;
        if self.view.state.weapon_rules == Rules::Energy {
            zone.minimum_range = self
                .estimated_min_range()
                .unwrap_or(f64::from(zone.minimum_range))
                .ceil() as _;
        }
        missiles::estimated_hit_percent(
            observation,
            self.mounted_solution(),
            &zone,
            if self.view.state.weapon_rules == Rules::Energy {
                f64::from(missiles::energy_profile::body(w).life) / 120.
            } else {
                profile
                    .guidance_ticks
                    .min(u64::from(w.movement.remove_t) * 30) as f64
                    / 120.
            },
            (self.view.own.launch_mode == LaunchMode::Boresight).then(|| profile.search_cap()),
        )
    }
}

/// The observation's read-only inputs deliberately exclude projectiles and
/// release readiness, so independent ownships can evaluate it on workers.
fn weapon_observation<'a>(
    own: &Ownship,
    launcher: Launcher,
    weapon_rules: Rules,
    contact: impl Fn(u32) -> Option<&'a Target>,
) -> Option<seeker::Observation> {
    if !own.armed || (weapon_rules.uses_seeker() && !own.guidance_available(launcher)) {
        return None;
    }
    if own.launch_mode == LaunchMode::Boresight {
        let w = &own.config.stations[own.selected].weapon;
        let profile = missiles::Profile::for_weapon(w)?;
        if weapon_rules == Rules::Energy {
            return own.bore_observation.filter(|o| {
                profile.guidance != Guidance::Infrared || own.energy_hit == Some(o.id)
            });
        }
        return own.bore_observation.filter(|o| {
            profile.guidance != Guidance::Infrared
                || (missiles::geometry(
                    &missiles::launch_geometry(w),
                    launcher.position,
                    launcher.basis,
                    o.position,
                    None,
                ) && missiles::intercept_kept(
                    &w.movement,
                    Motion::launch(w, launcher.velocity, launcher.position[1]),
                    launcher.position,
                    launcher.basis.forward,
                    o.position,
                    o.velocity,
                    0,
                    profile.guidance_ticks,
                )
                .is_some())
        });
    }
    own.mounted.observation.or_else(|| {
        let id = own.designated()?;
        let w = &own.config.stations[own.selected].weapon;
        if weapon_rules.uses_seeker()
            && missiles::Profile::for_weapon(w)
                .is_some_and(|p| contact(id).is_none_or(|t| !p.accepts(t)))
        {
            return None;
        }
        let contact = own.sensors.observation(id)?;
        let delta = missiles::sub(contact.position, launcher.position);
        Some(seeker::Observation {
            id: contact.id,
            position: contact.position,
            velocity: contact.velocity,
            quality: 1.,
            range: missiles::length(delta),
            off_axis: dot(unit(delta), launcher.basis.forward)
                .clamp(-1., 1.)
                .acos(),
        })
    })
}

impl Ownship {
    pub fn guidance_available(&self, launcher: Launcher) -> bool {
        self.config.stations.get(self.selected).is_some_and(|s| {
            missiles::Profile::for_weapon(&s.weapon)
                .is_none_or(|p| p.guidance_available(launcher.radar_power))
        })
    }
}
impl State {
    /// A state with no ownship: an AI-only scene, or a host that adds its
    /// human-flown aircraft with [`State::add_ownship`].
    pub fn without_ownships() -> Self {
        Self {
            weapon_rules: Rules::Spec,
            ownships: Vec::new(),
            rng: 0x46414a54,
            target_jammer: false,
            history: vec![],
            strikes: vec![],
            range_category: 0,
            next_target_id: 1,
            next_shot: 0,
            projectiles: vec![],
            ended_missile_threats: vec![],
            targets: vec![],
            ownship_rows: vec![],
            actor_support: BTreeMap::new(),
            ground_bounds: BTreeMap::new(),
            effects: vec![],
            marks: vec![],
            crashed: Default::default(),
            marks_made: 0,
            blast_rolls: Default::default(),
            sound_events: vec![],
            smoke: super::smoke::Smoke::default(),
            devices: Default::default(),
            energy_devices: Default::default(),
            energy_seed: 0,
            energy_predictions: Default::default(),
            energy_predictions_enabled: true,
            measure_prediction_cost: false,
            prediction_cpu_clock: None,
            prediction_cpu_cost_ns: None,
            prediction_cost_ns: 0,
            prediction_request_ns: 0,
            prediction_request_cpu_ns: 0,
            prediction_gate_samples: [0; 3],
            prediction_player_samples: [0; 3],
            device_log: Default::default(),
            decoy_log: Default::default(),
            ground_bursts: Vec::new(),
            debris: Vec::new(),
            ledger: Default::default(),
            tick: 0,
            service_remainder: 0,
            cheats: Default::default(),
            friendly_fire: FriendlyFire::default(),
            volumes: rewind::History::default(),
            rewinds: BTreeMap::new(),
            surface_rounds: BTreeMap::new(),
            next_surface_shot: SURFACE_PROJECTILE_ID_BASE,
            bombs: BTreeMap::new(),
            jettisoned: BTreeMap::new(),
            falls: Default::default(),
            guided: BTreeMap::new(),
            guided_impacts: BTreeMap::new(),
            kept: BTreeMap::new(),
            ai_cockpits: BTreeMap::new(),
            ai_laser_armed: BTreeSet::new(),
            ai_laser_on: BTreeSet::new(),
            ground_looks: BTreeMap::new(),
            ground_listing: BTreeMap::new(),
            airfield: Default::default(),
            sensor_weather: Default::default(),
            ground_ceiling: None,
            parked: BTreeMap::new(),
        }
    }
    /// A state with no ownship whose aircraft rows count from 0: an open
    /// mission, where the AI flies plane 0 too and humans add their ownships
    /// by handoff.
    pub fn open_mission() -> Self {
        Self {
            next_target_id: 0,
            ..Self::without_ownships()
        }
    }
    /// A state with one ownship, on aircraft 0: single player's arrangement,
    /// and the one most tests use.
    pub fn new(config: Configuration, external: bool) -> Result<Self> {
        Self::for_ownship(0, DEFAULT_OWNSHIP_SIDE, config, external)
    }
    /// A state with one ownship, on `aircraft`.
    pub fn for_ownship(
        aircraft: u32,
        side: Side,
        config: Configuration,
        external: bool,
    ) -> Result<Self> {
        let mut state = Self::without_ownships();
        state.range_category = config.target_category;
        state.add_ownship(Ownship::new(aircraft, side, config, external)?)?;
        Ok(state)
    }
    /// Adds a human-flown aircraft, keeping the ownships in aircraft id
    /// order. An aircraft that already has an ownship is refused.
    pub fn add_ownship(&mut self, ownship: Ownship) -> Result<()> {
        match self
            .ownships
            .binary_search_by_key(&ownship.aircraft, |o| o.aircraft)
        {
            Ok(_) => Err(super::invalid("aircraft already has an ownship")),
            Err(index) => {
                self.ownships.insert(index, ownship);
                Ok(())
            }
        }
    }
    /// Takes an aircraft's ownship out, with its stores, damage and
    /// countermeasures, when its human gives it back. Its rounds still in the
    /// air keep flying: each takes its own copy of the weapon record its
    /// station held, as the AI's rounds carry theirs.
    pub fn remove_ownship(&mut self, aircraft: u32) -> Option<Ownship> {
        let index = self
            .ownships
            .binary_search_by_key(&aircraft, |o| o.aircraft)
            .ok()?;
        let ownship = self.ownships.remove(index);
        for projectile in &mut self.projectiles {
            if projectile.owner == aircraft && projectile.weapon.is_none() {
                projectile.weapon = ownship
                    .config
                    .stations
                    .get(projectile.station)
                    .map(|station| station.weapon.clone());
            }
        }
        Some(ownship)
    }
    /// The human-flown aircraft, in aircraft id order.
    pub fn ownships(&self) -> &[Ownship] {
        &self.ownships
    }
    pub fn ownship(&self, aircraft: u32) -> Option<&Ownship> {
        self.ownships
            .binary_search_by_key(&aircraft, |o| o.aircraft)
            .ok()
            .map(|index| &self.ownships[index])
    }
    pub fn ownship_mut(&mut self, aircraft: u32) -> Option<&mut Ownship> {
        self.ownships
            .binary_search_by_key(&aircraft, |o| o.aircraft)
            .ok()
            .map(|index| &mut self.ownships[index])
    }
    /// The ownship with the lowest aircraft id, for hosts and tests that
    /// serve one human-flown aircraft. Panics when there is none; a host with
    /// several asks for each by its aircraft id.
    pub fn own(&self) -> &Ownship {
        self.ownships.first().expect("an ownship")
    }
    pub fn own_mut(&mut self) -> &mut Ownship {
        self.ownships.first_mut().expect("an ownship")
    }
    /// The view of [`State::own`].
    pub fn own_view(&self) -> OwnshipView<'_> {
        view(self, self.own())
    }
    /// Damage to the first ownship, through the whole hit pipeline.
    #[cfg(test)]
    fn damage_own(&mut self, amount: i32, events: &mut Vec<Event>) {
        let aircraft = self.own().aircraft;
        self.with_ownship(aircraft, |state, own| {
            state.damage_ownship(own, amount, events)
        });
    }
    /// The view of one ownship that answers what its cockpit shows.
    pub fn view(&self, aircraft: u32) -> Option<OwnshipView<'_>> {
        self.ownship(aircraft)
            .map(|own| OwnshipView { state: self, own })
    }
    /// Runs `work` on the state with one ownship taken out of it, so the
    /// work can change both. `None` for an aircraft without an ownship.
    fn with_ownship<R>(
        &mut self,
        aircraft: u32,
        work: impl FnOnce(&mut Self, &mut Ownship) -> R,
    ) -> Option<R> {
        let index = self
            .ownships
            .binary_search_by_key(&aircraft, |o| o.aircraft)
            .ok()?;
        let mut ownship = self.ownships.remove(index);
        let result = work(self, &mut ownship);
        self.ownships.insert(index, ownship);
        Some(result)
    }
    /// The weapon a projectile carries: its own record, or a station of its
    /// owner's ownship. A round nobody owns uses the first ownship's stations.
    pub fn weapon<'a>(&'a self, projectile: &'a Projectile) -> &'a Weapon {
        projectile_weapon(&self.ownships, projectile)
    }
    /// Whether a projectile's motor is burning now: what its smoke and its
    /// flame follow (bug B7). Gun rounds and bombs have no motor.
    pub fn motor_powered(&self, projectile: &Projectile) -> bool {
        motor_powered(self.weapon(projectile), projectile, (self.tick / 30) as u16)
    }
    /// [`Self::motor_powered`] for a caller that has the projectile's weapon
    /// already ([`Self::weapon`]).
    pub fn motor_powered_with(&self, weapon: &Weapon, projectile: &Projectile) -> bool {
        motor_powered(weapon, projectile, (self.tick / 30) as u16)
    }
    /// Development-only visual fixture. Gameplay damage always arrives through impacts.
    pub fn preview_localized_damage(&mut self, section: DamageSection, fraction: f64) {
        let fraction = fraction.clamp(0., 1.);
        for own in &mut self.ownships {
            let capacity = own.config.damage_capacity;
            let amount = (f64::from(capacity) * fraction).round() as i32;
            own.localized_damage = LocalizedDamage::default();
            own.localized_damage.record(section, amount, capacity);
        }
        for target in &mut self.targets {
            let amount = (f64::from(target.initial_hp) * fraction).round() as i32;
            target.localized_damage = LocalizedDamage::default();
            target
                .localized_damage
                .record(section, amount, target.initial_hp);
        }
    }
    /// Sets the side of an aircraft: an ownship or an aircraft row. `false`
    /// for an id that is neither.
    pub fn set_side(&mut self, aircraft: u32, side: Side) -> bool {
        if let Some(own) = self.ownship_mut(aircraft) {
            own.side = side;
            true
        } else if let Some(row) = self.targets.iter_mut().find(|t| t.id == aircraft) {
            row.side = side;
            true
        } else {
            false
        }
    }
    /// The bay request of one ownship.
    pub fn bay_demand(&self, aircraft: u32) -> bool {
        self.ownship(aircraft)
            .is_some_and(|own| own.bay_demand(self.tick))
    }
    /// One ownship lets go of its trigger and drops its queued rounds.
    pub fn release(&mut self, aircraft: u32) {
        if let Some(own) = self.ownship_mut(aircraft) {
            own.release();
            own.gun_key = false;
        }
    }

    /// Replace all fire-control snapshots of aircraft that are not ownships for
    /// the next missile step. An ownship's support is sourced from its own
    /// sensor component.
    ///
    /// An entry answers for the target its observation names: an owner may
    /// give one per target it guides missiles at, and an entry without an
    /// observation supports nothing, so it is dropped. A battery launcher's
    /// entry names the launcher as owner and its radar's position as
    /// `radar_position` (docs/spec/surface-defenses.md, "SAM batteries").
    pub fn set_actor_supports(&mut self, supports: impl IntoIterator<Item = ActorSupport>) {
        self.actor_support.clear();
        self.actor_support
            .extend(supports.into_iter().filter_map(|support| {
                support
                    .observation
                    .map(|observation| ((support.owner, observation.id), support))
            }));
    }
    /// The fire-control answer `owner` gives for `target`.
    fn support_for(&self, owner: u32, target: Option<u32>) -> Option<ActorSupport> {
        target.and_then(|target| self.actor_support.get(&(owner, target)).copied())
    }

    /// The rewind, in ticks, that projectile `id` carries: 0 for every round
    /// but a gun round a human fired with a view.
    /// The path of bomb `id` while it falls: where it was released and how
    /// fast it was going, scatter included. `None` for any other round.
    pub fn bomb_motion(&self, id: u32) -> Option<super::ballistics::BombMotion> {
        self.bombs.get(&id).copied()
    }
    pub fn rewind_of(&self, id: u32) -> u16 {
        self.rewinds.get(&id).copied().unwrap_or(0)
    }
    /// The last second of every aircraft's hit volume.
    pub fn hit_volumes(&self) -> &rewind::History {
        &self.volumes
    }
    /// Explicit reproducible mission/study seed for episode-local decoy draws.
    pub fn set_energy_seed(&mut self, seed: u64) {
        self.energy_seed = seed;
    }
    pub fn energy_seed(&self) -> u64 {
        self.energy_seed
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Current permitted missile measurements for RWR and AI awareness. This
    /// exposes seeker state and actor-owned support, never hidden target poses.
    /// `launchers` gives each ownship's launcher for this tick.
    pub fn missile_snapshots(
        &self,
        launchers: &[(u32, Launcher)],
    ) -> Vec<super::threats::MissileSnapshot> {
        self.snapshots(&self.ownships, launchers)
    }
    /// Explicit lifecycle notices, valid until the next combat step.
    pub fn ended_missile_threats(&self) -> &[u32] {
        &self.ended_missile_threats
    }

    fn snapshots(
        &self,
        ships: &[Ownship],
        launchers: &[(u32, Launcher)],
    ) -> Vec<super::threats::MissileSnapshot> {
        self.projectiles
            .iter()
            .filter(|p| p.threatening())
            .filter_map(|projectile| {
                let weapon = projectile_weapon(ships, projectile);
                let flight = projectile.guidance.as_ref();
                // A gun has no profile (tested), so its round skips the table.
                let profile = flight.map(|flight| flight.profile).or_else(|| {
                    (!is_gun(weapon))
                        .then(|| missiles::Profile::for_weapon(weapon))
                        .flatten()
                })?;
                let target = flight
                    .and_then(|flight| flight.seeker.target)
                    .or(projectile.target);
                let support =
                    if let Some(own) = ships.iter().find(|o| o.aircraft == projectile.owner) {
                        let launcher = launchers
                            .iter()
                            .find(|(aircraft, _)| *aircraft == own.aircraft)
                            .map(|(_, launcher)| launcher);
                        target.zip(launcher).map(|(id, launcher)| ActorSupport {
                            supported: agm::ownship_supports(
                                own,
                                Some(profile),
                                weapon,
                                launcher,
                                id,
                                self.targets.iter().find(|t| t.id == id),
                            ),
                            owner: own.aircraft,
                            observation: own.sensors.observation(id).map(|contact| {
                                let delta = sub(contact.position, projectile.position);
                                seeker::Observation {
                                    id,
                                    position: contact.position,
                                    velocity: contact.velocity,
                                    quality: 1.,
                                    off_axis: dot(unit(delta), projectile.direction)
                                        .clamp(-1., 1.)
                                        .acos(),
                                    range: missiles::length(delta),
                                }
                            }),
                            radar_position: launcher.position,
                            radar_emitting: launcher.radar && launcher.alive,
                        })
                    } else {
                        self.support_for(projectile.owner, target)
                    };
                let supported = profile.guidance == Guidance::Supported
                    && support.is_some_and(|answer| {
                        answer.supported
                            && answer.radar_emitting
                            && answer.observation.is_some_and(|o| Some(o.id) == target)
                    });
                let velocity = projectile.motion.map_or_else(
                    || {
                        projectile
                            .direction
                            .map(|axis| axis * f64::from(projectile.speed_f8) / 256.)
                    },
                    |motion| motion.velocity,
                );
                Some(super::threats::MissileSnapshot {
                    id: projectile.id,
                    owner: projectile.owner,
                    position: projectile.position,
                    velocity,
                    guidance: profile.guidance,
                    target,
                    radar_active: flight.is_some_and(|flight| {
                        profile.guidance == Guidance::Active
                            && flight.enabled
                            && flight.seeker.status != Status::Expired
                    }),
                    radar_acquired: flight.is_some_and(|flight| {
                        flight.seeker.status == Status::Pitbull
                            && flight.seeker.observation.is_some()
                    }),
                    supported,
                    supporting_radar_position: supported
                        .then(|| support.map(|answer| answer.radar_position))
                        .flatten(),
                    alive: true,
                })
            })
            .collect()
    }

    /// One ownship's selection ring: NAV, then each configured weapon station.
    /// Legacy range commands retain their old station-only behavior for tapes.
    pub fn cycle_selection(&mut self, aircraft: u32, forward: bool) {
        let (guns_only, unlimited_ammo) = (self.cheats.guns_only, self.cheats.unlimited_ammo);
        if let Some(own) = self.ownship_mut(aircraft) {
            own.cycle_selection(forward, guns_only, unlimited_ammo);
        }
    }
    /// T (forward) and Shift-T cycle radar contacts; friendly aircraft and
    /// wrecks are skipped.
    pub fn designate_next(&mut self, aircraft: u32, forward: bool) {
        if let Some(own) = self.ownship_mut(aircraft) {
            own.designate_next(forward);
        }
    }
    /// Enter selects the visible sensor contact nearest the nose.
    pub fn designate_visual(&mut self, aircraft: u32, launcher: Launcher) {
        if let Some(own) = self.ownship_mut(aircraft) {
            own.designate_visual(launcher);
        }
    }
    /// The seat's gunsight controls for the coming steps: slew deflection
    /// (x right, y up, -127 to 127) and the zoom step (1 to 6, 0 for the
    /// default). Held until set again; nothing without an AC-130 gun group.
    pub fn set_sight_input(&mut self, aircraft: u32, deflection: [i8; 2], zoom: u8) {
        let Some(own) = self.ownship_mut(aircraft) else {
            return;
        };
        let input = super::gunship::SightInput { deflection, zoom };
        if let Some(group) = own.gunship.as_mut() {
            group.input = input;
        }
        if let Some(sight) = own.sight.as_mut() {
            sight.input = input;
        }
    }
    /// A newly selected bomb, unguided or guided, starts in LVL when a ground
    /// target is designated (for a JDAM the sight's held point, for a laser
    /// bomb a laser spot also count) and the flight path is near level, else
    /// in CCIP ([`super::delivery::DeliveryMode::chosen`]); Ctrl+M overrides
    /// it. A guided bomb's CCIP is its reach scale with a plain trigger.
    fn choose_delivery(&self, own: &mut Ownship, launcher: Launcher) {
        if !own.armed {
            return;
        }
        let Some(w) = own.config.stations.get(own.selected).map(|s| &s.weapon) else {
            return;
        };
        let glide = super::guided::is_glide_bomb(w);
        if !super::ballistics::is_bomb(w) && !glide {
            return;
        }
        let mut ground_target = own
            .designated()
            .and_then(|id| self.targets.iter().find(|t| t.id == id))
            .is_some_and(|t| t.role == TargetRole::Surface && t.hp > 0);
        if glide && !ground_target {
            ground_target = match super::guided::kind(w) {
                Some(Guidance::Coordinate) => {
                    guided::designated_point(own, Guidance::Coordinate, &self.targets).is_some()
                }
                _ => own.laser.spot(own.aircraft, own.side).is_some(),
            };
        }
        let v = launcher.velocity;
        own.delivery_mode = crate::models::strike_class::default_delivery(
            own.config.strike_class,
            ground_target,
            v[1].atan2(v[0].hypot(v[2])),
        );
    }
    /// A manual range or cockpit command for one ownship.
    pub fn command(&mut self, aircraft: u32, command: Command, launcher: Launcher) {
        self.with_ownship(aircraft, |state, own| {
            state.command_of(own, command, launcher)
        });
    }
    fn command_of(&mut self, own: &mut Ownship, command: Command, launcher: Launcher) {
        let before = own.designated();
        self.apply_command(own, command, launcher);
        // A new designation retargets or blinds the player's supported
        // surface missiles in flight (retail `PROJRetargetMissiles`).
        if let Some(id) = own.designated()
            && before != Some(id)
        {
            agm::retarget(self, own, id, &launcher);
        }
    }
    fn apply_command(&mut self, own: &mut Ownship, command: Command, launcher: Launcher) {
        match command {
            Command::ClearRange => {
                self.targets.retain(|t| {
                    self.ground_bounds.contains_key(&t.id) || self.parked.contains_key(&t.id)
                });
                own.sensors.clear_selection();
                own.hud_selection = None;
                own.sight_hold = None;
                own.bore_observation = None;
                own.mounted = Seeker::default();
            }
            Command::TargetDistance(distance) => {
                if (1..=1_000_000).contains(&distance) {
                    for target in self.targets.iter_mut().filter(|t| {
                        !self.ground_bounds.contains_key(&t.id) && !self.parked.contains_key(&t.id)
                    }) {
                        target.position = std::array::from_fn(|i| {
                            launcher.position[i] + launcher.basis.forward[i] * f64::from(distance)
                        });
                    }
                }
            }
            Command::CompatibilityWeapons => {
                self.weapon_rules = Rules::Compatibility;
                own.launch_mode = LaunchMode::Cued;
                own.bore_observation = None;
                own.mounted = Seeker::default();
            }
            Command::TargetHeat(value) => {
                for t in self.targets.iter_mut().filter(|t| {
                    !self.ground_bounds.contains_key(&t.id) && !self.parked.contains_key(&t.id)
                }) {
                    t.heat = match value {
                        0 => Heat::Unknown,
                        1 => Heat::Engine {
                            on: false,
                            throttle: 0.,
                            afterburner: false,
                        },
                        2 => Heat::Engine {
                            on: true,
                            throttle: 0.,
                            afterburner: false,
                        },
                        3 => Heat::Engine {
                            on: true,
                            throttle: 1.,
                            afterburner: false,
                        },
                        _ => Heat::Engine {
                            on: true,
                            throttle: 1.,
                            afterburner: true,
                        },
                    };
                }
            }
            Command::ToggleTargetRadar => {
                for t in self.targets.iter_mut().filter(|t| {
                    !self.ground_bounds.contains_key(&t.id) && !self.parked.contains_key(&t.id)
                }) {
                    t.radar_emitting = !t.radar_emitting;
                }
            }
            Command::Incoming => {
                if self.projectiles.len() < MAX_PROJECTILES
                    && own.hp > 0
                    && !own.config.stations.is_empty()
                {
                    let w = &own.config.stations[own.selected].weapon;
                    let position = std::array::from_fn(|i| {
                        launcher.position[i] + launcher.basis.forward[i] * 1800.
                    });
                    self.projectiles.push(Projectile {
                        id: self.next_shot,
                        owner: INCOMING_OWNER,
                        weapon: Some(w.clone()),
                        guidance: None,
                        motion: None,
                        guidance_ticks: None,
                        age: 0,
                        incoming: Some(own.aircraft),
                        station: own.selected,
                        position,
                        previous: position,
                        direction: launcher.basis.forward.map(|v| -v),
                        speed_f8: launch_speed(&w.movement, (launcher.speed_fps * 256.) as i32)
                            .expect("validated speed")
                            * 256,
                        launched_t: (self.tick / 30) as u16,
                        target: if w.seeker.signature != 0 {
                            Some(own.aircraft)
                        } else {
                            None
                        },
                        fall: FallState::default(),
                        gun_round: None,
                        tracer: false,
                    });
                }
            }
            Command::DamagePlayer => own.pending_damage = true,
            Command::ToggleTargetJammer => {
                self.target_jammer = !self.target_jammer;
                for t in self.targets.iter_mut().filter(|t| {
                    !self.ground_bounds.contains_key(&t.id) && !self.parked.contains_key(&t.id)
                }) {
                    t.jammer_active = self.target_jammer;
                }
            }
            Command::ReleaseChaff => self.release_countermeasure(own, EffectKind::Chaff, launcher),
            Command::ReleaseFlare => self.release_countermeasure(own, EffectKind::Flare, launcher),
            Command::ToggleSeekerMode => {
                // Ctrl+M with an unguided bomb selected switches its sight
                // between CCIP and LVL (docs/spec/air-to-ground.md).
                if own.armed
                    && own.config.stations.get(own.selected).is_some_and(|s| {
                        super::ballistics::is_bomb(&s.weapon)
                            || super::guided::is_glide_bomb(&s.weapon)
                    })
                {
                    own.delivery_mode = own.delivery_mode.toggled();
                    return;
                }
                if self.weapon_rules == Rules::Compatibility || !own.guidance_available(launcher) {
                    return;
                }
                if missiles::Profile::for_weapon(&own.config.stations[own.selected].weapon)
                    .is_none_or(|p| !p.supports_boresight())
                {
                    return;
                }
                // A designation holds an infrared missile, and an
                // anti-radiation surface missile, in CUED.
                if own.designated().is_some()
                    && missiles::Profile::for_weapon(&own.config.stations[own.selected].weapon)
                        .is_some_and(|p| p.guidance == Guidance::Infrared || p.holds_on_angles())
                {
                    return;
                }
                own.launch_mode = if own.launch_mode == LaunchMode::Cued {
                    LaunchMode::Boresight
                } else {
                    LaunchMode::Cued
                };
                own.bore_observation = None;
                own.mounted = Seeker::default();
                own.mounted_key = None;
                own.release();
            }
            Command::NextGunGroup => {
                let Some(group) = &own.gunship else {
                    return;
                };
                let slots: Vec<_> = group.stations.iter().flatten().copied().collect();
                if slots.is_empty() {
                    return;
                }
                let next = slots
                    .iter()
                    .position(|index| *index == own.selected)
                    .map_or(0, |i| (i + 1) % slots.len());
                let membership = group.included;
                own.set_selection(slots[next] + 1);
                own.gunship.as_mut().expect("existing gunship").included = membership;
            }
            Command::ToggleGunGroup => {
                if let Some(group) = &mut own.gunship {
                    group.toggle(own.cockpit.selected);
                    own.release();
                }
            }
            Command::NextWeapon => {
                own.select_next();
                self.choose_delivery(own, launcher);
            }
            Command::NextSelection => {
                own.cycle_selection(true, self.cheats.guns_only, self.cheats.unlimited_ammo);
                self.choose_delivery(own, launcher);
            }
            Command::PreviousSelection => {
                own.cycle_selection(false, self.cheats.guns_only, self.cheats.unlimited_ammo);
                self.choose_delivery(own, launcher);
            }
            Command::AdvanceFromEmpty => {
                own.advance_from_empty(self.cheats.guns_only, self.cheats.unlimited_ammo);
                self.choose_delivery(own, launcher);
            }
            Command::SelectNav => {
                own.release();
                own.armed = false;
                own.bore_observation = None;
                own.mounted = Seeker::default();
                own.mounted_key = None;
                own.launch_mode = LaunchMode::Cued;
            }
            Command::Designate => own.designate_next(true),
            Command::DesignatePrevious => own.designate_next(false),
            Command::DesignateVisual => own.designate_visual(launcher),
            Command::DesignateVisualTarget(id) => {
                if own.sensors.designate_visual_contact(id) {
                    own.selection_changed();
                }
            }
            Command::DesignateTarget(id) => {
                if own.sensors.designate(id) {
                    own.hud_selection = Some(id);
                }
                if own.designated().is_some() {
                    own.launch_mode = LaunchMode::Cued;
                }
                own.sight_follows_designation();
            }
            Command::ToggleAirGround => {
                own.sensors.toggle_air_ground();
            }
            Command::NearestGround => {
                if own.easy_targeting(self.cheats.easy_targeting)
                    && let Some(id) = self.nearest_ground(own, launcher.position)
                {
                    own.sensors.select_ground(id);
                    own.selection_changed();
                }
            }
            Command::ClearDesignation => {
                own.sensors.clear_selection();
                own.hud_selection = None;
                own.sight_hold = None;
                own.bore_observation = None;
                own.mounted = Seeker::default();
                own.mounted_key = None;
                own.release();
                if let Some(group) = &mut own.gunship {
                    group.drop_hold();
                    // Dropping the target lets the guns follow the sight.
                    group.gun_lock = None;
                }
                if let Some(sight) = &mut own.sight {
                    sight.drop_hold();
                }
            }
            Command::SightDesignate => own.request_sight(super::sight::SightRequest::Designate),
            Command::SightPinGround => own.request_sight(super::sight::SightRequest::Pin),
            Command::SightNext => own.request_sight(super::sight::SightRequest::Next),
            Command::GunKeyDown => own.gun_key = true,
            Command::GunKeyUp => own.gun_key = false,
            Command::ToggleLaser => {
                if own.config.sight_sensor().is_some_and(|s| s.laser) {
                    own.laser.toggle();
                }
            }
            Command::ToggleArm => {
                own.armed = !own.armed && !own.config.stations.is_empty();
                own.release();
            }
            Command::Jettison => self.jettison(own, launcher, false),
            Command::JettisonTanks => self.jettison(own, launcher, true),
            Command::ReplaceTarget => self.range_target_of(own, launcher),
            Command::CycleClass => {
                self.range_category = [own.config.target_category, 0x2000, 0x100, 0x400, 0x40]
                    [(damage_class(self.range_category) + 1) % 5];
                self.range_target_of(own, launcher);
            }
            // Native equipment damage marks the station's high bit. Selecting
            // the failure manually is a test fixture, not a recovered damage roll.
            Command::FailStation => {
                if let Some(ammo) = own.ammo.get_mut(own.cockpit.selected) {
                    *ammo |= 0x8000;
                }
                own.release();
            }
        }
    }
    /// One device per press, as the retail "Chaff launched, %d left" message
    /// reports. Each missile guiding on the ownship with the matching seeker
    /// class rolls the B47 decoy chance (docs/spec/countermeasures.md). An
    /// empty or damaged dispenser releases nothing.
    fn release_countermeasure(&mut self, own: &mut Ownship, kind: EffectKind, launcher: Launcher) {
        let (count, effectiveness, signature) = match kind {
            EffectKind::Chaff => (&mut own.chaff, own.config.ecm.chaff[1], 3),
            _ => (&mut own.flares, own.config.ecm.flare[1], 2),
        };
        if *count == 0 || !launcher.alive || own.hp <= 0 {
            return;
        }
        if !self.cheats.unlimited_ammo {
            *count -= 1;
        }
        let left = *count;
        self.note_release(
            super::countermeasures::Release {
                position: launcher.position,
                velocity: launcher.velocity,
                basis: launcher.basis,
            },
            kind,
            own.aircraft,
            Some(left),
        );
        // The device this release made, which every roll below is against.
        let device = self.devices.released();
        if self.weapon_rules == Rules::Energy {
            self.energy_devices.effectiveness(
                own.aircraft,
                if signature == 2 {
                    missiles::decoys::Kind::Flare
                } else {
                    missiles::decoys::Kind::Chaff
                },
                effectiveness,
            );
            return;
        }
        for projectile in &mut self.projectiles {
            // Lazy: an AI missile carries its own weapon and its station
            // indexes the AI's loadout, which can be longer than this
            // ownship's; another ownship's round uses that ownship's stations.
            let ships = &self.ownships;
            let weapon = projectile.weapon.as_ref().unwrap_or_else(|| {
                let config = if projectile.owner == own.aircraft {
                    &own.config
                } else {
                    owner_configuration(ships, projectile.owner)
                };
                &config.stations[projectile.station].weapon
            });
            let guiding = projectile.target == Some(own.aircraft)
                && weapon.seeker.signature == signature
                && projectile.guidance.as_ref().is_none_or(|flight| {
                    flight.enabled && flight.seeker.acquired && flight.seeker.observation.is_some()
                });
            if !guiding {
                continue;
            }
            let (susceptibility, effectiveness) = (
                weapon.seeker.chaff_flare_chance.min(100),
                effectiveness.min(100),
            );
            let threshold = crate::ai::threat::decoy_threshold(susceptibility, effectiveness);
            let roll = draw(&mut self.rng, 100);
            let decoyed = roll < u16::from(threshold);
            // Write-only: the roll as a recording tells it.
            if self.decoy_log.len() == MAX_RELEASE_RECORDS {
                self.decoy_log.pop_front();
            }
            self.decoy_log.push_back(DecoyRoll {
                aircraft: own.aircraft,
                projectile: projectile.id,
                kind,
                device,
                susceptibility,
                effectiveness,
                threshold: u16::from(threshold),
                bound: 100,
                roll,
                decoyed,
            });
            if decoyed {
                self.ledger.resolve(projectile.id, Resolution::Spoofed);
                projectile.target = None;
                projectile.guidance = None;
            }
        }
    }
    fn damage_ownship(&mut self, own: &mut Ownship, amount: i32, events: &mut Vec<Event>) {
        let applied = amount.max(0).min(own.hp);
        if applied == 0 {
            return;
        }
        own.hp -= applied;
        own.damage = own.damage.saturating_add(amount);
        events.push(Event::OwnshipDamaged {
            aircraft: own.aircraft,
            amount: applied,
        });
        // Normal damage takes hit points only; system faults are Realistic.
        if self.cheats.system_damage() {
            self.damage_systems(own, amount, events);
        }
        if own.hp == 0 {
            own.release();
            events.push(Event::OwnshipDestroyed {
                aircraft: own.aircraft,
            });
        }
    }
    /// Realistic damage: a hit may fault a subsystem, and accumulated
    /// damage brings on the faults the aircraft's thresholds call for.
    fn damage_systems(&mut self, own: &mut Ownship, amount: i32, events: &mut Vec<Event>) {
        for index in super::systems::hit_faults(
            &own.config.system_damage,
            &own.subsystem_counts,
            own.damage,
            own.config.damage_capacity,
            amount,
            own.config.afterburner_available,
            |n| draw(&mut self.rng, n),
        ) {
            own.subsystem_counts[index] += 1;
            own.last_subsystem = Some(index);
            events.push(Event::SubsystemDamaged {
                aircraft: own.aircraft,
                index,
            });
            let Some(h) = index.checked_sub(36) else {
                continue;
            };
            if let Some(Some(slot)) = own.config.hardpoint_slots.get(h) {
                if own.rounds(*slot) > 0 {
                    own.ammo[*slot] |= 0x8000;
                }
            } else if Some(h) == own.config.radar_hardpoint {
                own.radar_failed = true;
            } else if h == own.config.visual_hardpoint {
                own.visual_failed = true;
            } else if Some(h) == own.config.infrared_hardpoint {
                own.infrared_failed = true;
            } else if Some(h) == own.config.rwr_hardpoint {
                own.rwr_failed = true;
            } else if Some(h) == own.config.ecm_hardpoint {
                use super::systems::EcmLoss;
                match super::systems::ecm_loss(&own.config.ecm, |n| draw(&mut self.rng, n)) {
                    Some(EcmLoss::Everything) => {
                        own.ecm_failed = true;
                        own.chaff = 0;
                        own.flares = 0;
                    }
                    Some(EcmLoss::Chaff) => own.chaff = 0,
                    Some(EcmLoss::Flares) => own.flares = 0,
                    None => {}
                }
            }
        }
    }
    /// An ownship's subsystem lifecycle reached a fatal outcome outside a
    /// projectile hit.
    pub fn systems_destroyed(&mut self, aircraft: u32) -> Option<Event> {
        let own = self.ownship_mut(aircraft)?;
        if own.hp <= 0 {
            return None;
        }
        own.hp = 0;
        own.damage = own.damage.max(own.config.damage_capacity);
        own.release();
        Some(Event::OwnshipDestroyed { aircraft })
    }
    /// An ownship's aircraft blew up in the air.
    pub fn ownship_airburst(&mut self, aircraft: u32, position: Vector) -> Option<Event> {
        if !self.ownship_explosion(aircraft) {
            return None;
        }
        self.blast(
            position,
            EffectKind::Destroyed,
            super::blast::AIRCRAFT,
            super::blast_scale::FULL,
        );
        Some(Event::Airburst(aircraft))
    }
    /// An ownship's aircraft hit land or, with `water`, the sea.
    pub fn ownship_ground_impact(
        &mut self,
        aircraft: u32,
        position: Vector,
        water: bool,
    ) -> Option<Event> {
        if !self.ownship_explosion(aircraft) {
            return None;
        }
        self.aircraft_crashed(aircraft, position, water);
        Some(Event::OwnshipGroundImpact { aircraft })
    }
    fn ownship_explosion(&mut self, aircraft: u32) -> bool {
        let Some(own) = self.ownship_mut(aircraft) else {
            return false;
        };
        if own.explosion_reported {
            return false;
        }
        own.explosion_reported = true;
        self.debris.retain(|piece| piece.owner != aircraft);
        true
    }
    /// An aircraft reached the ground: a crash explosion, and on land a
    /// crater with a fire and a smoke column for 15 minutes. Once per
    /// aircraft; presentation only (docs/spec/explosions.md).
    pub fn aircraft_crashed(&mut self, id: u32, position: Vector, water: bool) {
        use super::blast;
        if !self.crashed.insert(id) {
            return;
        }
        if water {
            self.blast(
                position,
                EffectKind::Destroyed,
                blast::CRASH_WATER,
                super::blast_scale::FULL,
            );
            return;
        }
        self.blast(
            position,
            EffectKind::Destroyed,
            blast::CRASH_LAND,
            super::blast_scale::FULL,
        );
        self.crater(
            position,
            blast::CRASH_CRATER,
            super::blast_scale::FULL,
            blast::CRASH_TICKS,
        );
        self.light_fire(position, blast::CRASH_TICKS);
    }
    /// A fire and its smoke column at `position` for `ticks` (a crash
    /// site's 15 minutes, or [`blast::FOREVER`] for a destroyed building's).
    /// Fires of limited life are capped at [`blast::MAX_FIRES`]: the oldest
    /// goes out when a new one would pass it. A fire that burns for good is
    /// never evicted and never counted, so a bombed airfield keeps every
    /// fire without crowding out the crash sites.
    pub(crate) fn light_fire(&mut self, position: Vector, ticks: u32) {
        use super::blast::MarkKind;
        if ticks != super::blast::FOREVER
            && self.marks.iter().filter(|m| m.is_limited_fire()).count() >= super::blast::MAX_FIRES
            && let Some(oldest) = self.marks.iter().position(|m| m.is_limited_fire())
        {
            self.marks.remove(oldest);
        }
        self.mark(position, MarkKind::Fire, ticks, super::blast_scale::FULL);
    }
    fn mark(&mut self, position: Vector, kind: super::blast::MarkKind, ticks: u32, scale: u8) {
        self.marks.push(super::blast::Mark {
            position,
            kind,
            ticks,
            born: self.tick,
            serial: self.marks_made,
            scale,
        });
        self.marks_made += 1;
    }
    /// A crater (or, size [`super::blast::POCK`], a gun round's pock mark)
    /// drawn at `scale`. Every one stays for good: no cap (John,
    /// 2026-10-12).
    pub(crate) fn crater(&mut self, position: Vector, size: u8, scale: u8, ticks: u32) {
        self.mark(position, super::blast::MarkKind::Crater(size), ticks, scale);
    }
    /// An explicit, non-AI range target of the selected ported aircraft. No
    /// targets are inserted into ordinary free flight or fabricated on scopes.
    /// Straight-flight mission fixture. No steering, sensors transmitting or AI.
    pub fn add_dummy(
        &mut self,
        config: &Configuration,
        position: Vector,
        basis: Basis,
        side: Side,
    ) {
        let id = self.next_target_id;
        self.next_target_id = id.checked_add(1).expect("target ID exhaustion");
        self.targets.push(Target {
            aircraft: Some(config.aircraft),
            role: TargetRole::Aircraft,
            id,
            position,
            basis,
            velocity: basis.forward.map(|v| v * 300.),
            heat: Heat::Engine {
                on: true,
                throttle: 0.7,
                afterburner: false,
            },
            radar_emitting: false,
            configuration: sensors::Configuration::CLEAN,
            signature: config.sensors.signature,
            jammer: config.sensors.jammer.clone(),
            jammer_active: false,
            airborne: true,
            on_ground: false,
            radius: AIRCRAFT_RADIUS_FT,
            hp: config.hit_points,
            initial_hp: config.hit_points,
            fragment_offsets: config.fragment_offsets,
            wreck: None,
            wreck_power: config.wreck_power,
            fragment_released: false,
            localized_damage: LocalizedDamage::default(),
            faults: SystemFaults::new(config),
            category: config.target_category,
            side,
        });
    }
    /// Replace imported scene objects, parked aircraft included, without
    /// changing aircraft or fixture IDs.
    pub fn remove_ground_targets(&mut self) {
        let (bounds, parked) = (&self.ground_bounds, &self.parked);
        let scene = |id: u32| bounds.contains_key(&id) || parked.contains_key(&id);
        self.projectiles.retain(|p| !p.target.is_some_and(&scene));
        self.targets.retain(|t| !scene(t.id));
        self.ground_bounds.clear();
        self.ground_looks.clear();
        self.ground_listing.clear();
        self.parked.clear();
        for own in &mut self.ownships {
            own.sensors.clear_selection();
            own.hud_selection = None;
            own.sight_hold = None;
        }
    }
    /// Register one imported, stationary surface object without consuming an
    /// aircraft roster index. The caller owns the explicit disjoint ID range.
    /// `side` is the side the object fights for, [`NO_SIDE`] for neutral
    /// scenery.
    pub fn add_ground_target(
        &mut self,
        id: u32,
        bounds: crate::airport::OrientedBox,
        hit_points: i32,
        category: u16,
        side: Side,
    ) -> Result<()> {
        if id == 0
            || !bounds.valid()
            || hit_points <= 0
            || self.targets.iter().any(|target| target.id == id)
            || self.ground_bounds.contains_key(&id)
        {
            return Err(super::invalid("invalid or duplicate ground target"));
        }
        let basis = Basis::new(bounds.heading, bounds.pitch, bounds.bank);
        // Aim inside the upper half of the solid volume so a planar ground
        // object's own terrain endpoint does not occlude its sensor observation.
        let aim = std::array::from_fn(|i| bounds.center[i] + basis.up[i] * bounds.half[1] * 0.5);
        self.targets.push(Target {
            aircraft: None,
            role: TargetRole::Surface,
            heat: Heat::Unknown,
            radar_emitting: false,
            id,
            position: aim,
            velocity: [0.; 3],
            basis,
            configuration: sensors::Configuration::CLEAN,
            signature: sensors::SignatureProfile::default(),
            jammer: None,
            jammer_active: false,
            airborne: false,
            on_ground: false,
            radius: bounds.half[0].max(bounds.half[1]).max(bounds.half[2]),
            hp: hit_points,
            initial_hp: hit_points,
            fragment_offsets: [[0.; 3]; 2],
            wreck: None,
            wreck_power: crate::wreck::Power::default(),
            fragment_released: false,
            localized_damage: LocalizedDamage::default(),
            faults: Default::default(),
            category,
            side,
        });
        self.ground_bounds.insert(id, bounds);
        Ok(())
    }
    /// Replaces the range's target with a fresh copy of one ownship's
    /// aircraft ahead of it.
    pub fn range_target(&mut self, aircraft: u32, launcher: Launcher) {
        self.with_ownship(aircraft, |state, own| state.range_target_of(own, launcher));
    }
    fn range_target_of(&mut self, own: &mut Ownship, launcher: Launcher) {
        let distance = own.config.stations.get(own.selected).map_or(900., |s| {
            if s.weapon.seeker.signature == 0 {
                900.
            } else {
                f64::from(s.weapon.seeker.zones[1].minimum_range) + 3000.
            }
        });
        // Retire the previous engagement atomically. Never let an old missile
        // hit or track a replacement fixture with a reused identity.
        own.release();
        self.projectiles.clear();
        self.effects.clear();
        self.smoke = super::smoke::Smoke::default();
        self.devices = Default::default();
        self.note_devices(DeviceNote::Cleared(self.tick));
        self.debris.clear();
        self.targets
            .retain(|t| self.ground_bounds.contains_key(&t.id) || self.parked.contains_key(&t.id));
        let id = self.next_target_id;
        self.next_target_id = self
            .next_target_id
            .checked_add(1)
            .expect("range ID exhaustion");
        // The fixture is a copy of this aircraft, so it carries the same PT
        // signatures and ECM record. No AI or autonomous behaviour is added.
        let yaw = launcher.basis.forward[0].atan2(launcher.basis.forward[2]);
        self.targets.push(Target {
            aircraft: Some(own.config.aircraft),
            role: TargetRole::Aircraft,
            heat: Heat::Unknown,
            radar_emitting: false,
            id,
            category: self.range_category,
            position: std::array::from_fn(|i| {
                launcher.position[i] + launcher.basis.forward[i] * distance
            }),
            velocity: launcher.basis.forward.map(|v| v * 300.),
            basis: Basis::new(yaw, 0., 0.),
            configuration: sensors::Configuration::CLEAN,
            signature: own.config.sensors.signature,
            jammer: own.config.sensors.jammer.clone(),
            jammer_active: self.target_jammer,
            airborne: true,
            on_ground: false,
            radius: AIRCRAFT_RADIUS_FT,
            hp: own.config.hit_points,
            initial_hp: own.config.hit_points,
            fragment_offsets: own.config.fragment_offsets,
            wreck: None,
            wreck_power: own.config.wreck_power,
            fragment_released: false,
            localized_damage: LocalizedDamage::default(),
            faults: Default::default(),
            side: NO_SIDE,
        });
        own.sensors.clear_selection();
        own.hud_selection = None;
        own.sight_hold = None;
    }
    pub fn take_sound_events(&mut self) -> Vec<crate::acoustics::Emission> {
        std::mem::take(&mut self.sound_events)
    }
    /// Projectile damage since the last drain, oldest first.
    pub fn take_strikes(&mut self) -> Vec<Strike> {
        std::mem::take(&mut self.strikes)
    }
    /// Projectile damage since the last drain, oldest first, left in place:
    /// the mission core's score facts read them before the radio drains
    /// them.
    pub fn strikes(&self) -> &[Strike] {
        &self.strikes
    }
    fn strike(&mut self, strike: Strike) {
        if self.strikes.len() == MAX_STRIKES {
            self.strikes.remove(0);
        }
        self.strikes.push(strike);
    }

    fn emit_sound(&mut self, position: Vector, kind: crate::acoustics::Kind) {
        self.push_sound(crate::acoustics::Emission {
            kind,
            position,
            arrived: false,
            own: false,
        });
    }
    fn push_sound(&mut self, emission: crate::acoustics::Emission) {
        if self.sound_events.len() == 256 {
            self.sound_events.remove(0);
        }
        self.sound_events.push(emission);
    }

    /// One chaff cartridge or flare leaving an aircraft: its visible device
    /// (a flare leaves as a pair) and the original's release recording, one
    /// per device, for the ownships and the AI alike (docs/spec/countermeasures.md).
    /// `owner` is the releasing aircraft.
    pub fn device_released(
        &mut self,
        release: super::countermeasures::Release,
        kind: EffectKind,
        owner: u32,
    ) {
        let left = self.ownship(owner).map(|own| match kind {
            EffectKind::Chaff => own.chaff,
            _ => own.flares,
        });
        self.note_release(release, kind, owner, left);
    }
    /// A release with the releasing ownship's devices left, when it is one.
    fn note_release(
        &mut self,
        release: super::countermeasures::Release,
        kind: EffectKind,
        owner: u32,
        left: Option<u8>,
    ) {
        let position = release.position;
        match kind {
            EffectKind::Chaff => self.devices.release_chaff(release),
            _ => self.devices.release_flare(release),
        }
        if self.weapon_rules == Rules::Energy {
            use missiles::decoys;
            let class = if kind == EffectKind::Chaff {
                decoys::Kind::Chaff
            } else {
                decoys::Kind::Flare
            };
            let id = self.devices.released();
            for side in 0..if class == decoys::Kind::Flare { 2 } else { 1 } {
                self.energy_devices.release(
                    decoys::Device::from_release(id, owner, class, release, side),
                    self.tick,
                );
            }
        }
        // Write-only: what a recording needs to rebuild the device.
        self.note_devices(DeviceNote::Released(DeviceRelease {
            owner,
            kind,
            release,
            number: self.devices.released(),
            tick: self.tick,
            left,
        }));
        self.push_sound(crate::acoustics::Emission {
            kind: match kind {
                EffectKind::Chaff => crate::acoustics::Kind::Chaff,
                _ => crate::acoustics::Kind::Flare,
            },
            position,
            arrived: false,
            own: left.is_some(),
        });
    }
    fn note_devices(&mut self, note: DeviceNote) {
        if self.device_log.len() == MAX_RELEASE_RECORDS {
            self.device_log.pop_front();
        }
        self.device_log.push_back(note);
    }
    /// Chaff and flare releases and resets since the last call, oldest
    /// first. For mission recordings; combat never reads them.
    pub fn take_device_notes(&mut self) -> Vec<DeviceNote> {
        self.device_log.drain(..).collect()
    }
    /// The player's decoy rolls since the last call, oldest first. For
    /// mission recordings; combat never reads them.
    pub fn take_decoy_rolls(&mut self) -> Vec<DecoyRoll> {
        self.decoy_log.drain(..).collect()
    }

    /// A launch flash or a piece of debris landing: no explosion type.
    fn effect(&mut self, position: Vector, kind: EffectKind) {
        self.push_effect(Effect {
            position,
            kind,
            ticks: 45,
            blast: None,
            scale: super::blast_scale::FULL,
        });
    }
    /// One original explosion: its type after the variety roll sets what is
    /// drawn, for how long, and what is heard.
    fn blast(&mut self, position: Vector, kind: EffectKind, explosion: u8, scale: u8) {
        // A weapon without a reviewed type (synthetic fixtures) keeps the
        // family its effect kind implies (fitted).
        let explosion = if super::blast::explosion(explosion).is_some() {
            explosion
        } else {
            match kind {
                EffectKind::Hit => 18,
                EffectKind::Ground => 15,
                _ => super::blast::AIRCRAFT,
            }
        };
        let explosion = super::blast::vary(explosion, &mut self.blast_rolls);
        let Some(row) = super::blast::explosion(explosion) else {
            return;
        };
        // A flak burst is heard as the retail air flak whichever sheet it
        // draws: the 100 mm shell draws type 28 (fitted), but both flak
        // records name type 27, and 27's recordings are the retail flak
        // sound (docs/spec/sound.md, "Flak").
        let heard = if kind == EffectKind::Flak {
            super::blast::FLAK_SOUND
        } else {
            explosion
        };
        self.emit_sound(position, crate::acoustics::Kind::Blast(heard));
        self.push_effect(Effect {
            position,
            kind,
            ticks: u16::from(row.seconds) * 120,
            blast: Some(explosion),
            scale,
        });
    }
    fn push_effect(&mut self, effect: Effect) {
        if self.effects.len() == MAX_EFFECTS {
            self.effects.remove(0);
        }
        self.effects.push(effect);
    }
    /// Exactly one host 120 Hz tick. Pausing means NOT calling this method.
    /// The host-to-native time conversion and stage ordering are authored here.
    /// `inputs` gives the trigger and launcher of each ownship for this tick;
    /// an ownship with no input is not stepped.
    pub fn step(
        &mut self,
        inputs: &[OwnshipInput],
        ground: impl Fn(f64, f64) -> f64 + Sync,
    ) -> Vec<Event> {
        self.step_surface(inputs, ground, |_, _| false)
    }
    /// [`Self::step`] where `water` says whether a point lies over the sea,
    /// which picks water explosions and leaves no crater there.
    pub fn step_surface(
        &mut self,
        inputs: &[OwnshipInput],
        ground: impl Fn(f64, f64) -> f64 + Sync,
        water: impl Fn(f64, f64) -> bool,
    ) -> Vec<Event> {
        self.step_rewound(inputs, &[], ground, water)
    }
    /// [`Self::step_surface`] with lag compensation: `rewinds` gives, for an
    /// aircraft that fires this tick, the rewind its gun rounds carry, in
    /// ticks (capped at [`rewind::MAX_REWIND_TICKS`]). On every tick of its
    /// flight such a round tests each aircraft's hit volume from that many
    /// ticks before, as the shooter's screen showed it. Missiles, rockets and
    /// bombs never rewind, and an aircraft not listed fires rounds with none,
    /// which take exactly the path of [`Self::step_surface`].
    pub fn step_rewound(
        &mut self,
        inputs: &[OwnshipInput],
        rewinds: &[(u32, u16)],
        ground: impl Fn(f64, f64) -> f64 + Sync,
        water: impl Fn(f64, f64) -> bool,
    ) -> Vec<Event> {
        self.step_rewound_with_executor(inputs, rewinds, ground, water, tore_workers::shared())
    }

    /// The same tick with an explicit executor, for serial references and
    /// deterministic scheduling comparisons without environment overrides.
    pub fn step_rewound_with_executor(
        &mut self,
        inputs: &[OwnshipInput],
        rewinds: &[(u32, u16)],
        ground: impl Fn(f64, f64) -> f64 + Sync,
        water: impl Fn(f64, f64) -> bool,
        executor: &tore_workers::Executor,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        // The ownships are worked one at a time in aircraft id order; every
        // stage that concerns only them loops over them here.
        let mut ships = std::mem::take(&mut self.ownships);
        let active: Vec<(usize, OwnshipInput)> = ships
            .iter()
            .enumerate()
            .filter_map(|(k, own)| {
                inputs
                    .iter()
                    .find(|input| input.aircraft == own.aircraft)
                    .map(|input| (k, *input))
            })
            .collect();
        let ceiling = self.ground_ceiling.clone();
        let obscured = |from: Vector, to: Vector| {
            terrain_hit_below(from, to, &ground, ceiling.as_deref()).is_some()
        };
        for &(k, input) in &active {
            let own = &mut ships[k];
            // A hidden team's launch gives it away (T.O.R.E opinionated,
            // John 2026-10-29), before this tick's observations read it.
            self.spot_team_launches(own, input.launcher.position);
            own.enforce_guns_only(self.cheats.guns_only);
            if std::mem::take(&mut own.pending_damage) && own.hp > 0 {
                // Explicit no-AI hit fixture uses this aircraft's gun damage. Native
                // percent input is 100; deterministic adapter RNG is not native RNG.
                let base = own.config.stations.first().map_or(1, |s| {
                    scaled_weapon_damage(
                        &s.weapon,
                        i32::from(
                            s.weapon.damage.by_class[damage_class(own.config.target_category)],
                        ),
                    ) as u16
                });
                let amount =
                    super::systems::damage_amount(base, 100, draw(&mut self.rng, 40) as u8);
                own.localized_damage.record(
                    DamageSection::Core,
                    amount,
                    own.config.damage_capacity,
                );
                self.damage_ownship(own, amount, &mut events);
                self.emit_sound(input.launcher.position, crate::acoustics::Kind::Impact);
            }
        }
        let now = (self.tick / 30) as u16;
        self.tick += 1;
        self.service_remainder += 256;
        let service = (self.service_remainder / 120) as i16;
        self.service_remainder %= 120;
        for e in &mut self.effects {
            e.ticks = e.ticks.saturating_sub(1);
        }
        self.effects.retain(|e| e.ticks > 0);
        for m in &mut self.marks {
            if m.ticks != super::blast::FOREVER {
                m.ticks -= 1;
            }
        }
        self.marks.retain(|m| m.ticks > 0);
        // Each ownship is a target to every other aircraft, rebuilt every step
        // from its launcher: what the others' sensors see, what rounds hit and
        // what collides. Nothing before the damage stage changes it.
        let rows: Vec<OwnRow> = active
            .iter()
            .map(|&(index, input)| {
                let own = &mut ships[index];
                let launcher = input.launcher;
                let previous = own
                    .previous_position
                    .replace(launcher.position)
                    .unwrap_or(launcher.position);
                OwnRow {
                    index,
                    launcher,
                    previous,
                    target: ownship_target(own, launcher),
                }
            })
            .collect();
        // Cockpit questions about another ownship read its row.
        self.ownship_rows.clear();
        if rows.len() > 1 {
            self.ownship_rows
                .extend(rows.iter().map(|r| r.target.clone()));
        }
        // The aircraft with one of their own laser weapons in flight: their
        // lasers stay on (docs/spec/air-to-ground.md, "The laser").
        let laser_owners = sighting::laser_owners(
            cockpit::Cockpits {
                ships: &ships,
                kept: &self.kept,
                ai: &self.ai_cockpits,
            },
            &self.projectiles,
        );
        // The sights and lasers of the cockpits the AI flies step first:
        // those humans left (slice FX-J), then the AI strikers' (slice
        // ST-C). Their spots are this tick's for every ownship. An entry
        // whose aircraft is gone or destroyed goes, as no human can take
        // that aircraft again.
        let targets = &self.targets;
        let standing = |aircraft: &u32| targets.iter().any(|t| t.id == *aircraft && t.hp > 0);
        self.kept.retain(|aircraft, _| standing(aircraft));
        self.ai_cockpits.retain(|aircraft, _| standing(aircraft));
        let flown = self
            .kept
            .iter_mut()
            .map(|(aircraft, kept)| {
                (
                    *aircraft,
                    kept.side,
                    &mut kept.cockpit,
                    sighting::Pilot::Kept,
                )
            })
            .chain(self.ai_cockpits.iter_mut().map(|(aircraft, ai)| {
                (
                    *aircraft,
                    ai.side,
                    &mut ai.cockpit,
                    sighting::Pilot::Striker,
                )
            }));
        for (aircraft, side, cockpit, pilot) in flown {
            let row = self
                .targets
                .iter()
                .find(|t| t.id == aircraft)
                .expect("a flown aircraft's row");
            sighting::step_flown(
                cockpit,
                (aircraft, side),
                pilot,
                row,
                &self.targets,
                &sighting::Context {
                    ground: &ground,
                    weather: &self.sensor_weather,
                    laser_owners: &laser_owners,
                    armed: &self.ai_laser_armed,
                    ai_on: &self.ai_laser_on,
                    tick: self.tick,
                },
            );
        }
        // Every laser spot as the last step left it: a guided release and
        // the guided sight read the other ownships' spots here, and their
        // own aircraft's fresh one.
        let spots_before = guided::spots(cockpit::Cockpits {
            ships: &ships,
            kept: &self.kept,
            ai: &self.ai_cockpits,
        });
        // These observations read fixed rows and change only their ownship.
        // Readiness and every release remain below, in aircraft id order.
        // Fitted: at least two active cockpits amortize a scoped dispatch.
        let observations_prepared = executor.should_dispatch(active.len(), 2);
        if observations_prepared {
            let facts = self.ground_facts();
            let context = observation::Context {
                facts: &facts,
                targets: &self.targets,
                rows: &rows,
                weapon_rules: self.weapon_rules,
                easy_targeting: self.cheats.easy_targeting,
                tick: self.tick,
                ground: &ground,
            };
            executor.for_each_mut(&mut ships, 2, |index, own| {
                if let Some((_, input)) = active.iter().find(|(k, _)| *k == index) {
                    observation::observe_ownship(own, index, input.launcher, &context);
                }
            });
        }
        for &(k, input) in &active {
            let own = &mut ships[k];
            let (launcher, held) = (input.launcher, input.held);
            if !observations_prepared {
                observation::observe_ownship(
                    own,
                    k,
                    launcher,
                    &observation::Context {
                        facts: &self.ground_facts(),
                        targets: &self.targets,
                        rows: &rows,
                        weapon_rules: self.weapon_rules,
                        easy_targeting: self.cheats.easy_targeting,
                        tick: self.tick,
                        ground: &ground,
                    },
                );
            }
            if own.sight.is_some() {
                sighting::step(
                    own,
                    launcher,
                    &self.targets,
                    &sighting::Context {
                        ground: &ground,
                        weather: &self.sensor_weather,
                        laser_owners: &laser_owners,
                        armed: &self.ai_laser_armed,
                        ai_on: &self.ai_laser_on,
                        tick: self.tick,
                    },
                );
            }
            // The cockpit's target is the designation as this step leaves
            // it, for the sight, the solution and the release below.
            own.sync_target();
            own.guided_sight = guided::sight(
                own.aircraft,
                own.side,
                &own.cockpit,
                &own.config,
                &launcher,
                &guided::SightContext {
                    spots: &spots_before,
                    targets: &self.targets,
                    weather: &self.sensor_weather,
                    ground: &ground,
                },
            );
            if own.config.stations.is_empty() {
                own.release_readiness = Readiness::Safe;
                own.release();
                continue;
            }
            if own.gunship.is_some() {
                step_gunship(own, k, launcher, &self.targets, &rows, &ground, self.tick);
            }
            // The sight predicts from the pylon the next store leaves (B3):
            // with unlimited ammunition the release's first number picks it.
            let mount = own.config.stations.get(own.selected).map_or([0.; 3], |s| {
                if self.cheats.unlimited_ammo {
                    s.release_point(self.next_shot & 1)
                } else {
                    s.next_release_point(u32::from(own.rounds(own.selected)))
                }
            });
            own.delivery = self.delivery_of(
                own.aircraft,
                &own.cockpit,
                &own.config,
                &launcher,
                mount,
                &ground,
                super::delivery::Clock {
                    tick: self.tick - 1,
                    service,
                    remainder: self.service_remainder,
                },
                false,
                // A human's rocket pipper always has a ground point.
                true,
            );
            let selected = own.selected;
            let grouped = own
                .gunship
                .as_ref()
                .is_some_and(|group| group.slot(selected).is_some());
            let fire_stations = if grouped {
                own.gunship
                    .as_ref()
                    .expect("grouped gunship")
                    .fire_stations()
            } else {
                vec![selected]
            };
            if fire_stations.is_empty() {
                own.release_readiness = Readiness::GroupEmpty;
                own.release();
            }
            // Tab fires the gun whatever is selected: with the gun (or the
            // AC-130's guns) selected it joins the trigger; with another
            // store selected the gun gets a pass of its own, which leaves the
            // selected store's readiness, bay request and seeker alone.
            let gun_key = own.gun_key && launcher.alive;
            let mut passes: Vec<(usize, bool, bool)> = fire_stations
                .iter()
                .map(|&index| {
                    let gun = grouped || is_gun(&own.config.stations[index].weapon);
                    (index, held || (gun_key && gun), false)
                })
                .collect();
            if gun_key
                && !grouped
                && !fire_stations.is_empty()
                && !is_gun(&own.config.stations[selected].weapon)
                && let Some(index) = key_gun_station(&own.config, |i| own.rounds(i))
            {
                passes.push((index, true, true));
            }
            for (index, held, key_pass) in passes {
                own.selected = index;
                let selected_readiness = own.release_readiness;
                own.release_readiness = if grouped {
                    let slot = own
                        .gunship
                        .as_ref()
                        .expect("grouped gunship")
                        .slot(index)
                        .expect("gun slot");
                    if !launcher.alive {
                        Readiness::LauncherLost
                    } else if self.projectiles.len() >= MAX_PROJECTILES {
                        Readiness::Capacity
                    } else {
                        own.gun_readiness(slot)
                    }
                } else {
                    view(self, own)
                        .at(launcher)
                        .with_terrain(&ground)
                        .readiness()
                };
                let bay_waits = own.bay_waits(launcher);
                // A pending bay release lapses if the shot is no longer wanted or the
                // doors never open.
                if !key_pass
                    && own.bay_release.is_some_and(|(station, pressed)| {
                        station != index
                            || self.tick.saturating_sub(pressed) > BAY_RELEASE_TICKS
                            || !(own.release_readiness == Readiness::BayClosed
                                || own
                                    .release_readiness
                                    .player_missile_may_fire(&own.config.stations[index].weapon))
                    })
                {
                    own.bay_release = None;
                }
                // Guns and missiles retain advisory cues without inhibiting release.
                let allowed = if grouped {
                    own.release_readiness.gun_may_fire()
                } else {
                    own.release_readiness
                        .player_missile_may_fire(&own.config.stations[index].weapon)
                } && !bay_waits;
                let station = &own.config.stations[index];
                let w = &station.weapon;
                let gun = is_gun(w);
                let pressed = held && launcher.alive && !own.triggers[index].was_held;
                let polled = own.triggers[index].poll(
                    held && launcher.alive,
                    w.flags,
                    w.burst.game_burst_t,
                    now,
                );
                // LVL holds a bomb's trigger while the release cue lies ahead:
                // a held trigger waits for the cue and releases there, once a
                // pass (docs/spec/air-to-ground.md, "The level release T").
                let held_for_bar = own
                    .delivery
                    .as_ref()
                    .and_then(|d| d.level_trigger(index))
                    .or_else(|| {
                        // A guided bomb's T: its sight is for the selected
                        // station only.
                        own.guided_sight
                            .as_ref()
                            .filter(|_| index == own.selected)
                            .and_then(|g| g.level_trigger())
                    });
                let polled = match held_for_bar {
                    Some(due) => {
                        due && held
                            && launcher.alive
                            && !self.projectiles.iter().any(|p| {
                                p.owner == own.aircraft
                                    && p.station == index
                                    && p.age <= super::delivery::RELEASE_GUARD_TICKS
                            })
                    }
                    None => polled,
                };
                if polled && bay_waits && own.release_readiness.player_missile_may_fire(w) {
                    own.bay_release = Some((index, self.tick));
                    own.release_readiness = Readiness::BayClosed;
                }
                let bay_open = !key_pass && own.bay_release.is_some() && !bay_waits;
                if bay_open {
                    own.bay_release = None;
                }
                let due = polled || bay_open
                // A gun repress uses the retained physical-round deadline,
                // not the old representative burst's quarter-second deadline.
                || (gun && pressed);
                let (count, debit, gun_round, tracer) = if gun {
                    let cadence = &mut own.gun_cadence[index];
                    let physical_rounds = u16::from(w.burst.game_rounds_in_burst.max(1))
                        .saturating_mul(u16::from(w.burst.actual_rounds_per_game.max(1)));
                    if due && allowed {
                        cadence.pending = cadence
                            .pending
                            .saturating_add(physical_rounds)
                            .min(physical_rounds);
                        cadence.next_scaled = cadence
                            .next_scaled
                            .max(self.tick.saturating_mul(u64::from(physical_rounds)));
                    }
                    if !held || !launcher.alive {
                        cadence.pending = 0;
                    }
                    if !allowed && cadence.pending > 0 {
                        cadence.next_scaled = self
                            .tick
                            .saturating_mul(u64::from(physical_rounds))
                            .saturating_add(super::gun_round::blocked_push(w));
                    }
                    let ready = cadence.pending > 0
                        && allowed
                        && self.tick.saturating_mul(u64::from(physical_rounds))
                            >= cadence.next_scaled;
                    if ready {
                        let ordinal = cadence.ordinal;
                        (
                            1,
                            1,
                            Some(
                                (ordinal % u64::from(w.burst.actual_rounds_per_game.max(1))) as u8,
                            ),
                            super::gun_round::tracer(w, ordinal),
                        )
                    } else {
                        (0, 1, None, false)
                    }
                } else if due && allowed {
                    // One trigger call: a pod's one rocket, or a record's
                    // whole burst (a bomb pack in one tick), for one debit.
                    (
                        super::ballistics::release_count(w),
                        u16::from(w.burst.actual_rounds_per_game),
                        None,
                        false,
                    )
                } else {
                    (0, u16::from(w.burst.actual_rounds_per_game), None, false)
                };
                let mut fired = false;
                // With unlimited ammunition the count never moves, so a
                // paired station alternates by the call's first number.
                let unlimited_side = self.next_shot & 1;
                if count > 0 {
                    for round in 0..count {
                        // A gun debits each round; any other call debits once,
                        // for the first of its projectiles (retail PROJFire).
                        let loaded = if !gun && round > 0 {
                            true
                        } else if self.cheats.unlimited_ammo {
                            own.rounds(index) > 0
                        } else {
                            unload(&mut own.ammo[index], debit)
                        };
                        if self.projectiles.len() == MAX_PROJECTILES || !loaded {
                            break;
                        }
                        let gun_pose = own.gunship.as_ref().and_then(|group| {
                            group
                                .slot(index)
                                .map(|slot| (slot, group.headings[slot], group.elevations[slot]))
                        });
                        let profile = missiles::Profile::for_weapon(w);
                        let designated = own.designated();
                        let supported = designated.is_some_and(|id| {
                            launcher.radar_power
                                && launcher.radar
                                && !own.radar_failed
                                && agm::ownship_supports(
                                    own,
                                    profile,
                                    w,
                                    &launcher,
                                    id,
                                    self.targets.iter().find(|t| t.id == id),
                                )
                        });
                        let mounted_lock = own.mounted.acquired
                            && matches!(own.mounted.status, Status::Locked | Status::Pitbull);
                        // An unsupported player shot starts with its own search, never
                        // a hidden cockpit target. Supported SARH retains identity so
                        // later illumination can revive guidance.
                        let searching = self.weapon_rules.uses_seeker()
                            && profile.is_some_and(|p| match p.guidance {
                                Guidance::Active => {
                                    !supported
                                        || own.launch_mode == LaunchMode::Boresight
                                        || own.release_readiness == Readiness::FieldOfView
                                }
                                Guidance::Infrared => {
                                    !mounted_lock
                                        || (own.launch_mode == LaunchMode::Cued
                                            && own.mounted.target != designated)
                                }
                                _ => false,
                            });
                        let permanently_unguided = self.weapon_rules.uses_seeker()
                            && profile.is_some_and(|p| {
                                !p.guidance_available(launcher.radar_power)
                                    && p.role != TargetRole::Aircraft
                                    && !matches!(p.guidance, Guidance::Active | Guidance::Supported)
                            });
                        let target = if searching || permanently_unguided {
                            None
                        } else if own.launch_mode == LaunchMode::Boresight {
                            own.mounted.target
                        } else {
                            designated
                        };
                        // Unsupported SARH uses the existing uncued mode to retain
                        // search/reacquisition through later support changes. AI and
                        // supported surface releases keep their cued track policy.
                        let waiting_for_support = self.weapon_rules.uses_seeker()
                            && !supported
                            && profile.is_some_and(|p| p.guidance == Guidance::Supported);
                        let mode = if searching || waiting_for_support {
                            LaunchMode::Boresight
                        } else {
                            own.launch_mode
                        };
                        let guidance = missiles::Profile::for_weapon(w)
                            .filter(|_| self.weapon_rules.uses_seeker())
                            .map(|profile| {
                                let mut flight =
                                    Flight::new(profile, mode, target, launcher.position);
                                flight.qualified_target = target.filter(|id| {
                                    self.targets
                                        .iter()
                                        .chain(peers(&rows, k))
                                        .find(|t| t.id == *id)
                                        .is_some_and(|t| flight.eligible(w, t))
                                });
                                if own.mounted.acquired
                                    && own.mounted.target == target
                                    && (profile.guidance != Guidance::Active
                                        || own.launch_mode == LaunchMode::Boresight)
                                {
                                    flight.seeker = own.mounted.clone();
                                }
                                // A surface radar weapon leaves for the point
                                // the A-G track gives; the track refreshes it.
                                if profile.needs_air_ground()
                                    && supported
                                    && let Some(contact) =
                                        target.and_then(|id| own.sensors.observation(id))
                                {
                                    flight.last_intercept = Some(contact.position);
                                }
                                if permanently_unguided {
                                    flight.unguided = true;
                                    flight.enabled = false;
                                    flight.seeker = Seeker::default();
                                    flight.seeker.status = Status::Unguided;
                                }
                                flight
                            });
                        // The whole releases left on the station, which pick
                        // the side of a paired station this one leaves (B3).
                        let units_after = if self.cheats.unlimited_ammo {
                            unlimited_side
                        } else {
                            super::hardpoints::units(u32::from(own.rounds(index)), u32::from(debit))
                        };
                        // The one release function, a human's trigger and
                        // an AI's release alike ([`cockpit`]).
                        let released = self.release_round(
                            &mut own.cockpit,
                            cockpit::Round {
                                id: self.next_shot,
                                owner: own.aircraft,
                                side: own.side,
                                index,
                                station,
                                round,
                                launcher,
                                launched_t: now,
                                record: None,
                                incoming: None,
                                gun_pose,
                                guidance,
                                seeker_target: target,
                                gun_round,
                                tracer,
                                units_after,
                            },
                            &spots_before,
                            &ground,
                        );
                        if !released {
                            break;
                        }
                        if let Some(flight) =
                            self.projectiles.last().and_then(|p| p.guidance.as_ref())
                            && flight.profile.guidance == Guidance::Active
                            && flight.enabled
                        {
                            events.push(Event::SeekerActivated(self.next_shot));
                            if flight.seeker.acquired {
                                events.push(Event::Pitbull(self.next_shot));
                            }
                        }
                        if let Some(ticks) = rewinds
                            .iter()
                            .find(|(aircraft, _)| *aircraft == own.aircraft)
                            .map(|(_, ticks)| (*ticks).min(rewind::MAX_REWIND_TICKS))
                            .filter(|ticks| gun && *ticks > 0)
                        {
                            self.rewinds.insert(self.next_shot, ticks);
                        }
                        own.shots += 1;
                        self.next_shot += 1;
                        fired = true;
                        events.push(Event::Fired {
                            aircraft: own.aircraft,
                            station: index,
                        });
                        if gun {
                            let cadence = &mut own.gun_cadence[index];
                            cadence.pending -= 1;
                            cadence.ordinal = cadence.ordinal.wrapping_add(1);
                            cadence.next_scaled = cadence.next_scaled.saturating_add(
                                u64::from(w.burst.game_burst_t.max(1)).saturating_mul(30),
                            );
                        }
                    }
                }
                if fired {
                    if station.rack.location().bay() && !gun {
                        own.bay_hold_until = self.tick + BAY_HOLD_TICKS;
                    }
                    self.effect(launcher.position, EffectKind::Launch);
                    if !key_pass {
                        own.bore_observation = None;
                        own.mounted = Seeker::default();
                        own.mounted_key = None;
                    }
                }
                if key_pass {
                    own.release_readiness = selected_readiness;
                }
            }
            own.selected = selected;
            if grouped {
                own.release_readiness = view(self, own).readiness(launcher);
            }
        }
        // Living target poses remain owned by their existing flight service.
        let old_targets: Vec<_> = self.targets.iter().map(|t| t.position).collect();
        let mut airbursts = Vec::new();
        let mut crashes = Vec::new();
        for t in &mut self.targets {
            if t.hp > 0 {
                for i in 0..3 {
                    t.position[i] += t.velocity[i] / 120.;
                }
            } else if t.airborne {
                let wreck = t.wreck.get_or_insert_with(|| {
                    let mut wreck = crate::wreck::Wreck::new(t.id, self.tick, [0.; 3]);
                    if !matches!(t.heat, Heat::Engine { on: false, .. }) {
                        wreck.power = t.wreck_power;
                    }
                    wreck
                });
                if let Some(phase) =
                    wreck.step(&mut t.position, &mut t.velocity, &mut t.basis, &ground)
                {
                    t.airborne = false;
                    if phase == crate::wreck::Phase::Exploded {
                        airbursts.push((t.id, t.position));
                    } else {
                        crashes.push((t.id, t.position));
                    }
                }
            }
        }
        for (id, position) in airbursts {
            self.debris.retain(|piece| piece.owner != id);
            self.blast(
                position,
                EffectKind::Destroyed,
                super::blast::AIRCRAFT,
                super::blast_scale::FULL,
            );
            events.push(Event::Airburst(id));
        }
        for (id, position) in crashes {
            self.aircraft_crashed(id, position, water(position[0], position[2]));
        }
        // Every aircraft's hit volume as the search below reads it, for the
        // rounds that look back.
        let ground_bounds = &self.ground_bounds;
        self.volumes.record(
            self.tick,
            rows.iter()
                .map(|r| {
                    (
                        r.target.id,
                        rewind::HitVolume {
                            position: r.target.position,
                            previous: r.previous,
                            basis: r.target.basis,
                            radius: r.target.radius,
                        },
                    )
                })
                .chain(
                    self.targets
                        .iter()
                        .zip(&old_targets)
                        .filter(|(t, _)| {
                            t.role == TargetRole::Aircraft && !ground_bounds.contains_key(&t.id)
                        })
                        .map(|(t, previous)| {
                            (
                                t.id,
                                rewind::HitVolume {
                                    position: t.position,
                                    previous: *previous,
                                    basis: t.basis,
                                    radius: t.radius,
                                },
                            )
                        }),
                ),
        );
        let mut ownship_hits = Vec::new();
        let mut strikes = Vec::new();
        let mut impacts = Vec::new();
        let mut sources = Vec::new();
        // Score changes, owner and whether it is a kill, applied after the loop.
        let mut scored: Vec<(u32, bool)> = Vec::new();
        // Jammer deception on target hits takes the first ownship's ECM record.
        let fixture_ecm = ships.first().map(|own| own.config.ecm);
        let friendly_fire_off = self.friendly_fire == FriendlyFire::Off;
        // The first pass of the contact search: which targets and ownship
        // rows a round's segment can reach this tick at all.
        let body = |t: &Target, previous: Vector| broad::Body {
            previous,
            position: t.position,
            basis: t.basis,
            radius: t.radius,
            solid: self
                .ground_bounds
                .get(&t.id)
                .map(|bounds| (bounds.center, bounds.half)),
        };
        // These three are read only for a round that is not an inert store,
        // so a sky with no such round in the air (the usual case between
        // fights) does not build them.
        let live_rounds = if self.jettisoned.is_empty() {
            !self.projectiles.is_empty()
        } else {
            self.projectiles
                .iter()
                .any(|p| !self.jettisoned.contains_key(&p.id))
        };
        let target_broad = broad::Broad::new(
            self.targets
                .iter()
                .zip(&old_targets)
                .filter(|_| live_rounds)
                .map(|(t, previous)| body(t, *previous)),
        );
        let row_bounds: Vec<Option<broad::Bounds>> = rows
            .iter()
            .filter(|_| live_rounds)
            .map(|r| {
                broad::Body {
                    solid: None,
                    ..body(&r.target, r.previous)
                }
                .bounds()
            })
            .collect();
        // Each aircraft's side, the first row of an id answering, for the
        // friendly fire rule and for whom a surface round counts as hostile.
        let mut target_sides: BTreeMap<u32, Side> = BTreeMap::new();
        if live_rounds && (friendly_fire_off || !self.surface_rounds.is_empty()) {
            for t in &self.targets {
                target_sides.entry(t.id).or_insert(t.side);
            }
        }
        let mut candidates: Vec<usize> = Vec::new();
        // Every round's collateral bursts, applied after the search.
        let mut bursts: Vec<collateral::Burst> = Vec::new();
        // A round's pass reads its weapon record while the round is borrowed
        // whole, so the record is taken out of the round for the pass (a
        // round that carries its own has no station to read) and put back
        // after it, with no clone of the record.
        let mut pass = |p: &mut Projectile, owned: &Option<Weapon>| -> bool {
            let w = owned.as_ref().unwrap_or_else(|| {
                &owner_configuration(&ships, p.owner).stations[p.station].weapon
            });
            // Inert stores never enter guidance, fuzing, collision damage or scoring.
            if self.jettisoned.contains_key(&p.id) {
                let Some(motion) = self.bombs.get(&p.id) else {
                    return false;
                };
                p.previous = p.position;
                p.age += 1;
                let seconds = p.age as f64 / 120.;
                // The stored motion's position, direction and velocity at
                // `seconds`, from one sample on the store's cursor.
                let flight = self.falls.follow(p.id, motion).at(motion, seconds);
                p.position = flight.position;
                p.direction = unit(flight.velocity);
                p.speed_f8 = (missiles::length(flight.velocity) * 256.) as i32;
                if let Some(at) = terrain_hit(p.previous, p.position, &ground) {
                    let mut position = std::array::from_fn(|i| {
                        p.previous[i] + (p.position[i] - p.previous[i]) * at
                    });
                    position[1] = ground(position[0], position[2]);
                    let wet = water(position[0], position[2]);
                    impacts.push((
                        position,
                        EffectKind::Ground,
                        if wet { 17 } else { 15 },
                        (!wet).then_some(super::blast::POCK),
                        super::blast_scale::INERT,
                    ));
                    return false;
                }
                return true;
            }
            let by_ownship = ships.iter().any(|o| o.aircraft == p.owner);
            let easy = self.cheats.easy_aiming && by_ownship && p.incoming.is_none();
            // Classified once; the eased copy keeps the record.
            let gun = is_gun(w);
            let eased = (easy && !gun).then(|| eased_weapon(w));
            let w = eased.as_ref().unwrap_or(w);
            // A surface unit's round, and its shooter's side: who is hostile
            // to its flak fuze, and whom friendly fire spares from any
            // round's hits and bursts.
            let surface = self.surface_rounds.get(&p.id).copied();
            let owner_side = if surface.is_some() || friendly_fire_off {
                ships
                    .iter()
                    .find(|o| o.aircraft == p.owner)
                    .map(|o| o.side)
                    .or_else(|| target_sides.get(&p.owner).copied())
                    .unwrap_or(NO_SIDE)
            } else {
                NO_SIDE
            };
            let surface_gun = surface.is_some() && gun;
            let shooter = collateral::Shooter {
                side: if friendly_fire_off {
                    owner_side
                } else {
                    NO_SIDE
                },
                surface: surface.map(|_| owner_side),
                ownship: by_ownship,
            };
            let hitbox = if easy {
                crate::cheats::EASY_AIMING_HITBOX
            } else {
                1.
            };
            // A gun round a human fired with a view tests every aircraft's
            // volume from that many ticks back; every other round, the
            // current one, by today's code.
            let rewind = self
                .rewinds
                .get(&p.id)
                .copied()
                .filter(|ticks| *ticks > 0 && gun);
            let volumes = &self.volumes;
            let past = |id: u32| rewind.and_then(|ticks| volumes.volume(id, ticks));
            if p.age == 0 {
                p.direction = projectile_launch_direction(w, p.direction, p.id, p.owner, p.station);
                self.ledger
                    .launch(p.id, p.owner, p.incoming.or(p.target), ShotKind::of(w));
            }
            let m = &w.movement;
            if if super::ballistics::is_bomb(w)
                || (super::guided::is_glide_bomb(w) && p.motion.is_some())
            {
                p.age >= 36_000 || p.position[1] > 100_000.
            } else if let Some(motion) = &p.motion {
                motion.energy.map_or_else(
                    || missiles::removed(m, p.age),
                    |e| p.age >= u64::from(e.body.life),
                ) || p.position[1] > 100000.
            } else {
                removal_due(m, now, p.launched_t, (p.position[1] * 256.) as i32)
            } {
                if surface.is_some_and(|round| round.flak) {
                    // A flak shell that reaches the end of its life bursts there.
                    impacts.push((
                        p.position,
                        EffectKind::Flak,
                        flak_explosion(w),
                        None,
                        super::blast_scale::of(w),
                    ));
                    bursts.push(collateral::Burst::new(
                        p,
                        w,
                        shooter,
                        p.position,
                        collateral::Detonation::Air,
                        None,
                        true,
                    ));
                } else {
                    self.ledger.resolve(p.id, Resolution::Missed);
                }
                return false;
            }
            p.previous = p.position;
            let mut fuze_until = 1.;
            let phase = if let Some(motion) = &p.motion {
                motion.engine_phase(m, p.age)
            } else {
                engine_phase(m, now, p.launched_t)
            };
            // Every motor smokes while it burns, a rocket's (no seeker) as
            // well as a missile's (bug B6); gun rounds and bombs have none.
            // The same rule as `motor_powered`.
            if phase == super::EnginePhase::Powered && !gun {
                sources.push((
                    std::array::from_fn(|i| p.position[i] - p.direction[i] * 4.),
                    super::smoke::Kind::Missile,
                ));
            }
            let old_direction = p.direction;
            if let Some(motion) = &mut p.motion {
                motion.prepare(p.position, p.direction, p.age, self.devices.wind);
            }
            if p.guidance.is_some() {
                let was_active = p.guidance.as_ref().unwrap().enabled;
                let was_acquired = p.guidance.as_ref().unwrap().seeker.acquired;
                let actor_support = if let Some(row) = rows.iter().find(|r| r.target.id == p.owner)
                {
                    let own = &ships[row.index];
                    p.guidance
                        .as_ref()
                        .and_then(|flight| {
                            flight.seeker.target.or_else(|| {
                                (flight.profile.guidance == Guidance::Supported)
                                    .then(|| own.designated())
                                    .flatten()
                            })
                        })
                        .map(|id| ActorSupport {
                            owner: p.owner,
                            supported: agm::ownship_supports(
                                own,
                                p.guidance.as_ref().map(|flight| flight.profile),
                                w,
                                &row.launcher,
                                id,
                                self.targets.iter().find(|t| t.id == id),
                            ),
                            observation: own.sensors.observation(id).map(|contact| {
                                let delta = sub(contact.position, p.position);
                                seeker::Observation {
                                    id,
                                    position: contact.position,
                                    velocity: contact.velocity,
                                    quality: 1.,
                                    off_axis: dot(unit(delta), p.direction).clamp(-1., 1.).acos(),
                                    range: missiles::length(delta),
                                }
                            }),
                            radar_position: row.launcher.position,
                            radar_emitting: row.launcher.radar && row.launcher.alive,
                        })
                } else {
                    p.guidance
                        .as_ref()
                        .and_then(|flight| flight.seeker.target)
                        .and_then(|id| self.actor_support.get(&(p.owner, id)).copied())
                };
                let owner = p.owner;
                guide_owned(
                    p,
                    w,
                    &self.targets,
                    rows.iter().map(|r| &r.target).filter(|t| t.id != owner),
                    actor_support.as_ref(),
                    &obscured,
                    &ground,
                );
                if let Some(target) = p.target.and_then(|id| {
                    self.targets
                        .iter()
                        .chain(rows.iter().map(|r| &r.target))
                        .find(|t| t.id == id)
                }) && let Some(roll) = energy_decoy(
                    p,
                    w,
                    target,
                    &self.energy_devices,
                    self.energy_seed,
                    &obscured,
                ) {
                    if roll.decoyed {
                        self.ledger.resolve(p.id, Resolution::Spoofed);
                    }
                    if self.decoy_log.len() == MAX_RELEASE_RECORDS {
                        self.decoy_log.pop_front();
                    }
                    self.decoy_log.push_back(roll);
                }
                let f = p.guidance.as_ref().unwrap();
                if f.profile.guidance == Guidance::Active {
                    if !was_active && f.enabled {
                        events.push(Event::SeekerActivated(p.id));
                    }
                    if !was_acquired && f.seeker.status == Status::Pitbull {
                        events.push(Event::Pitbull(p.id));
                    }
                }
            } else {
                if p.guidance_ticks.is_some_and(|end| p.age >= end) {
                    p.target = None;
                }
                if let Some(t) = p.target.and_then(|id| {
                    rows.iter()
                        .find(|r| r.target.id == id && r.target.body_present())
                        .map(|r| &r.target)
                        .or_else(|| self.targets.iter().find(|t| t.id == id && t.body_present()))
                }) {
                    // Required illumination is specific to this missile's own
                    // target, never to whatever the cockpit has selected now.
                    let supported = if p.incoming.is_some() {
                        // The diagnostic incoming round is always illuminated.
                        true
                    } else if owned.is_some() {
                        self.targets.iter().any(|owner| {
                            owner.id == p.owner && owner.hp > 0 && owner.radar_emitting
                        })
                    } else {
                        owner_ownship(&ships, p.owner).is_some_and(|own| own.sensors.supports(t.id))
                    };
                    if acquisition(w, p.position, p.direction, t.position, supported, 0)
                        && terrain_hit(p.position, t.position, &ground).is_none()
                    {
                        let desired = unit(sub(t.position, p.position));
                        let rate = if phase == EnginePhase::Powered {
                            m.powered_turn_rate
                        } else {
                            m.unpowered_turn_rate
                        };
                        // Authored pursuit, capped by source angle-rate field; native
                        // PN, lead, sun, Doppler, ECM and RNG contracts remain open.
                        let angle = dot(p.direction, desired).clamp(-1., 1.).acos();
                        let fraction = (f64::from(rate.max(0)) * std::f64::consts::TAU
                            / 65520.
                            / 120.
                            / angle.max(1e-9))
                        .min(1.);
                        p.direction = unit(std::array::from_fn(|i| {
                            p.direction[i] * (1. - fraction) + desired[i] * fraction
                        }));
                    } else {
                        events.push(Event::TrackLost(t.id));
                        p.target = None;
                    }
                } else if let Some(id) = p.target.take() {
                    events.push(Event::TrackLost(id));
                }
            }
            // A guided air-to-ground weapon steers itself (turning its
            // motion with it); one that is not guiding may fall instead.
            let guided_fall = self.guided.get_mut(&p.id).map(|flight| {
                guided::steer(
                    flight,
                    p,
                    w,
                    &guided::Context {
                        ships: &ships,
                        cockpits: cockpit::Cockpits {
                            ships: &ships,
                            kept: &self.kept,
                            ai: &self.ai_cockpits,
                        },
                        rows: &rows,
                        targets: &self.targets,
                        ground: &ground,
                    },
                    &mut self.falls,
                )
            });
            if super::guided::is_glide_bomb(w)
                && let Some(motion) = p.motion.as_mut()
            {
                let target = self
                    .guided
                    .get(&p.id)
                    .filter(|f| f.guiding())
                    .and_then(|f| f.point);
                let mut flight = super::bomb_flight::Flight {
                    position: p.position,
                    velocity: motion.velocity,
                };
                flight.step(super::bomb_flight::Body::of(w), p.age, target);
                p.position = flight.position;
                motion.velocity = flight.velocity;
                p.direction = unit(flight.velocity);
                p.speed_f8 = (missiles::length(flight.velocity) * 256.) as i32;
            } else if let Some(Some((fall, start))) = guided_fall {
                let seconds = (p.age + 1 - start) as f64 / 120.;
                let flight = self.falls.follow(p.id, &fall).at(&fall, seconds);
                p.position = flight.position;
                p.direction = unit(flight.velocity);
                p.speed_f8 = (missiles::length(flight.velocity) * 256.) as i32;
            } else if let Some(motion) = &mut p.motion {
                if guided_fall.is_none() {
                    motion.turn(old_direction, p.direction);
                }
                let delta = motion.step_over_terrain(m, p.age, p.direction, &ground);
                for (position, delta) in p.position.iter_mut().zip(delta) {
                    *position += delta;
                }
                p.speed_f8 = (missiles::length(motion.velocity) * 256.) as i32;
                if let Some(e) = &mut motion.energy {
                    p.direction = e.flight.axis;
                    if let Some(pursuit) = &mut e.decoy.pursuit {
                        fuze_until = pursuit.after_motion(
                            p.previous.map(|v| v * missiles::energy::FT),
                            e.flight.position,
                        );
                    }
                    if e.flight.defeated {
                        fuze_until = 0.;
                    }
                }
            } else if super::ballistics::is_bomb(w) {
                // A bomb record advances on the shared integration grid. One placed in
                // flight without a release (a driver's or a test's round)
                // starts its path where it is, at its own speed.
                let motion = self.bombs.entry(p.id).or_insert_with(|| {
                    super::ballistics::BombMotion::new(super::ballistics::ReleaseState {
                        position: p.position,
                        velocity: p.direction.map(|v| v * f64::from(p.speed_f8) / 256.),
                        body: super::bomb_flight::Body::of(w),
                    })
                });
                motion.advance(p.age + 1);
                p.position = motion.flight.position;
                p.direction = unit(motion.flight.velocity);
                p.speed_f8 = (missiles::length(motion.flight.velocity) * 256.) as i32;
            } else {
                if w.flags & 0x40 != 0 {
                    let target =
                        commanded_speed(m, phase, p.speed_f8, (p.position[1] * 256.) as i32) as i16;
                    p.speed_f8 = axial_speed(m, p.speed_f8, target, false, service)
                        .expect("validated movement");
                }
                let distance = f64::from(p.speed_f8) * f64::from(service) / 65536.;
                for i in 0..3 {
                    p.position[i] += p.direction[i] * distance;
                }
                p.position[1] = f64::from(
                    p.fall
                        .advance(
                            w.flags & 4 != 0,
                            phase,
                            service,
                            (p.position[1] * 256.) as i32,
                        )
                        .expect("positive service"),
                ) / 256.;
            }
            let armed = if p.motion.is_some() {
                p.age >= u64::from(w.damage.fuze_arm_t) * 30
            } else {
                now.wrapping_sub(p.launched_t) >= w.damage.fuze_arm_t
            };
            // Launch grace for a drop launch: a store whose motor lights after
            // release falls below its launcher, lights at about the edge of the
            // launcher's box and climbs ahead past it, so it spares its launcher
            // until a second after its motor lights, wherever it is.
            let separating = w.movement.ignite_t > 0
                && if p.motion.is_some() {
                    p.age < (u64::from(w.movement.ignite_t) + DROP_SEPARATION_T) * 30
                } else {
                    now.wrapping_sub(p.launched_t)
                        < w.movement.ignite_t.saturating_add(DROP_SEPARATION_T as u16)
                };
            p.age += 1;
            let mut first: Option<(f64, Option<Hit>)> = None;
            // With friendly fire off, no round damages an aircraft of its
            // shooter's own side, the shooter included.
            let shooter_side = shooter.side;
            let spares =
                |side: Side| friendly_fire_off && shooter_side != NO_SIDE && side == shooter_side;
            // The round's swept segment widened by its fuze, for the first
            // pass; a round tested against past volumes keeps it for the
            // ground objects' solid boxes alone.
            let segment = broad::Bounds::segment(
                p.previous,
                p.position,
                f64::from(w.damage.fuze_radius.max(0)),
            );
            let reach = if rewind.is_none() { segment } else { None };
            // A rewound round meets an aircraft's past volume, which the
            // first pass bounds as it bounds a volume now: the round's
            // segment that misses the box cannot touch it.
            let past_missed = |id: u32| {
                past(id).is_some_and(|v| {
                    let body = broad::Body {
                        previous: v.previous,
                        position: v.position,
                        basis: v.basis,
                        radius: v.radius,
                        solid: None,
                    };
                    matches!((body.bounds(), segment), (Some(b), Some(s)) if !b.overlaps(&s))
                })
            };
            // One search over every aircraft row and every ownship. A gun round
            // and missile never hit their own launcher. Bomb launch grace remains separate.
            if armed {
                for (n, r) in rows.iter().enumerate() {
                    if let (Some(reach), Some(Some(row))) = (reach, row_bounds.get(n))
                        && !row.overlaps(&reach)
                    {
                        continue;
                    }
                    let t = &r.target;
                    if t.id == p.owner && launcher_immune(w) {
                        continue;
                    }
                    if gun && past_missed(t.id) {
                        continue;
                    }
                    // Easy aiming widens the volume of the aircraft it shoots at,
                    // never the shooter's own.
                    let hitbox = if p.owner == t.id { 1. } else { hitbox };
                    if (!gun || p.owner != t.id)
                        && (t.hp <= 0 || !spares(t.side))
                        && (t.hp <= 0 || p.guidance.as_ref().is_none_or(|f| f.eligible(w, t)))
                        && t.body_present()
                        && let Some(at) = if gun {
                            if let Some(v) = past(t.id) {
                                rewound_contact(p, v, hitbox)
                            } else {
                                let previous = std::array::from_fn(|i| {
                                    p.previous[i] + t.position[i] - r.previous[i]
                                });
                                LocalizedDamage::contact(previous, p.position, t, hitbox)
                                    .map(|v| v.0)
                            }
                        } else {
                            let radius = t.radius * hitbox + f64::from(w.damage.fuze_radius.max(0));
                            let start = sub(p.previous, r.previous);
                            // Launch grace: a round still inside its own launcher's
                            // volume (the aircraft sphere widened by the fuze)
                            // neither hits nor fuzes on it, nor does a separating
                            // drop-launched store.
                            if p.owner == t.id
                                && (separating || dot(start, start) <= radius * radius)
                            {
                                None
                            } else {
                                segment_sphere(start, sub(p.position, t.position), radius)
                            }
                        }
                        && at < fuze_until
                        && first.is_none_or(|f| at < f.0)
                    {
                        first = Some((at, Some(Hit::Ownship(n))));
                    }
                }
            }
            if armed {
                if rewind.is_none() {
                    target_broad.candidates(reach, &mut candidates);
                } else {
                    target_broad.candidates_rewound(segment, &mut candidates);
                }
                for &i in &candidates {
                    let t = &self.targets[i];
                    // A surface round strikes aircraft and the other side's
                    // ground objects and ships: never its own side's, neutral
                    // scenery or its own launcher, whatever the friendly fire
                    // setting.
                    if surface.is_some()
                        && t.role != TargetRole::Aircraft
                        && !opposed(owner_side, t.side)
                    {
                        continue;
                    }
                    if !(t.body_present()
                        && (!(gun || launcher_immune(w)) || t.id != p.owner)
                        && !(t.hp > 0 && t.role == TargetRole::Aircraft && spares(t.side)))
                    {
                        continue;
                    }
                    if t.hp > 0 && p.guidance.as_ref().is_some_and(|f| !f.eligible(w, t)) {
                        continue;
                    }
                    if gun
                        && (t.role == TargetRole::Aircraft || self.parked.contains_key(&t.id))
                        && !self.ground_bounds.contains_key(&t.id)
                        && past_missed(t.id)
                    {
                        continue;
                    }
                    let hitbox = if p.owner == t.id { 1. } else { hitbox };
                    let at = if let Some(bounds) = self.ground_bounds.get(&t.id) {
                        // Contact uses the reviewed/fitted solid box. Fuze blast
                        // radius remains a separate damage rule and does not turn
                        // a long runway into a giant interception sphere.
                        bounds.segment_fraction(p.previous, p.position)
                    } else if gun
                        && (t.role == TargetRole::Aircraft || self.parked.contains_key(&t.id))
                    {
                        // A parked aircraft has the same aircraft volume.
                        if let Some(v) = past(t.id) {
                            rewound_contact(p, v, hitbox)
                        } else {
                            let previous = std::array::from_fn(|axis| {
                                p.previous[axis] + t.position[axis] - old_targets[i][axis]
                            });
                            LocalizedDamage::contact(previous, p.position, t, hitbox).map(|v| v.0)
                        }
                    } else {
                        // A parked aircraft is met at its aircraft sphere: a bomb or
                        // missile's fuze radius does not reach it (fitted; nothing
                        // bursts beside a ground target).
                        let fuze = if self.parked.contains_key(&t.id) {
                            0
                        } else {
                            w.damage.fuze_radius.max(0)
                        };
                        let radius = t.radius * hitbox + f64::from(fuze);
                        let start = sub(p.previous, old_targets[i]);
                        // Launch grace: a round still inside its own launcher's
                        // volume (the aircraft sphere widened by the fuze)
                        // neither hits nor fuzes on it, nor does a separating
                        // drop-launched store.
                        if p.owner == t.id && (separating || dot(start, start) <= radius * radius) {
                            None
                        } else {
                            segment_sphere(start, sub(p.position, t.position), radius)
                        }
                    };
                    if let Some(at) = at
                        && at < fuze_until
                        && first.is_none_or(|f| at < f.0)
                    {
                        first = Some((at, Some(Hit::Target(i))));
                    }
                }
            }
            // Sample the whole segment, then bisect the first crossing; bounded
            // contact approximation prevents fast rounds tunneling through terrain.
            if let Some(at) = terrain_hit_below(p.previous, p.position, &ground, ceiling.as_deref())
                && first.is_none_or(|f| at < f.0)
            {
                first = Some((at, None));
            }
            if let Some(round) = surface {
                // A flak shell has no proximity fuze (John, 2026-10-13: its bursts
                // fall in an egg around the target, so the burst point is the time
                // fuze's alone). It bursts on contact with what it strikes, at the
                // end tick below, or at the end of its life.
                // The round's end tick: a flak shell's time fuze bursts it;
                // any other round has gone past its target and goes away.
                if first.is_none() && round.end_tick.is_some_and(|end| p.age >= end) {
                    if round.flak {
                        impacts.push((
                            p.position,
                            EffectKind::Flak,
                            flak_explosion(w),
                            None,
                            super::blast_scale::of(w),
                        ));
                        bursts.push(collateral::Burst::new(
                            p,
                            w,
                            shooter,
                            p.position,
                            collateral::Detonation::Air,
                            None,
                            true,
                        ));
                    } else {
                        self.ledger.resolve(p.id, Resolution::Missed);
                    }
                    return false;
                }
            }
            if let Some((at, target)) = first {
                let position =
                    std::array::from_fn(|i| p.previous[i] + (p.position[i] - p.previous[i]) * at);
                let wreck = match target {
                    Some(Hit::Ownship(n)) => Some(&rows[n].target),
                    Some(Hit::Target(i)) => Some(&self.targets[i]),
                    None => None,
                }
                .filter(|t| t.hp <= 0);
                if let Some(wreck) = wreck {
                    // Physical contact consumes the round without repeating the
                    // kill, damaging its pilot, or changing the last attacker.
                    self.ledger.resolve(p.id, Resolution::Hit(0));
                    events.push(Event::Hit(wreck.id));
                    // A destroyed runway still takes the weapon's crater (B16).
                    let crater = if self
                        .ground_looks
                        .get(&wreck.id)
                        .is_some_and(|look| look.pavement)
                        && !water(position[0], position[2])
                    {
                        super::blast_scale::crater_of(w).map(|(size, _)| size)
                    } else {
                        None
                    };
                    impacts.push((
                        position,
                        EffectKind::Hit,
                        w.effects.object_explosion,
                        crater,
                        super::blast_scale::of(w),
                    ));
                    if by_ownship {
                        scored.push((p.owner, false));
                    }
                    let burst = collateral::Burst::new(
                        p,
                        w,
                        shooter,
                        position,
                        if wreck.role == TargetRole::Aircraft {
                            collateral::Detonation::Air
                        } else {
                            collateral::Detonation::Ground
                        },
                        Some(wreck.id),
                        false,
                    );
                    if burst.collateral() {
                        bursts.push(burst);
                    }
                    return false;
                }
                if let Some(Hit::Ownship(n)) = target {
                    let r = &rows[n];
                    let own = &ships[r.index];
                    let deception = super::systems::deception_chance(
                        own.config.ecm,
                        w.seeker.signature,
                        r.launcher.jammer && !own.ecm_failed,
                    );
                    // A jammer defeats missiles, never a surface gun's rounds.
                    if deception != 0
                        && !surface_gun
                        && i32::from(draw(&mut self.rng, 100))
                            >= super::systems::hit_chance(100, deception)
                    {
                        self.ledger.resolve(p.id, Resolution::Jammed);
                        events.push(Event::Defeated(r.target.id));
                    } else {
                        let base = projectile_damage(
                            p,
                            w,
                            i32::from(w.damage.by_class[damage_class(own.config.target_category)]),
                        ) as u16;
                        let amount =
                            super::systems::damage_amount(base, 100, draw(&mut self.rng, 40) as u8);
                        self.ledger
                            .resolve(p.id, Resolution::Hit(u32::try_from(amount).unwrap_or(0)));
                        let (section, position) = if let Some(v) = past(r.target.id) {
                            rewound_section(p, v, &r.target, position)
                        } else {
                            let previous = std::array::from_fn(|i| {
                                p.previous[i] + r.target.position[i] - r.previous[i]
                            });
                            let section =
                                LocalizedDamage::section_segment(previous, p.position, &r.target);
                            (section, position)
                        };
                        ownship_hits.push((
                            n,
                            amount,
                            section,
                            is_aircraft_gun(w),
                            p.owner,
                            w.flags,
                            true,
                        ));
                        impacts.push((
                            position,
                            EffectKind::Hit,
                            w.effects.object_explosion,
                            None,
                            super::blast_scale::of(w),
                        ));
                        if !gun {
                            events.push(Event::Jolt(Jolt {
                                target: r.target.id,
                                from: position,
                                strength: f64::from(
                                    w.damage.by_class[damage_class(own.config.target_category)],
                                ) / 100.,
                                share: 1.,
                            }));
                        }
                        let burst = collateral::Burst::new(
                            p,
                            w,
                            shooter,
                            position,
                            collateral::Detonation::Air,
                            Some(r.target.id),
                            false,
                        );
                        if burst.collateral() {
                            bursts.push(burst);
                        }
                    }
                    return false;
                }
                if let Some(Hit::Target(i)) = target {
                    let t = &mut self.targets[i];
                    let deception = fixture_ecm.map_or(0, |ecm| {
                        super::systems::deception_chance(
                            ecm,
                            w.seeker.signature,
                            self.target_jammer
                                && !self.ground_bounds.contains_key(&t.id)
                                && !self.parked.contains_key(&t.id),
                        )
                    });
                    // A jammer defeats missiles, never a surface gun's rounds.
                    if deception != 0
                        && !surface_gun
                        && i32::from(draw(&mut self.rng, 100))
                            >= super::systems::hit_chance(100, deception)
                    {
                        self.ledger.resolve(p.id, Resolution::Jammed);
                        events.push(Event::Defeated(t.id));
                        return false;
                    }
                    let class = damage_class(t.category);
                    let nominal = i32::from(w.damage.by_class[class]).max(0);
                    // Only aircraft have a history; a ground object never rewinds.
                    let (section, position) = if let Some(v) = past(t.id) {
                        rewound_section(p, v, t, position)
                    } else {
                        let previous = std::array::from_fn(|axis| {
                            p.previous[axis] + t.position[axis] - old_targets[i][axis]
                        });
                        (
                            LocalizedDamage::section_segment(previous, p.position, t),
                            position,
                        )
                    };
                    let scaled = projectile_damage(p, w, nominal);
                    if t.role == TargetRole::Aircraft && !gun {
                        events.push(Event::Jolt(Jolt {
                            target: t.id,
                            from: position,
                            strength: f64::from(nominal) / 100.,
                            share: 1.,
                        }));
                    }
                    let critical = critical_hit(t, w, section, scaled);
                    let applied = if critical { t.hp } else { scaled.min(t.hp) };
                    t.hp -= applied;
                    // A parked aircraft takes damage by section like any
                    // aircraft (no system faults: nothing is running).
                    if t.role == TargetRole::Aircraft || self.parked.contains_key(&t.id) {
                        t.localized_damage.record(section, scaled, t.initial_hp);
                    }
                    if t.role == TargetRole::Aircraft && t.hp > 0 {
                        t.faults
                            .hit(scaled, t.initial_hp, |n| draw(&mut self.rng, n));
                    }
                    if self.history.len() == MAX_HIT_RECORDS {
                        self.history.remove(0);
                    }
                    self.history.push(HitRecord {
                        tick: self.tick,
                        target: t.id,
                        station: p.station,
                        class,
                        nominal,
                        applied,
                        hp_after: t.hp,
                    });
                    // Only an ownship's own rounds move its score. An AI
                    // aircraft killing another AI aircraft still raises the
                    // hit and destroyed events the host needs for damage,
                    // debris and effects.
                    if by_ownship {
                        scored.push((p.owner, false));
                    }
                    self.ledger
                        .resolve(p.id, Resolution::Hit(u32::try_from(applied).unwrap_or(0)));
                    let credit = Kill {
                        owner: p.owner,
                        victim: t.id,
                        category: t.category,
                        aircraft: t.role == TargetRole::Aircraft,
                    };
                    // Only a hit that did damage makes its shooter the last
                    // attacker, as for an ownship below.
                    if applied > 0 {
                        self.ledger.damaged(credit);
                    }
                    events.push(Event::Hit(t.id));
                    strikes.push(Strike {
                        owner: p.owner,
                        victim: t.id,
                        weapon_flags: w.flags,
                        destroyed: t.hp == 0,
                        amount: applied,
                    });
                    if t.hp == 0 {
                        if by_ownship {
                            scored.push((p.owner, true));
                        }
                        self.ledger.kill(credit);
                        events.push(Event::Destroyed(t.id));
                    }
                    // A weapon on pavement leaves its crater there, as on
                    // open ground (B16): a bombed runway shows its craters.
                    let crater = if self
                        .ground_looks
                        .get(&t.id)
                        .is_some_and(|look| look.pavement)
                        && !water(position[0], position[2])
                    {
                        super::blast_scale::crater_of(w).map(|(size, _)| size)
                    } else {
                        None
                    };
                    impacts.push((
                        position,
                        EffectKind::Hit,
                        w.effects.object_explosion,
                        crater,
                        super::blast_scale::of(w),
                    ));
                    if t.hp == 0 {
                        // A ground object explodes as its unit record says,
                        // leaving its crater on land.
                        let parked = self.parked.contains_key(&t.id);
                        let (explosion, crater) = if t.role == TargetRole::Aircraft || parked {
                            (super::blast::AIRCRAFT, 0)
                        } else if let Some(look) = self.ground_looks.get(&t.id) {
                            (
                                super::blast::ground_object(Some(look.explosion)),
                                if water(position[0], position[2]) {
                                    0
                                } else {
                                    look.crater
                                },
                            )
                        } else {
                            (super::blast::GROUND_OBJECT, 0)
                        };
                        impacts.push((
                            position,
                            EffectKind::Destroyed,
                            explosion,
                            (crater > 0).then_some(crater),
                            super::blast_scale::FULL,
                        ));
                    }
                    // A round on an aircraft bursts in the air; on a ground
                    // object or ship, at ground level.
                    let burst = collateral::Burst::new(
                        p,
                        w,
                        shooter,
                        position,
                        if t.role == TargetRole::Aircraft {
                            collateral::Detonation::Air
                        } else {
                            collateral::Detonation::Ground
                        },
                        Some(t.id),
                        false,
                    );
                    if burst.collateral() {
                        bursts.push(burst);
                    }
                } else if (super::ballistics::is_bomb(w) && !armed)
                    || (at >= fuze_until
                        && p.motion.and_then(|m| m.energy).is_some_and(|e| {
                            e.flight.defeated || e.decoy.pursuit.is_some_and(|p| p.passed)
                        }))
                {
                    // A bomb that reaches the ground before its fuze arms is a
                    // dud: no burst, no splash, no crater, only a puff of dirt
                    // where it lands (opinionated, lead, 2026-10-10;
                    // docs/spec/air-to-ground.md, "Warnings").
                    self.ledger.resolve(p.id, Resolution::Missed);
                    events.push(Event::Ground);
                    if !water(position[0], position[2]) {
                        impacts.push((
                            position,
                            EffectKind::Ground,
                            DUD_PUFF,
                            None,
                            super::blast_scale::INERT,
                        ));
                    }
                } else {
                    self.ledger.resolve(p.id, Resolution::Missed);
                    let burst = collateral::Burst::new(
                        p,
                        w,
                        shooter,
                        position,
                        collateral::Detonation::Ground,
                        None,
                        false,
                    );
                    if burst.collateral() {
                        bursts.push(burst);
                    }
                    events.push(Event::Ground);
                    if water(position[0], position[2]) {
                        impacts.push((
                            position,
                            EffectKind::Ground,
                            w.effects.water_explosion,
                            None,
                            super::blast_scale::of(w),
                        ));
                    } else {
                        impacts.push((
                            position,
                            EffectKind::Ground,
                            w.effects.land_explosion,
                            // The crater follows the explosion's scale; a
                            // gun round leaves a pock mark (John, 2026-10-12).
                            super::blast_scale::crater_of(w).map(|(size, _)| size),
                            super::blast_scale::of(w),
                        ));
                    }
                }
                return false;
            }
            true
        };
        self.ended_missile_threats.clear();
        self.projectiles.retain_mut(|p| {
            let owned = p.weapon.take();
            let kept = pass(p, &owned);
            p.weapon = owned;
            if !p.threatening() {
                self.ended_missile_threats.push(p.id);
            }
            kept
        });
        // A round that is gone takes its rewind, its surface record and its
        // bomb path with it.
        if !self.rewinds.is_empty()
            || !self.surface_rounds.is_empty()
            || !self.bombs.is_empty()
            || !self.guided.is_empty()
            || !self.falls.is_empty()
        {
            let flying: std::collections::BTreeSet<u32> =
                self.projectiles.iter().map(|p| p.id).collect();
            self.rewinds.retain(|id, _| flying.contains(id));
            self.surface_rounds.retain(|id, _| flying.contains(id));
            self.bombs.retain(|id, _| flying.contains(id));
            self.jettisoned.retain(|id, _| flying.contains(id));
            self.falls.retain(|id| flying.contains(&id));
            // A guided weapon that is gone marks its aircraft's last impact.
            let (tick, impacts) = (self.tick, &mut self.guided_impacts);
            self.guided.retain(|id, flight| {
                let keep = flying.contains(id);
                if !keep {
                    impacts.insert(flight.owner, tick);
                }
                keep
            });
        }
        self.ground_bursts.clear();
        for burst in &bursts {
            if burst.collateral() && burst.share < 100 {
                self.ground_bursts.push(GroundBurst {
                    position: burst.position,
                    radius: burst.radius,
                    percent: burst.percent * burst.share / 100,
                });
            }
        }
        for burst in &bursts {
            self.apply_burst(
                burst,
                &rows,
                friendly_fire_off,
                &water,
                collateral::Outputs {
                    events: &mut events,
                    strikes: &mut strikes,
                    ownship_hits: &mut ownship_hits,
                    impacts: &mut impacts,
                    scored: &mut scored,
                },
            );
        }
        for strike in strikes {
            self.strike(strike);
        }
        for (owner, kill) in scored {
            if let Some(own) = ships.iter_mut().find(|o| o.aircraft == owner) {
                if kill {
                    own.kills += 1;
                } else {
                    own.hits += 1;
                }
            }
        }
        // Invulnerable: hits still show their impact effect but do no damage.
        if self.cheats.invulnerable() {
            ownship_hits.clear();
        }
        for (n, amount, section, direct_gun, owner, weapon_flags, recorded) in ownship_hits {
            let own = &mut ships[rows[n].index];
            own.localized_damage
                .record(section, amount, own.config.damage_capacity);
            // Normal damage takes hit points only; the pilot-kill and
            // heavy core hits belong to Realistic.
            let lethal = direct_gun && self.cheats.system_damage();
            if lethal && section == DamageSection::Cockpit && own.hp > 0 {
                events.push(Event::PilotKilled {
                    aircraft: own.aircraft,
                });
            }
            let amount = if lethal
                && (section == DamageSection::Cockpit
                    || (section == DamageSection::Core && amount >= own.config.damage_capacity / 2))
            {
                own.hp
            } else {
                amount
            };
            let (alive, before) = (own.hp > 0, own.hp);
            self.damage_ownship(own, amount, &mut events);
            if alive && amount > 0 {
                // The debrief credits the shooter, human or AI, with the kill,
                // or, if the aircraft is lost another way, with the last hit on
                // it, as it does for a hit on an AI aircraft (John,
                // 2026-09-29). The diagnostic incoming round belongs to no
                // aircraft and is credited to nobody.
                if owner != INCOMING_OWNER {
                    let credit = Kill {
                        owner,
                        victim: own.aircraft,
                        category: own.config.target_category,
                        aircraft: true,
                    };
                    if recorded {
                        self.ledger.damaged(credit);
                    } else {
                        self.ledger.blasted(credit);
                    }
                    if own.hp == 0 {
                        self.ledger.kill(credit);
                    }
                }
                self.strike(Strike {
                    owner,
                    victim: own.aircraft,
                    weapon_flags,
                    destroyed: own.hp == 0,
                    amount: before - own.hp,
                });
            }
        }
        for (p, kind, explosion, crater, scale) in impacts {
            self.blast(p, kind, explosion, scale);
            if let Some(size) = crater {
                let mark_scale = if scale == super::blast_scale::INERT {
                    jettison::CRATER_SCALE
                } else {
                    scale
                };
                self.crater(p, size, mark_scale, super::blast::FOREVER);
            }
        }
        self.burn_wrecks();
        use super::smoke::Kind;
        // A damaged parked aircraft smokes like a damaged aircraft in the air
        // (docs/spec/surface-defenses.md, "Parked aircraft"); a destroyed one
        // burns at its crash site instead.
        for t in self.targets.iter().filter(|t| {
            (t.airborne || (t.hp > 0 && self.parked.contains_key(&t.id)))
                && t.damage_fraction() >= 0.5
        }) {
            sources.push((
                std::array::from_fn(|i| t.position[i] - t.basis.forward[i] * 15.),
                Kind::Aircraft,
            ));
        }
        for r in &rows {
            let (own, launcher) = (&ships[r.index], r.launcher);
            let falling = own.hp == 0
                && launcher.position[1] > ground(launcher.position[0], launcher.position[2]);
            if !own.explosion_reported
                && own.hp <= own.config.damage_capacity / 2
                && ((launcher.alive && own.hp > 0) || falling)
            {
                sources.push((
                    std::array::from_fn(|i| launcher.position[i] - launcher.basis.forward[i] * 15.),
                    Kind::Aircraft,
                ));
            }
        }
        // A burning crash site's column rises from just above its fire.
        sources.extend(
            self.marks
                .iter()
                .filter(|m| m.kind == super::blast::MarkKind::Fire)
                .map(|m| {
                    (
                        [
                            m.position[0],
                            m.position[1] + super::smoke::BURNING_SOURCE_FT,
                            m.position[2],
                        ],
                        Kind::Burning,
                    )
                }),
        );
        self.smoke.step(sources);
        self.devices.step(&ground);
        self.energy_devices.step(
            self.devices.wind.map(|v| v * missiles::energy::FT),
            &|x, z| {
                ground(x / missiles::energy::FT, z / missiles::energy::FT) * missiles::energy::FT
            },
        );
        let mut debris_impacts = Vec::new();
        self.debris.retain_mut(|piece| {
            if let Some(mut p) = piece.step(&ground) {
                p[1] += 6.;
                debris_impacts.push(p);
                false
            } else {
                true
            }
        });
        for p in debris_impacts {
            self.effect(p, EffectKind::DebrisImpact);
        }
        for t in &mut self.targets {
            if t.airborne
                && t.hp == 0
                && t.localized_damage.structural_section.is_some()
                && !t.fragment_released
            {
                t.fragment_released = true;
                let variant = t.aircraft.and_then(|aircraft| {
                    super::debris::damage_variant(
                        aircraft,
                        t.localized_damage.structural_section.unwrap() as usize,
                    )
                });
                if self.debris.len() < super::debris::MAX_PIECES
                    && let Some(variant) = variant
                {
                    self.debris.push(super::debris::Piece::new(
                        t.id,
                        variant,
                        t.position,
                        t.velocity,
                        t.basis,
                        t.fragment_offsets[variant],
                    ));
                }
            }
        }
        for r in &rows {
            let (own, launcher) = (&mut ships[r.index], r.launcher);
            if own.hp == 0
                && !own.explosion_reported
                && own.localized_damage.structural_section.is_some()
                && !own.fragment_released
            {
                own.fragment_released = true;
                let variant = super::debris::damage_variant(
                    own.config.aircraft,
                    own.localized_damage.structural_section.unwrap() as usize,
                );
                if self.debris.len() < super::debris::MAX_PIECES
                    && let Some(variant) = variant
                {
                    self.debris.push(super::debris::Piece::new(
                        own.aircraft,
                        variant,
                        launcher.position,
                        launcher.velocity,
                        launcher.basis,
                        own.config.fragment_offsets[variant],
                    ));
                }
            }
        }
        let launchers: Vec<(u32, Launcher)> =
            rows.iter().map(|r| (r.target.id, r.launcher)).collect();
        let snapshots = self.snapshots(&ships, &launchers);
        for r in &rows {
            let (own, launcher) = (&mut ships[r.index], r.launcher);
            let heading_deg = launcher.basis.forward[0]
                .atan2(launcher.basis.forward[2])
                .to_degrees();
            let pitch_deg = launcher.basis.forward[1].clamp(-1., 1.).asin().to_degrees();
            own.missile_threats.observe(
                self.tick,
                super::threats::Receiver {
                    id: own.aircraft,
                    position: launcher.position,
                    velocity: launcher.velocity,
                    heading_deg,
                    pitch_deg,
                    skill: crate::ai::Experience::Ace,
                    rwr_operating: launcher.alive && !own.rwr_failed,
                    visual_operating: launcher.alive && !own.visual_failed,
                    visibility_limit_ft: Some(5. * missiles::NMI),
                },
                &snapshots,
                |from, to| !obscured(from, to),
            );
        }
        if !self.cheats.ignore_midair_collisions {
            self.midair_collisions(&mut ships, &rows, &mut events);
        }
        self.ownships = ships;
        if self.weapon_rules == Rules::Energy && self.energy_predictions_enabled {
            self.advance_energy_predictions(&rows, &ground);
        }
        events
    }
    /// Aircraft whose paths came within their combined radii this tick
    /// collided. A midair collision destroys every aircraft involved,
    /// Invulnerable or not (John, 2026-09-23). No kill is credited.
    fn midair_collisions(
        &mut self,
        ships: &mut [Ownship],
        rows: &[OwnRow],
        events: &mut Vec<Event>,
    ) {
        // Targets first, then the ownships in aircraft id order.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        enum Body {
            Target(usize),
            Ownship(usize),
        }
        let mut bodies: Vec<(Body, Vector, Vector, f64)> = self
            .targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.role == TargetRole::Aircraft && t.airborne && t.hp > 0)
            .map(|(i, t)| (Body::Target(i), t.position, t.velocity, t.radius))
            .collect();
        for (n, r) in rows.iter().enumerate() {
            if r.launcher.alive && ships[r.index].hp > 0 {
                bodies.push((
                    Body::Ownship(n),
                    r.launcher.position,
                    r.launcher.velocity,
                    AIRCRAFT_RADIUS_FT,
                ));
            }
        }
        // Invulnerable spares the ownships; whatever they hit is still destroyed.
        let spared = self.cheats.invulnerable();
        let mut struck = std::collections::BTreeSet::new();
        let mut blasts = Vec::new();
        for (n, a) in bodies.iter().enumerate() {
            for b in &bodies[n + 1..] {
                let now = sub(a.1, b.1);
                let before = sub(now, sub(a.2, b.2).map(|v| v * crate::flight::DT));
                if segment_sphere(before, now, a.3 + b.3).is_some() {
                    struck.extend([a.0, b.0]);
                    blasts.push(std::array::from_fn(|i| (a.1[i] + b.1[i]) / 2.));
                }
            }
        }
        for body in struck {
            match body {
                Body::Ownship(_) if spared => continue,
                Body::Ownship(n) => {
                    let own = &mut ships[rows[n].index];
                    own.hp = 0;
                    own.damage = own.damage.max(own.config.damage_capacity);
                    own.release();
                    events.push(Event::OwnshipDestroyed {
                        aircraft: own.aircraft,
                    });
                }
                Body::Target(index) => {
                    let t = &mut self.targets[index];
                    t.hp = 0;
                    events.push(Event::Destroyed(t.id));
                }
            }
        }
        for blast in blasts {
            self.blast(
                blast,
                EffectKind::Destroyed,
                super::blast::AIRCRAFT,
                super::blast_scale::FULL,
            );
        }
    }
}
fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}

fn segment_box_fraction(from: Vector, to: Vector, lo: Vector, hi: Vector) -> Option<f64> {
    let mut enter: f64 = 0.;
    let mut exit: f64 = 1.;
    for axis in 0..3 {
        let delta = to[axis] - from[axis];
        if delta.abs() < 1e-12 {
            if from[axis] < lo[axis] || from[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let a = (lo[axis] - from[axis]) / delta;
        let b = (hi[axis] - from[axis]) / delta;
        enter = enter.max(a.min(b));
        exit = exit.min(a.max(b));
        if enter > exit {
            return None;
        }
    }
    Some(enter.max(0.))
}

/// A rewound gun round against an aircraft's volume from its history: the
/// round's segment in that volume's moving frame, as the current test does
/// with the current volume.
fn rewound_contact(p: &Projectile, v: &rewind::HitVolume, hitbox: f64) -> Option<f64> {
    let previous = std::array::from_fn(|i| p.previous[i] + v.position[i] - v.previous[i]);
    aircraft_contact(previous, p.position, v.position, v.basis, v.radius * hitbox).map(|v| v.0)
}
/// Where a rewound gun round hit: the section of the volume it was tested
/// against, and the impact moved from that past volume onto the aircraft as
/// it is now, so the hit shows on the aircraft every screen draws.
fn rewound_section(
    p: &Projectile,
    v: &rewind::HitVolume,
    target: &Target,
    impact: Vector,
) -> (DamageSection, Vector) {
    let past = Target {
        position: v.position,
        basis: v.basis,
        radius: v.radius,
        ..target.clone()
    };
    let previous = std::array::from_fn(|i| p.previous[i] + v.position[i] - v.previous[i]);
    let section = LocalizedDamage::section_segment(previous, p.position, &past);
    let impact = std::array::from_fn(|i| impact[i] + target.position[i] - v.position[i]);
    (section, impact)
}
/// John's one-third rule for the aircraft guns (2026-09-20). Surface guns
/// keep their record's damage: the AAA tuning table already matches retail
/// damage per second.
fn scaled_weapon_damage(w: &Weapon, damage: i32) -> i32 {
    let damage = damage.max(0);
    if is_aircraft_gun(w) {
        damage / 3
    } else {
        damage
    }
}

fn projectile_damage(p: &Projectile, w: &Weapon, damage: i32) -> i32 {
    let total = scaled_weapon_damage(w, damage);
    let Some(round) = p.gun_round else {
        return total;
    };
    let divisor = i32::from(w.burst.actual_rounds_per_game.max(1));
    total / divisor + i32::from(i32::from(round) < total.rem_euclid(divisor))
}

fn critical_hit(target: &Target, weapon: &Weapon, section: DamageSection, damage: i32) -> bool {
    target.role == TargetRole::Aircraft
        && is_aircraft_gun(weapon)
        && (section == DamageSection::Cockpit
            || (section == DamageSection::Core && damage >= target.initial_hp / 2))
}

/// Whether `p`, a round of `w`, has its motor burning at game time `now`
/// (quarter seconds): a missile by its age, a rocket by the game clock it
/// was launched on, as each flies.
pub fn motor_powered(w: &Weapon, p: &Projectile, now: u16) -> bool {
    if is_gun(w) {
        return false;
    }
    let m = &w.movement;
    let phase = if let Some(motion) = p.motion {
        motion.engine_phase(m, p.age)
    } else {
        engine_phase(m, now, p.launched_t)
    };
    phase == super::EnginePhase::Powered
}

/// The SUU-16 gun pod: an external store that fires a gun, hung under the
/// aircraft and drawn there, its hardware mass kept until it is removed
/// (the F-4J's default station, the F-4B's centreline option).
pub fn is_gun_pod(w: &Weapon) -> bool {
    w.source == "SUU16.JT"
}
/// Whether `w` is a gun: an aircraft's gun record or a surface gun of the AAA
/// tuning table. Gun rounds are hit-tested against the aircraft volume, spread
/// by the gun dispersion and never home.
pub fn is_gun(w: &Weapon) -> bool {
    GunRecords::get().contains(&w.source)
}
/// The gun records, aircraft and surface, upper-cased and sorted, so that
/// classifying a round is one binary search and no scan of about sixty names.
/// Record names compare without regard to ASCII case, as before.
struct GunRecords {
    names: Vec<Vec<u8>>,
    longest: usize,
}
impl GunRecords {
    fn get() -> &'static Self {
        static RECORDS: std::sync::OnceLock<GunRecords> = std::sync::OnceLock::new();
        RECORDS.get_or_init(|| {
            let mut names: Vec<Vec<u8>> = AircraftId::ALL
                .into_iter()
                .chain([AircraftId::Faxx])
                .flat_map(|aircraft| aircraft.guns().iter().copied())
                .chain(super::surface_guns::records())
                .map(|name| name.to_ascii_uppercase().into_bytes())
                .collect();
            names.sort_unstable();
            names.dedup();
            let longest = names.iter().map(Vec::len).max().unwrap_or(0);
            GunRecords { names, longest }
        })
    }
    fn contains(&self, source: &str) -> bool {
        let source = source.as_bytes();
        if source.len() > self.longest {
            return false;
        }
        let mut upper = [0u8; 48];
        let Some(slot) = upper.get_mut(..source.len()) else {
            // A table name this long is not expected; scan instead.
            return self
                .names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(source));
        };
        for (to, from) in slot.iter_mut().zip(source) {
            *to = from.to_ascii_uppercase();
        }
        self.names
            .binary_search_by(|name| name.as_slice().cmp(&*slot))
            .is_ok()
    }
}
/// The scan [`is_gun`] replaced, kept to test it against.
#[cfg(test)]
fn is_gun_scan(w: &Weapon) -> bool {
    is_aircraft_gun(w) || super::surface_guns::is_surface_gun(&w.source)
}
/// The station Tab fires: the first gun with rounds left, else the first gun,
/// else none. `rounds` is the aircraft's count at a station.
pub fn key_gun_station(config: &Configuration, rounds: impl Fn(usize) -> u16) -> Option<usize> {
    let guns = || {
        config
            .stations
            .iter()
            .enumerate()
            .filter(|(_, s)| is_gun(&s.weapon))
            .map(|(i, _)| i)
    };
    guns().find(|&i| rounds(i) > 0).or_else(|| guns().next())
}
/// An aircraft's gun record, which the one-third damage rule and the critical
/// (pilot and central) gun kills apply to.
pub fn is_aircraft_gun(w: &Weapon) -> bool {
    AircraftId::ALL
        .into_iter()
        .chain([AircraftId::Faxx])
        .any(|aircraft| {
            aircraft
                .guns()
                .iter()
                .any(|gun| w.source.eq_ignore_ascii_case(gun))
        })
}

const GUN_DISPERSION_HALF_ANGLE: f64 = 0.25_f64.to_radians();

pub fn projectile_launch_direction(
    weapon: &Weapon,
    direction: Vector,
    id: u32,
    owner: u32,
    station: usize,
) -> Vector {
    // A flak shell has no dispersion: it is aimed at its own burst point in
    // the egg around the target, and the egg is the whole of its scatter
    // (John, 2026-10-13: bursts within about 300 m by 100 m).
    if !is_gun(weapon) || surface::is_flak(weapon) {
        return direction;
    }
    let forward = unit(direction);
    let seed = id
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add(owner.rotate_left(13))
        .wrapping_add((station as u32).wrapping_mul(0x85eb_ca6b));
    let azimuth_u = f64::from(mix32(seed ^ 0xa511_e9b3)) / f64::from(u32::MAX);
    let radius_u = f64::from(mix32(seed ^ 0x63d8_3595)) / f64::from(u32::MAX);
    let azimuth = azimuth_u * std::f64::consts::TAU;
    let min_cos = GUN_DISPERSION_HALF_ANGLE.cos();
    let cos_theta = 1. - radius_u * (1. - min_cos);
    let sin_theta = (1. - cos_theta * cos_theta).max(0.).sqrt();
    let reference = if forward[1].abs() < 0.9 {
        [0., 1., 0.]
    } else {
        [1., 0., 0.]
    };
    let right = unit(cross(reference, forward));
    let up = unit(cross(forward, right));
    unit(std::array::from_fn(|i| {
        forward[i] * cos_theta
            + right[i] * sin_theta * azimuth.cos()
            + up[i] * sin_theta * azimuth.sin()
    }))
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}
fn acquisition(
    w: &Weapon,
    position: Vector,
    forward: Vector,
    target: Vector,
    radar: bool,
    zone: usize,
) -> bool {
    // PROJLock checks launcher illumination under flag 0x200. The reviewed
    // default radar stores all require radar at launch; only R530 has 0x200.
    if w.seeker.signature == 3 && !radar && (zone == 1 || w.flags & 0x200 != 0) {
        return false;
    }
    cone(
        &tore_formats::weapons::Zone {
            minimum_range: 0,
            ..w.seeker.zones[zone]
        },
        position,
        forward,
        target,
    )
}
fn cone(
    z: &tore_formats::weapons::Zone,
    position: Vector,
    forward: Vector,
    target: Vector,
) -> bool {
    zone_readiness(z, position, forward, target) == Readiness::Ready
}
fn zone_readiness(
    z: &tore_formats::weapons::Zone,
    position: Vector,
    forward: Vector,
    target: Vector,
) -> Readiness {
    let d = sub(target, position);
    let distance = dot(d, d).sqrt();
    let angle = dot(unit(d), forward).clamp(-1., 1.).acos();
    if distance < f64::from(z.minimum_range) {
        Readiness::MinimumRange
    } else if distance > f64::from(z.maximum_range) {
        Readiness::MaximumRange
    } else if d[1] < f64::from(z.minimum_altitude) || d[1] > f64::from(z.maximum_altitude) {
        Readiness::Altitude
    } else if angle > f64::from(z.heading.min(z.pitch).max(0)) * std::f64::consts::TAU / 65520. {
        Readiness::FieldOfView
    } else {
        Readiness::Ready
    }
}

pub fn segment_sphere(a: Vector, b: Vector, radius: f64) -> Option<f64> {
    let d = sub(b, a);
    let c = dot(a, a) - radius * radius;
    if c <= 0. {
        return Some(0.);
    }
    let aa = dot(d, d);
    let bb = dot(a, d);
    let disc = bb * bb - aa * c;
    if aa <= 1e-15 || disc < 0. {
        return None;
    }
    let t = (-bb - disc.sqrt()) / aa;
    (0. ..=1.).contains(&t).then_some(t)
}
/// [`terrain_hit`], answering `None` without sampling when `ceiling`
/// proves the whole segment above the ground. The ceiling must be built
/// from the same ground (`GroundCeiling` never clears a segment that
/// touches it), so the answer is always the one sampling gives.
pub fn terrain_hit_below(
    a: Vector,
    b: Vector,
    ground: &impl Fn(f64, f64) -> f64,
    ceiling: Option<&crate::ground_ceiling::GroundCeiling>,
) -> Option<f64> {
    if ceiling.is_some_and(|ceiling| ceiling.clear_above(a, b)) {
        return None;
    }
    terrain_hit(a, b, ground)
}

pub fn terrain_hit(a: Vector, b: Vector, ground: &impl Fn(f64, f64) -> f64) -> Option<f64> {
    let below = |t: f64| {
        let p: Vector = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        p[1] <= ground(p[0], p[2])
    };
    if below(0.) {
        return Some(0.);
    }
    for i in 1..=8 {
        let mut high = f64::from(i) / 8.;
        if below(high) {
            let mut low = f64::from(i - 1) / 8.;
            for _ in 0..10 {
                let mid = (low + high) * 0.5;
                if below(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            return Some(high);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWN: u32 = 0;
    use tore_formats::weapons::*;
    use tore_formats::weapons::{Guidance, Seeker};
    #[test]
    fn smaller_bombs_share_guidance_sound_and_allow_lighter_racks() {
        use crate::combat::{bomb_flight::Body, loadout::Loadout};
        use tore_formats::aircraft::Hardpoint;
        let config = fixture(true).own().configuration().clone();
        let mut load = Loadout {
            aircraft: AircraftId::F18,
            configuration: config,
            quantities: vec![0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 100.,
            maximum_lbs: 10000.,
            hardpoints: vec![Hardpoint {
                location: 4,
                flags: 0x108,
                position: [0; 3],
                store: Some("PAVEWAY.JT".into()),
                count: 4,
                weight_class: 20,
            }],
            tank_hardpoints: vec![],
            cheat: false,
        };
        for aircraft in AircraftId::ALL {
            load.aircraft = aircraft;
            for (source, donor, weight, _, shape) in tore_formats::weapons::corrections::SMALL_BOMBS
            {
                let mut w = load.configuration.stations[0].weapon.clone();
                w.source = source.into();
                w.shape = Some(shape.into());
                w.weight = weight;
                w.flags = if source == "GBU38.JT" {
                    0x2a043
                } else {
                    0x2a543
                };
                w.seeker.signature = if source == "GBU38.JT" { 0 } else { 1 };
                w.movement.fuel_t = 0;
                w.movement.initial_speed = 0;
                w.movement.ignite_t = 0;
                w.movement.launch_retard = 100;
                w.burst.projectiles_in_pod = 1;
                w.burst.game_rounds_in_burst = 1;
                load.hardpoints[0].store = Some(donor.into());
                assert_eq!(load.capacity(0, &w), 2000 / weight, "{aircraft:?} {source}");
                let body = Body::of(&w);
                assert_eq!(body.mass, f64::from(weight) / 32.);
                assert_eq!(body.steering, if source == "GBU38.JT" { 2 } else { 1 });
                assert_eq!(crate::combat::release_sound(&w), Some("&BOMB.11K"));
                assert!(body.area < 0.1323 * 4_f64.powf(2. / 3.) * 1.3);
                let mut larger = w.clone();
                larger.source = donor.into();
                larger.weight = 2000;
                assert_eq!(body.steering, Body::of(&larger).steering);
                assert_eq!(
                    crate::combat::release_sound(&w),
                    crate::combat::release_sound(&larger)
                );
                assert_eq!(
                    crate::combat::blast_scale::of(&w),
                    if weight == 500 { 100 } else { 126 }
                );
                assert!(crate::combat::store_sizes::StoreSize::of_weapon(&w).length < 14.2);
                load.cheat = true;
                assert!(load.capacity(0, &w) > 0);
                load.cheat = false;
            }
        }
    }

    #[test]
    fn r27_loadout_family_and_rail_rules() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        let config = fixture(true).own().configuration().clone();
        let mut load = Loadout {
            aircraft: AircraftId::Su27,
            configuration: config,
            quantities: vec![0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 100.,
            maximum_lbs: 10000.,
            hardpoints: vec![Hardpoint {
                location: 4,
                flags: 0xc0,
                position: [0; 3],
                store: None,
                count: 1,
                weight_class: 0,
            }],
            tank_hardpoints: vec![],
            cheat: false,
        };
        for aircraft in [
            AircraftId::Su27,
            AircraftId::Su35,
            AircraftId::Mig29,
            AircraftId::Yak141,
            AircraftId::F18,
        ] {
            load.aircraft = aircraft;
            load.configuration.hardpoint_slots = vec![None; 6];
            load.configuration.hardpoint_slots[5] = Some(0);
            for source in ["AA10.JT", "R27ER.JT", "R27T.JT", "R27ET.JT"] {
                let mut w = load.configuration.stations[0].weapon.clone();
                w.source = source.into();
                w.flags = if matches!(source, "R27T.JT" | "R27ET.JT") {
                    0x1204f
                } else {
                    0x1234f
                };
                w.seeker.signature = if w.flags & 0x200 == 0 { 2 } else { 3 };
                let expected = match aircraft {
                    AircraftId::Su27 | AircraftId::Su35 | AircraftId::Mig29 => 1,
                    AircraftId::Yak141 if matches!(source, "AA10.JT" | "R27T.JT") => 1,
                    _ => 0,
                };
                assert_eq!(load.capacity(0, &w), expected, "{aircraft:?} {source}");
                if aircraft == AircraftId::Mig29 {
                    load.configuration.hardpoint_slots[5] = None;
                    load.configuration.hardpoint_slots[4] = Some(0);
                    assert_eq!(
                        load.capacity(0, &w),
                        0,
                        "only the inner MiG-29 pair takes R-27"
                    );
                    load.configuration.hardpoint_slots[4] = None;
                    load.configuration.hardpoint_slots[5] = Some(0);
                }
                load.hardpoints[0].flags = 0x80;
                assert_eq!(
                    load.capacity(0, &w),
                    0,
                    "short IR rail cannot take {source}"
                );
                load.hardpoints[0].flags = 0xc0;
            }
        }
    }

    #[test]
    fn optional_bomb_and_dual_rail_rows_obey_type_count_and_weight_limits() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        for (id, row, flags, count, weight) in [
            (AircraftId::F15, 6, 0x3c1, 3, 45),
            (AircraftId::F22, 8, 0x2a5, 4, 90),
            (AircraftId::F22n, 8, 0x2a5, 4, 90),
        ] {
            let mut config = fixture(false).own().configuration().clone();
            let mut weapon = config.stations[0].weapon.clone();
            config.stations.truncate(1);
            config.hardpoint_slots = vec![None; row + 1];
            config.hardpoint_slots[row] = Some(0);
            config.tanks.clear();
            let mut load = Loadout {
                aircraft: id,
                configuration: config,
                quantities: vec![0],
                fuel_lbs: 0.,
                internal_capacity_lbs: 5000.,
                empty_lbs: 10000.,
                maximum_lbs: 50000.,
                hardpoints: vec![Hardpoint {
                    location: if row == 6 { 0 } else { 4 },
                    flags,
                    position: [0; 3],
                    store: None,
                    count,
                    weight_class: weight,
                }],
                tank_hardpoints: vec![],
                cheat: false,
            };
            weapon.source = "MK82.JT".into();
            weapon.flags = 0x12;
            weapon.seeker.signature = 0;
            weapon.burst.projectiles_in_pod = 1;
            weapon.weight = 500;
            assert_eq!(load.capacity(0, &weapon), if row == 6 { 3 } else { 0 });
            weapon.weight = 2000;
            assert_eq!(load.capacity(0, &weapon), if row == 6 { 2 } else { 0 });
            weapon.weight = 4501;
            assert_eq!(load.capacity(0, &weapon), 0);
            weapon.source = "AIM120.JT".into();
            weapon.flags = 2;
            weapon.seeker.signature = 3;
            weapon.weight = 350;
            assert_eq!(load.capacity(0, &weapon), if row == 8 { 4 } else { 0 });
            weapon.source = "AGM65G.JT".into();
            assert_eq!(load.capacity(0, &weapon), 0);
            load.cheat = true;
            assert_eq!(load.capacity(0, &weapon), count);
        }
    }

    #[test]
    fn the_eagles_wing_pylons_take_a_tank_or_air_to_air_missiles_one_a_side() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::{Hardpoint, stations};
        // The appended row as the parser builds it: a tank, infrared or
        // active radar, 90 class, two (one a pylon).
        let mut rows = [
            (1, 8, [0, 0, 0], "VIS340.SEE", 1, 0),
            (1, 8, [0, 0, 0], "F15R.SEE", 1, 0),
            (1, 8, [0, 0, -50], "F15.ECM", 1, 0),
            (1, 8, [5, -5, -5], "M61.JT", 940, 0),
            (1, 0x7e5, [25, -10, 0], "AIM120.JT", 4, 60),
            (4, 0x7e5, [50, -20, 0], "AIM9M.JT", 4, 14),
        ]
        .map(
            |(location, flags, position, store, count, weight_class)| Hardpoint {
                location,
                flags,
                position,
                store: Some(store.into()),
                count,
                weight_class,
            },
        )
        .to_vec();
        stations::append(AircraftId::F15, &mut rows);
        let pylons = rows[7].clone();
        assert_eq!((pylons.flags, pylons.count), (0x2a5, 2));
        let mut config = fixture(false).own().configuration().clone();
        let mut weapon = config.stations[0].weapon.clone();
        config.stations.truncate(1);
        config.hardpoint_slots = vec![None; 8];
        config.hardpoint_slots[7] = Some(0);
        config.tanks.clear();
        let mut load = Loadout {
            aircraft: AircraftId::F15,
            configuration: config,
            quantities: vec![0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 5000.,
            empty_lbs: 10000.,
            maximum_lbs: 50000.,
            hardpoints: vec![pylons],
            tank_hardpoints: vec![],
            cheat: false,
        };
        let set = |w: &mut Weapon, source: &str, flags, signature, weight| {
            w.source = source.into();
            w.flags = flags;
            w.seeker.signature = signature;
            w.weight = weight;
            w.burst.projectiles_in_pod = 1;
        };
        // An AIM-120 or a Sidewinder of any mark, one a side.
        set(&mut weapon, "AIM120.JT", 0x1204f, 3, 345);
        assert_eq!(load.capacity(0, &weapon), 2);
        for source in ["AIM9B.JT", "AIM9M.JT", "AIM9X.JT"] {
            set(&mut weapon, source, 0x1204f, 2, 190);
            assert_eq!(load.capacity(0, &weapon), 2, "{source}");
        }
        // Not the Sparrow, the Archer, bombs or the Maverick.
        set(&mut weapon, "AIM7.JT", 0x1234f, 3, 505);
        assert_eq!(load.capacity(0, &weapon), 0);
        set(&mut weapon, "AA11.JT", 0x1204f, 2, 230);
        assert_eq!(load.capacity(0, &weapon), 0);
        set(&mut weapon, "MK82.JT", 0x22012, 0, 500);
        assert_eq!(load.capacity(0, &weapon), 0);
        set(&mut weapon, "AGM65G.JT", 0x1204f, 1, 675);
        assert_eq!(load.capacity(0, &weapon), 0);
        // The same row on another aircraft keeps the plain mask rule (the
        // Sparrow's 0x40 class is not in the mask, nor are bombs).
        set(&mut weapon, "AIM120.JT", 0x1204f, 3, 345);
        load.aircraft = AircraftId::F18;
        assert_eq!(load.capacity(0, &weapon), 2);
        set(&mut weapon, "AA11.JT", 0x1204f, 2, 230);
        assert_eq!(load.capacity(0, &weapon), 2);
    }

    #[test]
    fn the_f16c_wingtip_pair_takes_the_amraam_and_nothing_else_changes() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        let tip = Hardpoint {
            location: 5,
            flags: 0x85,
            position: [54, 2, -14],
            store: None,
            count: 2,
            weight_class: 4,
        };
        let mut config = fixture(false).own().configuration().clone();
        let mut weapon = config.stations[0].weapon.clone();
        config.stations.truncate(1);
        config.hardpoint_slots = vec![None; 10];
        config.hardpoint_slots[8] = Some(0);
        config.tanks.clear();
        let mut load = Loadout {
            aircraft: AircraftId::F16C,
            configuration: config,
            quantities: vec![0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 5000.,
            empty_lbs: 10000.,
            maximum_lbs: 50000.,
            hardpoints: vec![tip],
            tank_hardpoints: vec![],
            cheat: false,
        };
        let set = |w: &mut Weapon, source: &str, flags, signature, weight| {
            w.source = source.into();
            w.flags = flags;
            w.seeker.signature = signature;
            w.weight = weight;
            w.burst.projectiles_in_pod = 1;
        };
        // The AIM-120 (a radar missile of 345 lb) gets the row's own count,
        // one a tip, although the infrared-only mask and 400 lb class refuse it.
        set(&mut weapon, "AIM120.JT", 0x1204f, 3, 345);
        assert_eq!(load.capacity(0, &weapon), 2);
        // Every other store keeps the source row's rule: the Sidewinder two,
        // the heavier Archer one, radar and bomb stores none.
        set(&mut weapon, "AIM9M.JT", 0x1204f, 2, 190);
        assert_eq!(load.capacity(0, &weapon), 2);
        set(&mut weapon, "AA11.JT", 0x1204f, 2, 230);
        assert_eq!(load.capacity(0, &weapon), 1);
        set(&mut weapon, "AIM7.JT", 0x1234f, 3, 505);
        assert_eq!(load.capacity(0, &weapon), 0);
        set(&mut weapon, "MK82.JT", 0x22012, 0, 500);
        assert_eq!(load.capacity(0, &weapon), 0);
        // Only the reviewed, appended F-16C row 8: an F-16C record without
        // the appended row (nine rows), another row, or another aircraft
        // gets the plain mask rule.
        set(&mut weapon, "AIM120.JT", 0x1204f, 3, 345);
        load.configuration.hardpoint_slots.truncate(9);
        assert_eq!(load.capacity(0, &weapon), 0);
        load.configuration.hardpoint_slots.push(None);
        load.configuration.hardpoint_slots.swap(7, 8);
        assert_eq!(load.capacity(0, &weapon), 0);
        load.configuration.hardpoint_slots.swap(7, 8);
        load.aircraft = AircraftId::F18;
        assert_eq!(load.capacity(0, &weapon), 0);
        load.aircraft = AircraftId::F16C;
        assert_eq!(load.capacity(0, &weapon), 2);
        // Cheat loading is unchanged: the row's raw capacity.
        load.cheat = true;
        assert_eq!(load.capacity(0, &weapon), 2);
        set(&mut weapon, "AIM54C.JT", 0x1206f, 3, 975);
        assert_eq!(load.capacity(0, &weapon), 2);
    }

    #[test]
    fn derived_wet_rows_enforce_capacity_and_feed_without_changing_fuel_layout() {
        use crate::combat::{loading, loadout::Loadout};
        use tore_formats::aircraft::{Hardpoint, stations};
        let mut resources = std::collections::BTreeMap::from([
            ("F15.PT".to_owned(), Vec::new()),
            ("F16C.PT".to_owned(), Vec::new()),
            ("F22.PT".to_owned(), Vec::new()),
            ("MIG29.PT".to_owned(), Vec::new()),
            ("RAFALE.PT".to_owned(), Vec::new()),
        ]);
        stations::extend_resources(&mut resources);
        for (id, row, name, count, limit) in [
            (AircraftId::F15, 6, "F610.GAS", 1, 45),
            (AircraftId::F15, 7, "F610.GAS", 2, 90),
            (AircraftId::F16C, 9, "F370.GAS", 2, 60),
            (AircraftId::F22, 8, "F600.GAS", 2, 90),
            (AircraftId::F22n, 8, "F600.GAS", 2, 90),
            (AircraftId::Mig29, 8, "M396.GAS", 1, 30),
            (AircraftId::Rafale, 9, "R528.GAS", 1, 40),
        ] {
            let tank = TankStore::parse(name, &resources[name]).unwrap();
            let h = Hardpoint {
                location: if count == 1 { 0 } else { 4 },
                flags: 0x205,
                position: [0; 3],
                store: None,
                count: match (id, row) {
                    (AircraftId::F15, 6) => 3,
                    (AircraftId::F22 | AircraftId::F22n, 8) => 4,
                    _ => count,
                },
                weight_class: limit,
            };
            assert_eq!(
                loading::Station::from_source(&h)
                    .unwrap()
                    .allowed_count(loading::Store::tank(tank.tank), false),
                count
            );
            let mut config = fixture(false).own().configuration().clone();
            config.stations.clear();
            config.hardpoint_slots = vec![None; row + 1];
            config.tanks = vec![TankStation {
                hardpoint: row,
                mount: [if count == 1 { 0. } else { 7. }, -5., -1.],
                rack: crate::combat::hardpoints::Rack::from_source(&h),
                store: None,
                quantity: 0,
            }];
            config.refresh_tanks().unwrap();
            let mut load = Loadout {
                aircraft: id,
                configuration: config,
                quantities: vec![],
                fuel_lbs: 5000.,
                internal_capacity_lbs: 5000.,
                empty_lbs: 10000.,
                maximum_lbs: 50000.,
                hardpoints: vec![],
                tank_hardpoints: vec![h],
                cheat: false,
            };
            let mut small = tank.clone();
            small.source = "SYNTHETIC.GAS".into();
            small.tank.fuel_weight = 100;
            small.tank.empty_weight = 10;
            assert_eq!(
                load.tank_capacity(0, &small),
                count,
                "one tank per physical pylon"
            );
            load.select_tank(0, tank.clone()).unwrap();
            load.validate().unwrap();
            let pounds = f64::from(count * tank.tank.fuel_weight);
            assert_eq!(
                load.configuration.external_fuel_lbs[stations::fuel_bank(row).unwrap()],
                pounds
            );
            let mut fuel = crate::aircraft_systems::Fuel::new(load.configuration.external_fuel_lbs);
            let mut internal = load.fuel_lbs;
            fuel.consume(&mut internal, pounds + 5.);
            assert_eq!(internal, 4995.);
            load.configuration.tanks[0].quantity += 1;
            load.refresh_equipment().unwrap();
            assert!(load.validate().is_err());
            load.aircraft = AircraftId::F18;
            assert_eq!(load.tank_capacity(0, &tank), 0);
        }
    }

    #[test]
    fn external_tank_quantity_accounts_for_shell_fuel_and_empty_equipment() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        let tank = TankStore {
            source: "SYNTHETIC.GAS".into(),
            name: "Synthetic tank".into(),
            tank: tore_formats::weapons::Tank {
                empty_weight: 198,
                fuel_weight: 1650,
                flags: 1,
            },
        };
        let hardpoint = Hardpoint {
            location: 4,
            flags: 0x200,
            position: [0; 3],
            store: Some(tank.source.clone()),
            count: 2,
            weight_class: 0,
        };
        let mut config = fixture(false).own().configuration().clone();
        config.stations.clear();
        config.hardpoint_slots = vec![None; 6];
        config.tanks = vec![TankStation {
            hardpoint: 5,
            mount: [0.; 3],
            rack: Default::default(),
            store: Some(tank.clone()),
            quantity: 2,
        }];
        config.refresh_tanks().unwrap();
        let mut load = Loadout {
            aircraft: AircraftId::F14,
            configuration: config,
            quantities: vec![],
            fuel_lbs: 15741.,
            internal_capacity_lbs: 15741.,
            empty_lbs: 10000.,
            maximum_lbs: 50000.,
            hardpoints: vec![],
            tank_hardpoints: vec![hardpoint],
            cheat: false,
        };
        load.validate().unwrap();
        assert_eq!(load.external_fuel_lbs(), 3300.);
        assert_eq!(load.tank_shell_lbs(), 396.);
        assert_eq!(load.total_lbs(), 10000. + 15741. + 3696.);
        let accepted = load.clone();
        load.clear_tanks().unwrap();
        load.validate().unwrap();
        assert_eq!(load.external_fuel_lbs(), 0.);
        assert_eq!(load.tank_shell_lbs(), 0.);
        assert_eq!(load.fuel_lbs, 15741.);
        assert_eq!(load.total_lbs(), 10000. + 15741.);
        assert_eq!(accepted.external_fuel_lbs(), 3300.);
        load.select_tank(0, tank).unwrap();
        let mut fuel = crate::aircraft_systems::Fuel::new(load.configuration.external_fuel_lbs);
        let mut internal = load.fuel_lbs;
        fuel.consume(&mut internal, 3300.);
        assert_eq!(fuel.external_lbs(), 0.);
        assert_eq!(internal, 15741.);
        assert_eq!(
            f64::from(load.configuration.external_equipment_lbs) - fuel.used_lbs(),
            396.
        );
        assert_eq!(load.configuration.tanks[0].quantity, 2);
        load.configuration.tanks[0].quantity = 3;
        load.configuration.refresh_tanks().unwrap();
        assert!(load.validate().is_err());
    }

    #[test]
    fn gun_pod_has_separate_installed_units_ammunition_and_retained_hardware() {
        let mut config = fixture(false).own().configuration().clone();
        config.aircraft = AircraftId::F4J;
        config.stations[0].weapon.source = "SUU16.JT".into();
        config.stations[0].weapon.weight = 0;
        config.stations[0].count = 1;
        config.stations[0].internal = false;
        config.gun_pods = vec![GunPod {
            station: 0,
            quantity: 1,
            rounds_per_pod: 600,
            weight_lbs: 1702,
        }];
        config.refresh_tanks().unwrap();
        assert_eq!(config.ammunition(&[1]).unwrap(), [600]);
        assert!(config.ammunition(&[60]).is_err());
        let mut state = State::new(config, true).unwrap();
        assert_eq!(state.own().ammo, [600]);
        assert_eq!(state.own().payload_lbs(), 1702.);
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                |_, _| -10000.,
            );
        }
        assert!(state.own().ammo[0] > 0 && state.own().ammo[0] < 599);
        assert!(state.projectiles.len() > 1);
        state.own_mut().ammo[0] = 0;
        state.own_mut().note_loaded();
        assert_eq!(state.own().payload_lbs(), 1702.);
        state.command(0, Command::Jettison, launcher());
        assert_eq!(state.own().payload_lbs(), 0.);
    }

    #[test]
    fn a_gun_pod_row_is_a_pod_only_while_the_pod_is_its_weapon_and_fires_from_the_nose() {
        let mut config = fixture(false).own().configuration().clone();
        config.stations[0].weapon.source = "AIM9B.JT".into();
        config.stations[0].count = 4;
        config.stations[0].internal = false;
        config.stations[0].mount = [0., -6.33, 0.];
        config.stations[0].rack = crate::combat::hardpoints::Rack {
            location: crate::combat::hardpoints::Location::Centerline.byte(),
            ..crate::combat::hardpoints::Rack::default()
        };
        config.gun_pods = vec![GunPod {
            station: 0,
            quantity: 0,
            rounds_per_pod: 600,
            weight_lbs: 1702,
        }];
        // An empty entry on a row carrying missiles is no pod, and a gun
        // mount is the record's own.
        assert!(config.gun_pod(0).is_none());
        assert_eq!(config.stations[0].gun_mount(), config.stations[0].mount);
        config.stations[0].weapon.source = "SUU16.JT".into();
        config.stations[0].count = 1;
        config.gun_pods[0].quantity = 1;
        assert_eq!(config.gun_pod(0).map(|pod| pod.quantity), Some(1));
        // The pod's rounds start at its nose: half the pod's length ahead
        // of where its middle hangs, the same point the sight solves from.
        let station = &config.stations[0];
        let middle = station.release_point(0);
        let muzzle = station.muzzle();
        assert_eq!(station.gun_mount(), muzzle);
        assert!(
            (muzzle[2] - middle[2] - crate::combat::store_sizes::SUU16_LENGTH / 2.).abs() < 1e-9
        );
        assert_eq!((muzzle[0], muzzle[1]), (middle[0], middle[1]));
        assert_eq!(
            crate::combat::store_sizes::StoreSize::of_weapon(&station.weapon).length,
            crate::combat::store_sizes::SUU16_LENGTH
        );
    }

    #[test]
    fn dormant_source_station_starts_empty_and_plus_restores_capacity() {
        use crate::combat::loadout::{EditableStation, Loadout};
        use tore_formats::aircraft::Hardpoint;
        let mut config = fixture(false).own().configuration().clone();
        config.stations[0].weapon.source = "MK82.JT".into();
        config.stations[0].weapon.flags |= 2;
        config.stations[0].internal = false;
        config.stations[0].count = 0;
        config.hardpoint_slots = vec![None, None, Some(0)];
        let state = State::new(config.clone(), true).unwrap();
        assert_eq!(state.own().ammo, [0]);
        assert_eq!(state.own().payload_lbs(), 0.);
        let mut load = Loadout {
            aircraft: AircraftId::F18,
            configuration: config,
            quantities: vec![0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 100.,
            maximum_lbs: 1000.,
            hardpoints: vec![Hardpoint {
                location: 4,
                flags: 0x100,
                position: [0; 3],
                store: None,
                count: 4,
                weight_class: 0,
            }],
            tank_hardpoints: vec![],
            cheat: false,
        };
        assert_eq!(
            load.editable_stations(),
            [EditableStation {
                hardpoint: 2,
                location: 4,
                weapon: Some(0),
                tank: None
            }]
        );
        load.change(0, 1);
        assert_eq!(load.quantities, [1]);
        assert_eq!(load.configuration.stations[0].count, 4);
        load.validate().unwrap();
    }

    #[test]
    fn tank_transfer_is_one_unit_atomic_and_unloading_overweight_drafts_is_allowed() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        let store = TankStore {
            source: "SYNTHETIC.GAS".into(),
            name: "Synthetic tank".into(),
            tank: tore_formats::weapons::Tank {
                empty_weight: 100,
                fuel_weight: 200,
                flags: 1,
            },
        };
        let h = Hardpoint {
            location: 4,
            flags: 0x200,
            position: [0; 3],
            store: Some(store.source.clone()),
            count: 2,
            weight_class: 0,
        };
        let mut config = fixture(false).own().configuration().clone();
        config.stations.clear();
        config.hardpoint_slots = vec![None; 3];
        config.tanks = vec![
            TankStation {
                hardpoint: 1,
                mount: [0.; 3],
                rack: Default::default(),
                store: Some(store.clone()),
                quantity: 2,
            },
            TankStation {
                hardpoint: 2,
                mount: [0.; 3],
                rack: Default::default(),
                store: None,
                quantity: 0,
            },
        ];
        config.refresh_tanks().unwrap();
        let mut load = Loadout {
            aircraft: AircraftId::F14,
            configuration: config,
            quantities: vec![],
            fuel_lbs: 0.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 100.,
            maximum_lbs: 200.,
            hardpoints: vec![],
            tank_hardpoints: vec![h.clone(), h],
            cheat: false,
        };
        assert!(load.validate().is_err());
        load.transfer_tank(0, 1).unwrap();
        assert_eq!(
            load.configuration
                .tanks
                .iter()
                .map(|s| s.quantity)
                .collect::<Vec<_>>(),
            [1, 1]
        );
        load.unload_tank(0).unwrap();
        assert_eq!(load.configuration.tanks[0].quantity, 0);
        assert!(load.validate().is_err());
        load.tank_hardpoints[0].flags = 0;
        load.tank_hardpoints[0].store = None;
        let mass = load.total_lbs();
        assert!(load.transfer_tank(1, 0).is_err());
        assert_eq!(load.configuration.tanks[1].quantity, 1);
        assert_eq!(load.configuration.tanks[0].quantity, 0);
        assert_eq!(load.total_lbs(), mass);
    }

    #[test]
    fn variety_guided_defaults_use_reviewed_families_and_at2_stays_unguided() {
        let mut weapon = fixture(true).own().configuration().stations[0]
            .weapon
            .clone();
        for (source, guidance, role, removal) in [
            (
                "AA10.JT",
                missiles::Guidance::Supported,
                TargetRole::Aircraft,
                424,
            ),
            (
                "AIM7.JT",
                missiles::Guidance::Supported,
                TargetRole::Aircraft,
                424,
            ),
            (
                "AIM7E.JT",
                missiles::Guidance::Supported,
                TargetRole::Aircraft,
                240,
            ),
            (
                "AIM9B.JT",
                missiles::Guidance::Infrared,
                TargetRole::Aircraft,
                80,
            ),
            (
                "AGM88.JT",
                missiles::Guidance::Emitter,
                TargetRole::Surface,
                160,
            ),
        ] {
            weapon.source = source.into();
            weapon.flags = if guidance == missiles::Guidance::Supported {
                0x241
            } else {
                0x41
            };
            weapon.seeker.signature = match guidance {
                missiles::Guidance::Infrared => 2,
                missiles::Guidance::Emitter => 4,
                _ => 3,
            };
            weapon.movement.remove_t = removal;
            let profile = missiles::Profile::for_weapon(&weapon).unwrap();
            profile.validate().unwrap();
            assert_eq!(profile.guidance, guidance);
            assert_eq!(profile.role, role);
            assert_eq!(profile.guidance_ticks, u64::from(removal) * 30);
            assert_eq!(profile.memory_ticks, 240);
            assert_eq!(profile.activation_ft, None);
            assert!(!profile.jammer_emissions);
        }
        weapon.source = "AT2.JT".into();
        weapon.seeker.signature = 0;
        assert!(missiles::Profile::for_weapon(&weapon).is_none());
        assert!(super::super::loadout::supported(&weapon.source));
    }

    #[test]
    fn unarmed_aircraft_keeps_nav_and_absent_systems_through_commands_and_ticks() {
        let mut config = fixture(false).own().configuration().clone();
        config.aircraft = AircraftId::C130;
        config.stations.clear();
        config.hardpoint_slots = vec![None];
        config.radar_hardpoint = None;
        config.ecm_hardpoint = None;
        config.ecm = Countermeasures::NONE;
        config.sensors.radar = None;
        config.sensors.infrared = None;
        config.sensors.jammer = None;
        let mut state = State::new(config, false).unwrap();
        let launcher = launcher();
        assert!(!state.own().armed);
        for command in [
            Command::NextWeapon,
            Command::NextSelection,
            Command::PreviousSelection,
            Command::ToggleArm,
            Command::ToggleSeekerMode,
            Command::Jettison,
            Command::FailStation,
            Command::ReleaseChaff,
            Command::ReleaseFlare,
            Command::Incoming,
            Command::DamagePlayer,
        ] {
            state.command(0, command, launcher);
        }
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher,
                }],
                |_, _| -10000.,
            );
            let view = state.view(0).unwrap();
            assert_eq!(view.readiness(launcher), Readiness::Safe);
            assert!(!view.can_lock(launcher));
            assert_eq!(view.seeker_tone(launcher), None);
            assert!(view.mounted_solution(launcher).is_none());
        }
        assert!(state.projectiles.is_empty());
        assert!(!state.own().armed);
        assert_eq!(state.own().chaff, 0);
        assert_eq!(state.own().flares, 0);
    }

    pub(super) fn fixture(guided: bool) -> State {
        let zone = Zone {
            heading: 12000,
            pitch: 12000,
            minimum_range: 0,
            maximum_range: 10000,
            minimum_altitude: i32::MIN,
            maximum_altitude: i32::MAX,
        };
        let seeker = Seeker {
            flags: [0; 2],
            signature: if guided { 3 } else { 0 },
            look_down: 0,
            doppler_above: 0,
            doppler_below: 0,
            doppler_minimum_range: 0,
            all_aspect: 0,
            zones: [zone; 2],
            chaff_flare_chance: 0,
            deception_chance: 0,
        };
        let w = Weapon {
            source: "SYNTHETIC.JT".into(),
            name: "Synthetic".into(),
            hud_name: "SYN".into(),
            shape: None,
            fire_sound: None,
            native_callback: "_PROJProc".into(),
            flags: if guided { 0x240 } else { 0x844 },
            object_flags: 0,
            weight: 10,
            movement: Movement {
                minimum_speed: 10,
                corner_speed: 1000,
                maximum_speed: 2000,
                acceleration: 100,
                deceleration: 2,
                initial_speed: 1000,
                final_speed: 500,
                launch_retard: 100,
                ignite_t: 0,
                fuel_t: 10,
                remove_t: 20,
                powered_turn_rate: 10000,
                unpowered_turn_rate: 10000,
                performance_at_0: 100,
                performance_at_20: 100,
                cruise: [0; 4],
                jink: [0; 3],
            },
            burst: Burst {
                projectiles_in_pod: 1,
                actual_rounds_per_game: 2,
                game_rounds_in_burst: 1,
                game_rounds_in_carpet_burst: 1,
                game_burst_t: 1,
                reload_t: 0,
                startup_shots: 0,
                random_fire_percent: 0,
                offset_fire_percent: 0,
                offset_fire_heading: 0,
                offset_fire_pitch: 0,
                sine_pattern: [0; 4],
            },
            seeker,
            guidance: Guidance {
                track_t: 1,
                track_max_g_raw: 1,
                target_sun_chance: 0,
                max_aon: 0,
                chances: [100; 4],
                hit_modifiers: [0; 9],
            },
            damage: Damage {
                by_class: [10; 5],
                fuze_arm_t: 0,
                fuze_radius: 0,
                side_hit_fuze_failure: 0,
                collateral_radius: 0,
                collateral_percent: 0,
            },
            effects: Effects {
                object_explosion: 0,
                land_explosion: 0,
                water_explosion: 0,
                crater_size: 0,
                smoke: [0; 5],
                max_sound_distance: 0,
                frequency_adjustment: 0,
            },
        };
        State::new(
            Configuration {
                fragment_offsets: [[0.; 3]; 2],
                ecm: tore_formats::weapons::Countermeasures {
                    weight: 0,
                    flags: 0,
                    mode_flags: 0x10,
                    chaff: [0; 4],
                    flare: [0; 4],
                    radar_deception_chance: 30,
                    radar_signature_add: 0,
                    radar_noise_range: [0; 2],
                    infrared_deception_chance: 0,
                    infrared_signature_add: 0,
                    infrared_lose_lock_time: 0,
                },
                system_damage: [0x11; 45],
                damage_capacity: 30,
                afterburner_available: true,
                hardpoint_slots: vec![Some(0)],
                radar_hardpoint: Some(1),
                visual_hardpoint: 3,
                ecm_hardpoint: Some(2),
                aircraft: AircraftId::F18,
                strike_class: StrikeClass::Fighter,
                stations: vec![Station {
                    weapon: w,
                    mount: [0.; 3],
                    count: 11,
                    internal: !guided,
                    rack: crate::combat::hardpoints::Rack {
                        // Released from the record's own point (a bay).
                        location: 3,
                        ..Default::default()
                    },
                }],
                hit_points: 20,
                target_category: 0x80,
                fixed_external_equipment_lbs: 0,
                tanks: vec![],
                gun_pods: vec![],
                external_equipment_lbs: 0,
                external_fuel_lbs: [0.; 9],
                engines: 1,
                wreck_power: crate::wreck::Power::default(),
                infrared_hardpoint: None,
                rwr_hardpoint: None,
                sensors: sensor_profiles(),
                sight: None,
                designator_pods: vec![],
            },
            true,
        )
        .unwrap()
    }

    #[test]
    fn player_wreck_keeps_smoking_until_impact_or_airburst_then_puffs_fade() {
        use super::super::smoke::Kind;
        let mut s = fixture(false);
        s.own_mut().hp = 0;
        let mut l = launcher();
        l.alive = false;
        for tick in 0..120 {
            l.position[1] = 5000. - f64::from(tick);
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.smoke.puffs.len(), 10);
        assert!(s.smoke.puffs.iter().all(|p| p.kind == Kind::Aircraft));
        let mut airburst = s.clone();
        airburst.ownship_airburst(0, l.position);
        s.ownship_ground_impact(0, [l.position[0], 0., l.position[2]], false);
        for _ in 0..120 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            airburst.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        let wreck_puffs = |state: &State| {
            state
                .smoke
                .puffs
                .iter()
                .filter(|p| p.kind == Kind::Aircraft)
                .count()
        };
        for state in [&s, &airburst] {
            assert_eq!(wreck_puffs(state), 10);
            assert!(
                state
                    .smoke
                    .puffs
                    .iter()
                    .filter(|p| p.kind == Kind::Aircraft)
                    .all(|p| p.age >= 120)
            );
        }
        // Only the crash on the ground leaves a burning site behind.
        assert!(s.smoke.puffs.iter().any(|p| p.kind == Kind::Burning));
        assert!(airburst.smoke.puffs.iter().all(|p| p.kind != Kind::Burning));
        for _ in 0..960 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(wreck_puffs(&s), 0);
        let mut grounded = fixture(false);
        grounded.own_mut().hp = 0;
        l.position[1] = 0.;
        for _ in 0..24 {
            grounded.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        assert!(grounded.smoke.puffs.is_empty());
    }

    #[test]
    fn sound_emissions_retain_exact_impact_positions_and_are_consumed_once() {
        use crate::acoustics::Kind;
        let mut s = fixture(false);
        s.blast(
            [100., 200., 300.],
            EffectKind::Ground,
            17,
            super::super::blast_scale::FULL,
        );
        s.blast(
            [-100., 400., 900.],
            EffectKind::Destroyed,
            34,
            super::super::blast_scale::FULL,
        );
        s.blast(
            [100., 200., 300.],
            EffectKind::Ground,
            17,
            super::super::blast_scale::FULL,
        );
        // Launch flashes and debris landing are silent.
        s.effect([0.; 3], EffectKind::Launch);
        let sounds = s.take_sound_events();
        assert_eq!(sounds.len(), 3);
        assert_eq!(sounds[0].position, [100., 200., 300.]);
        assert_eq!(sounds[0].kind, Kind::Blast(17));
        assert_eq!(sounds[1].position, [-100., 400., 900.]);
        assert_eq!(sounds[1].kind, Kind::Blast(34));
        assert_eq!(s.effects[1].blast, Some(34));
        assert_eq!(s.effects[1].ticks, 240);
        assert!(s.take_sound_events().is_empty());
        // Clearing short-lived visuals cannot erase an already emitted wave.
        s.effects.clear();
        assert_eq!(sounds.len(), 3);
        for _ in 0..1000 {
            s.blast(
                [0.; 3],
                EffectKind::Hit,
                18,
                super::super::blast_scale::FULL,
            );
        }
        assert_eq!(s.take_sound_events().len(), 256);
    }
    #[test]
    fn fixed_barrel_lead_flies_into_moving_targets_and_never_hits_its_shooter() {
        use crate::ai::gunnery;
        for velocity in [[0., 0., -200.], [0., 0., 200.], [250., 0., 0.]] {
            for incoming in [false, true] {
                for aimed in [false, true] {
                    let mut state = fixture(false);
                    let mut gun = super::super::gunsight::tests::weapon();
                    gun.source = "M61.JT".into();
                    state.own_mut().config.stations[0].weapon = gun.clone();
                    let mut own = launcher();
                    own.radar = false;
                    let observed = gunnery::Target {
                        id: 99,
                        position: [0., 1000., 800.],
                        velocity,
                        basis: Basis::new(0., 0., 0.),
                    };
                    let aim = gunnery::solve(&gun, &own, [0.; 3], observed).unwrap();
                    let d = aim.aim.direction;
                    own.basis = Basis::new(
                        d[0].atan2(d[2]) + if aimed { 0. } else { 0.2 },
                        d[1].asin(),
                        0.,
                    );
                    assert_eq!(
                        gunnery::solve(&gun, &own, [0.; 3], observed)
                            .unwrap()
                            .aligned,
                        aimed
                    );
                    let mut victim = target(99, observed.position, 1000, 0x80);
                    victim.radius = AIRCRAFT_RADIUS_FT;
                    victim.velocity = velocity;
                    state.targets = vec![target(7, own.position, 1000, 0x80), victim];
                    let player = Launcher {
                        position: [10000., 1000., 0.],
                        ..own
                    };
                    state.command(0, Command::Incoming, player);
                    let p = &mut state.projectiles[0];
                    p.owner = 7;
                    p.incoming = incoming.then_some(0);
                    p.position = own.position;
                    p.previous = own.position;
                    p.direction = own.basis.forward;
                    // Isolate nominal ballistics. Live launches at age zero
                    // also receive dispersion, already covered separately.
                    p.age = 1;
                    for _ in 0..240 {
                        state.step(
                            &[OwnshipInput {
                                aircraft: 0,
                                held: false,
                                launcher: player,
                            }],
                            |_, _| 0.,
                        );
                    }
                    assert_eq!(state.targets[0].hp, 1000, "shooter, {velocity:?}");
                    assert_eq!(
                        state.targets[1].hp < 1000,
                        aimed,
                        "{velocity:?}, incoming={incoming}"
                    );
                }
            }
        }
    }

    #[test]
    fn ai_gun_can_hit_player_even_when_aim_metadata_names_another_aircraft() {
        let mut state = fixture(false);
        state.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        let own = launcher();
        state.command(0, Command::Incoming, own);
        let p = &mut state.projectiles[0];
        p.owner = 7;
        p.incoming = None;
        p.position = [0., 1000., 100.];
        p.previous = p.position;
        let before = state.own().hp;
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: own,
                }],
                |_, _| 0.,
            );
        }
        assert!(state.own().hp < before);
    }

    #[test]
    fn an_ai_shooter_is_credited_with_an_ownship_it_shoots_down() {
        let fly = |owner: u32| {
            let mut state = fixture(false);
            state.own_mut().config.stations[0].weapon.source = "M61.JT".into();
            state.own_mut().hp = 1;
            let own = launcher();
            state.command(0, Command::Incoming, own);
            let p = &mut state.projectiles[0];
            p.owner = owner;
            p.incoming = None;
            p.position = [0., 1000., 100.];
            p.previous = p.position;
            for _ in 0..120 {
                state.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: own,
                    }],
                    |_, _| 0.,
                );
            }
            assert_eq!(state.own().hp, 0);
            state.ledger.kills().to_vec()
        };
        let kills = fly(7);
        assert_eq!(kills.len(), 1, "{kills:?}");
        assert_eq!((kills[0].owner, kills[0].victim), (7, 0));
        assert!(kills[0].aircraft);
        // The diagnostic incoming round belongs to no aircraft.
        assert!(fly(INCOMING_OWNER).is_empty());
    }

    #[test]
    fn a_crash_on_land_burns_for_15_minutes_once_and_the_sea_leaves_nothing() {
        use super::super::blast::{self, MarkKind};
        use super::super::smoke::Kind;
        let mut s = fixture(false);
        s.aircraft_crashed(9, [100., 0., 200.], false);
        s.aircraft_crashed(9, [100., 0., 200.], false);
        s.aircraft_crashed(10, [900., 0., 200.], true);
        let kinds: Vec<_> = s.marks.iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            [MarkKind::Crater(blast::CRASH_CRATER), MarkKind::Fire]
        );
        assert!(s.marks.iter().all(|m| m.ticks == blast::CRASH_TICKS));
        // Two crash explosions: one on land, one in the water.
        let explosions: Vec<_> = s.effects.iter().filter_map(|e| e.blast).collect();
        assert_eq!(explosions.len(), 2);
        assert!([35, 36, 37].contains(&explosions[0]));
        assert_eq!(explosions[1], blast::CRASH_WATER);
        let l = launcher();
        for _ in 0..600 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        let column: Vec<_> = s
            .smoke
            .puffs
            .iter()
            .filter(|p| p.kind == Kind::Burning)
            .collect();
        assert_eq!(column.len(), 50);
        assert!(column.iter().all(|p| p.position[1] >= 20.));
        for _ in 600..blast::CRASH_TICKS {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        assert!(s.marks.is_empty());
    }
    #[test]
    fn weapon_craters_and_pock_marks_stay_for_the_mission_without_a_cap() {
        use super::super::blast::{FOREVER, MarkKind, POCK};
        let mut s = fixture(false);
        for i in 0..1_000 {
            s.crater([i as f64, 0., 0.], 9, 159, FOREVER);
            s.crater([i as f64, 0., 10.], POCK, 50, FOREVER);
        }
        assert_eq!(s.marks.len(), 2_000);
        for _ in 0..1000 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.marks.len(), 2_000);
        assert!(
            s.marks
                .iter()
                .all(|m| matches!(m.kind, MarkKind::Crater(9 | POCK)))
        );
        assert_eq!(s.marks[0].scale, 159);
        assert_eq!(s.marks[1].scale, 50);
    }

    /// A gun round fired into the player's cockpit, stepped until it lands
    /// or the player is destroyed.
    fn cockpit_gun_hit(damage: crate::cheats::Damage) -> (State, Vec<Event>) {
        let mut s = fixture(false);
        s.cheats.damage = damage;
        // Synthetic gun data uses the exact reviewed gun identity for contact classification.
        s.own_mut().config.stations[0].weapon.source = AircraftId::F18.gun().unwrap().into();
        let l = launcher();
        s.command(0, Command::Incoming, l);
        let p = s.projectiles.last_mut().unwrap();
        p.position = std::array::from_fn(|i| {
            l.position[i] + l.basis.forward[i] * 100. + l.basis.up[i] * 11.
        });
        p.previous = p.position;
        p.age = 1;
        let mut events = Vec::new();
        for _ in 0..120 {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            ));
            if s.own().hp == 0 {
                break;
            }
        }
        (s, events)
    }
    #[test]
    fn incoming_cockpit_hit_reports_pilot_death_without_needing_nose_breakup() {
        let (s, events) = cockpit_gun_hit(crate::cheats::Damage::Realistic);
        assert!(events.contains(&Event::PilotKilled { aircraft: 0 }));
        assert!(events.contains(&Event::OwnshipDestroyed { aircraft: 0 }));
        assert_eq!(s.own().damage_section(), None);
    }
    #[test]
    fn normal_damage_cockpit_hit_takes_hit_points_without_killing_the_pilot() {
        let (s, events) = cockpit_gun_hit(crate::cheats::Damage::Normal);
        assert!(!events.contains(&Event::PilotKilled { aircraft: 0 }));
        assert!(!events.contains(&Event::SubsystemDamaged {
            aircraft: 0,
            index: 26
        }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { .. }))
        );
        assert!(s.own().hp < s.own().config.damage_capacity);
    }
    #[test]
    fn player_impact_explosion_cleans_up_once_and_excludes_later_airbursts() {
        let mut s = fixture(false);
        s.debris.push(super::super::debris::Piece::new(
            0,
            0,
            [0., 1., 0.],
            [0.; 3],
            Basis::new(0., 0., 0.),
            [0.; 3],
        ));
        assert_eq!(
            s.ownship_ground_impact(0, [0., 0., 0.], false),
            Some(Event::OwnshipGroundImpact { aircraft: 0 })
        );
        assert!(s.debris.is_empty());
        assert_eq!(
            s.effects
                .iter()
                .filter(|e| e.kind == EffectKind::Destroyed)
                .count(),
            1
        );
        assert_eq!(s.ownship_ground_impact(0, [0., 0., 0.], false), None);
        assert_eq!(s.ownship_airburst(0, [0., 0., 0.]), None);
    }
    #[test]
    fn wreck_airburst_removes_target_and_fragments_without_awarding_another_kill() {
        let mut s = fixture(false);
        let mut aircraft = target(77, [0., 100000., 2000.], 100, 0x80);
        aircraft.hp = 0;
        aircraft.wreck_power = crate::wreck::Power::symmetric(2, 20., 10.);
        s.targets.push(aircraft);
        // Search deterministic destruction ticks rather than depending on global combat RNG.
        let born = (0..1000)
            .find(|born| {
                let mut w = crate::wreck::Wreck::new(77, *born, [0.; 3]);
                let (mut p, mut v, mut b) = ([0., 100000., 0.], [0.; 3], Basis::new(0., 0., 0.));
                for _ in 0..120 {
                    w.step(&mut p, &mut v, &mut b, |_, _| 0.);
                }
                w.phase == crate::wreck::Phase::Exploded
            })
            .unwrap();
        s.tick = born.saturating_sub(1);
        s.targets[0].wreck = Some(crate::wreck::Wreck::new(77, born, [0.; 3]));
        s.debris.push(super::super::debris::Piece::new(
            77,
            0,
            s.targets[0].position,
            [0.; 3],
            s.targets[0].basis,
            [0.; 3],
        ));
        let mut bursts = 0;
        let mut exploded = false;
        for _ in 0..240 {
            bursts += s
                .step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: launcher(),
                    }],
                    |_, _| 0.,
                )
                .iter()
                .filter(|e| **e == Event::Airburst(77))
                .count();
            exploded |= s.effects.iter().any(|e| e.kind == EffectKind::Destroyed);
        }
        assert_eq!(bursts, 1);
        assert!(!s.targets[0].airborne);
        assert_eq!(s.own().kills, 0);
        assert!(s.debris.iter().all(|p| p.owner != 77));
        assert!(exploded);
        // An airburst leaves no crash site.
        assert!(s.marks.is_empty());
        assert_eq!(
            s.ownship_airburst(0, [0., 5000., 0.]),
            Some(Event::Airburst(0))
        );
        assert_eq!(s.ownship_airburst(0, [0., 5000., 0.]), None);
    }
    #[test]
    fn heavy_enemy_hit_degrades_components_without_detaching_a_live_nose() {
        let mut s = fixture(false);
        s.cheats.damage = crate::cheats::Damage::Realistic;
        s.own_mut().config.damage_capacity = 100;
        s.own_mut().hp = 100;
        s.own_mut().config.system_damage = [0; 45];
        for index in [19, 5, 14, 12] {
            s.own_mut().config.system_damage[index] = 0x11;
        }
        let mut events = Vec::new();
        // A real incoming projectile sweeps the ownship nose. The zero seed
        // fixes the damage draw at its lower boundary for this synthetic test.
        let raw = if is_gun(&s.own().config.stations[0].weapon) {
            345
        } else {
            115
        };
        s.own_mut().config.stations[0].weapon.damage.by_class = [raw; 5];
        s.rng = 0;
        s.command(0, Command::Incoming, launcher());
        for _ in 0..720 {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            ));
            if s.own().hp < 100 {
                break;
            }
        }
        assert_eq!(s.own().hp, 8);
        let mut components = crate::aircraft_systems::Systems::default();
        for event in events {
            if let Event::SubsystemDamaged { index, .. } = event {
                components.hit(index, 1.);
            }
        }
        for index in [19, 5, 14, 12] {
            assert_eq!(s.own().subsystem_counts[index], 1);
        }
        assert_eq!(components.power_available(), 0.75);
        assert_eq!(components.oil_pressure(), 0.5);
        assert_eq!(components.controls([1., 0., 0.], [0.; 3], 0)[0], 0.5);
        for _ in 0..120 {
            components.advance(true, 1., 1., 0.92, false, &mut 100.);
        }
        assert!(components.fluids.hydraulic < 1.);
        assert!(components.engine.temperature > 0.);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert!(s.debris.is_empty());
        s.damage_own(7, &mut Vec::new());
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().hp, 1);
        assert!(s.debris.is_empty());
        s.damage_own(1, &mut Vec::new());
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().hp, 0);
        assert_eq!(s.debris.len(), 1);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert_eq!(s.debris.len(), 1);
    }
    #[test]
    fn normal_damage_takes_hit_points_without_system_faults() {
        let run = |damage| {
            let mut s = fixture(false);
            s.cheats.damage = damage;
            s.own_mut().config.system_damage = [0x1f; 45];
            s.own_mut().config.damage_capacity = 200;
            s.own_mut().hp = 200;
            let mut events = Vec::new();
            for _ in 0..30 {
                s.damage_own(4, &mut events);
            }
            assert_eq!(s.own().hp, 80);
            s
        };
        let normal = run(crate::cheats::Damage::Normal);
        assert_eq!(normal.own().subsystem_counts, [0; 45]);
        assert_eq!(normal.own().last_subsystem, None);
        let realistic = run(crate::cheats::Damage::Realistic);
        assert!(realistic.own().subsystem_counts.iter().any(|n| *n > 0));
    }
    #[test]
    fn rwr_damage_is_a_receiver_fault_and_systems_destruction_is_once_only() {
        let mut s = fixture(false);
        s.cheats.damage = crate::cheats::Damage::Realistic;
        s.own_mut().config.rwr_hardpoint = Some(4);
        s.own_mut().config.system_damage = [0; 45];
        s.own_mut().config.system_damage[40] = 0x1f;
        s.own_mut().hp = 10000;
        let mut events = Vec::new();
        for _ in 0..30 {
            s.damage_own(4, &mut events);
        }
        assert_eq!(s.own().subsystem_counts[40], 1);
        assert!(s.own().rwr_failed);
        assert!(!s.own().radar_failed);
        let reset = State::new(s.own().config.clone(), true).unwrap();
        assert!(!reset.own().rwr_failed);
        assert_eq!(
            s.systems_destroyed(0),
            Some(Event::OwnshipDestroyed { aircraft: 0 })
        );
        assert_eq!(s.systems_destroyed(0), None);
        assert_eq!(s.own().hp, 0);
    }
    #[test]
    fn player_selection_wraps_through_nav_and_arms_only_weapons() {
        let initial = fixture(false);
        let mut config = initial.own().configuration().clone();
        config.stations.push(config.stations[0].clone());
        let mut state = State::new(config, true).unwrap();
        let l = launcher();
        state.command(0, Command::SelectNav, l);
        assert!(!state.own().armed);
        assert!(
            !state
                .step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l
                    }],
                    |_, _| 0.
                )
                .iter()
                .any(|e| matches!(e, Event::Fired { .. }))
        );
        for (command, selected, armed) in [
            (Command::NextSelection, 0, true),
            (Command::NextSelection, 1, true),
            (Command::NextSelection, 1, false),
            (Command::PreviousSelection, 1, true),
            (Command::PreviousSelection, 0, true),
            (Command::PreviousSelection, 0, false),
        ] {
            state.command(0, command, l);
            assert_eq!((state.own().selected, state.own().armed), (selected, armed));
        }
    }

    #[test]
    fn localized_sections_follow_aircraft_basis_and_accumulate_before_breakup() {
        let mut target = target(1, [10., 20., 30.], 100, 0x80);
        target.basis = Basis::new(std::f64::consts::FRAC_PI_2, 0., 0.);
        let nose = std::array::from_fn(|i| target.position[i] + target.basis.forward[i] * 20.);
        let left = std::array::from_fn(|i| target.position[i] - target.basis.right[i] * 20.);
        let tail = std::array::from_fn(|i| target.position[i] - target.basis.forward[i] * 20.);
        assert_eq!(LocalizedDamage::section(nose, &target), DamageSection::Nose);
        assert_eq!(
            LocalizedDamage::section(left, &target),
            DamageSection::LeftWing
        );
        assert_eq!(LocalizedDamage::section(tail, &target), DamageSection::Tail);
        let cockpit_center: Vector = std::array::from_fn(|i| {
            target.position[i] + target.basis.up[i] * 6. + target.basis.forward[i] * 7.
        });
        let cockpit_from = std::array::from_fn(|i| cockpit_center[i] + target.basis.right[i] * 10.);
        let cockpit_to = std::array::from_fn(|i| cockpit_center[i] - target.basis.right[i] * 10.);
        assert_eq!(
            LocalizedDamage::section_segment(cockpit_from, cockpit_to, &target),
            DamageSection::Cockpit
        );
        let movement = [40., -12., 25.];
        let old_projectile: Vector = std::array::from_fn(|i| cockpit_from[i] - movement[i]);
        let relative_previous: Vector = std::array::from_fn(|i| old_projectile[i] + movement[i]);
        assert_eq!(
            LocalizedDamage::contact(relative_previous, cockpit_to, &target, 1.).map(|v| v.1),
            Some(DamageSection::Cockpit)
        );
        let core_from = std::array::from_fn(|i| target.position[i] - target.basis.right[i] * 5.);
        let core_to = std::array::from_fn(|i| target.position[i] + target.basis.right[i] * 5.);
        assert_eq!(
            LocalizedDamage::section_segment(core_from, core_to, &target),
            DamageSection::Core
        );
        let first_from: Vector =
            std::array::from_fn(|i| cockpit_center[i] + target.basis.right[i] * 20.);
        let first_to: Vector =
            std::array::from_fn(|i| cockpit_center[i] + target.basis.right[i] * 10.);
        let second_to: Vector =
            std::array::from_fn(|i| cockpit_center[i] - target.basis.right[i] * 10.);
        assert_eq!(
            LocalizedDamage::contact(first_from, first_to, &target, 1.),
            None
        );
        assert_eq!(
            LocalizedDamage::contact(first_to, second_to, &target, 1.).map(|v| v.1),
            Some(DamageSection::Cockpit)
        );
        target.localized_damage.record(DamageSection::Nose, 74, 100);
        assert_eq!(target.localized_damage.structural_variant, None);
        target.localized_damage.record(DamageSection::Nose, 1, 100);
        assert_eq!(target.localized_damage.structural_variant, Some(0));
        assert_eq!(
            target.localized_damage.structural_section,
            Some(DamageSection::Nose)
        );
        assert_eq!(target.localized_damage.fractions(100)[0], 0.75);
    }

    #[test]
    fn a_dropped_target_leaves_the_hud_unless_easy_targeting_keeps_it() {
        let mut state = fixture(true);
        let ownship = launcher();
        state.range_target(0, ownship);
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: ownship,
                }],
                |_, _| 0.,
            );
        }
        state.designate_next(0, true);
        let id = state.own_view().designated().expect("fixture contact");
        state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .position = [0., 1000., -5000.];
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: ownship,
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(state.own_view().designated(), None);
        assert!(state.own_view().display_target().is_none());
        // Turning Easy targeting on later does not bring it back.
        state.cheats.easy_targeting = true;
        state.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: ownship,
            }],
            |_, _| 0.,
        );
        assert!(state.own_view().display_target().is_none());
    }
    #[test]
    fn views_keep_a_dropped_target_only_within_visual_range() {
        let mut state = fixture(true);
        let ownship = launcher();
        let blind = Launcher {
            radar: false,
            radar_power: false,
            ..ownship
        };
        let select = |state: &mut State| {
            for _ in 0..120 {
                state.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: ownship,
                    }],
                    |_, _| 0.,
                );
            }
            state.designate_next(0, true);
            state.own_view().designated().expect("fixture contact")
        };
        let run = |state: &mut State, launcher: Launcher| {
            for _ in 0..120 {
                state.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher,
                    }],
                    |_, _| 0.,
                );
            }
        };
        state.range_target(0, ownship);
        let id = select(&mut state);
        // Radar off: the selection and HUD square drop, the views keep it.
        run(&mut state, blind);
        assert_eq!(state.own_view().designated(), None);
        assert!(state.own_view().display_target().is_none());
        assert!(state.own_view().weapon_observation(blind).is_none());
        assert_eq!(state.own_view().view_target().map(|t| t.id), Some(id));
        state.command(0, Command::ClearDesignation, blind);
        assert!(state.own_view().view_target().is_none());
        // Behind the pilot but inside visual range it stays; past the 10 nmi
        // range it is gone, and coming back does not restore it.
        assert_eq!(select(&mut state), id);
        run(&mut state, blind);
        let place = |state: &mut State, position| {
            state
                .targets
                .iter_mut()
                .find(|t| t.id == id)
                .unwrap()
                .position = position;
            run(state, blind);
        };
        place(&mut state, [0., 1000., -5000.]);
        assert_eq!(state.own_view().view_target().map(|t| t.id), Some(id));
        place(&mut state, [0., 1000., -70_000.]);
        assert!(state.own_view().view_target().is_none());
        place(&mut state, [0., 1000., 3000.]);
        assert!(state.own_view().view_target().is_none());
    }
    #[test]
    fn easy_targeting_keeps_the_selection_off_scope_without_weapon_support() {
        let mut state = fixture(true);
        state.cheats.easy_targeting = true;
        let ownship = launcher();
        state.range_target(0, ownship);
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: ownship,
                }],
                |_, _| 0.,
            );
        }
        state.designate_next(0, true);
        let id = state.own_view().designated().expect("fixture contact");
        assert_eq!(state.own_view().display_target().map(|t| t.id), Some(id));
        state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .position = [0., 1000., -5000.];
        for _ in 0..120 {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: ownship,
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(
            state.own_view().designated(),
            Some(id),
            "radar keeps it set"
        );
        assert!(state.own().sensors.contact(id).is_none());
        assert!(state.own_view().weapon_observation(ownship).is_none());
        assert_eq!(state.own_view().display_target().map(|t| t.id), Some(id));
        state.targets.iter_mut().find(|t| t.id == id).unwrap().hp = 0;
        assert_eq!(state.own_view().display_target().map(|t| t.id), Some(id));
        state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .airborne = false;
        assert!(state.own_view().display_target().is_none());
        state.targets.iter_mut().find(|t| t.id == id).unwrap().hp = 10;
        state.command(0, Command::ClearDesignation, ownship);
        assert!(state.own_view().display_target().is_none());
    }

    #[test]
    fn guns_take_exact_integer_third_while_missiles_keep_damage() {
        let mut gun = fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        gun.source = "M61.JT".into();
        assert_eq!(scaled_weapon_damage(&gun, 11), 3);
        assert_eq!(scaled_weapon_damage(&gun, 2), 0);
        let missile = fixture(true).own().configuration().stations[0]
            .weapon
            .clone();
        assert_eq!(scaled_weapon_damage(&missile, 11), 11);
        let aircraft = target(1, [0.; 3], 20, 0x80);
        let mut surface = aircraft.clone();
        surface.role = TargetRole::Surface;
        assert!(critical_hit(&aircraft, &gun, DamageSection::Cockpit, 1));
        assert!(!critical_hit(&surface, &gun, DamageSection::Cockpit, 20));
    }
    #[test]
    fn gun_dispersion_is_bounded_normalized_symmetric_and_deterministic() {
        let mut gun = fixture(false).own().configuration().stations[0]
            .weapon
            .clone();
        gun.source = "M61.JT".into();
        let forward = [0., 0., 1.];
        let first = projectile_launch_direction(&gun, forward, 42, 7, 0);
        assert_eq!(first, projectile_launch_direction(&gun, forward, 42, 7, 0));
        let mut mean = [0.; 3];
        let samples = 20_000;
        for id in 0..samples {
            let direction = projectile_launch_direction(&gun, forward, id, 7, 0);
            let length = dot(direction, direction).sqrt();
            let angle = dot(direction, forward).clamp(-1., 1.).acos();
            assert!((length - 1.).abs() < 1e-12);
            assert!(angle <= GUN_DISPERSION_HALF_ANGLE + 1e-12);
            for axis in 0..3 {
                mean[axis] += direction[axis] / f64::from(samples);
            }
        }
        assert!(mean[0].abs() < 5e-5, "lateral bias {}", mean[0]);
        assert!(mean[1].abs() < 5e-5, "vertical bias {}", mean[1]);
        let expected_cos = (1. + GUN_DISPERSION_HALF_ANGLE.cos()) * 0.5;
        assert!((mean[2] - expected_cos).abs() < 2e-7);
        let missile = fixture(true).own().configuration().stations[0]
            .weapon
            .clone();
        assert_eq!(
            projectile_launch_direction(&missile, forward, 42, 7, 0),
            forward
        );
    }

    #[test]
    fn live_gun_release_applies_dispersion_once() {
        let mut s = fixture(false);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        let launcher = launcher();
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher,
            }],
            |_, _| 0.,
        );
        let projectile = s.projectiles.first().expect("gun round was not released");
        let angle = dot(projectile.direction, launcher.basis.forward)
            .clamp(-1., 1.)
            .acos();
        assert!(angle > 0. && angle <= GUN_DISPERSION_HALF_ANGLE + 1e-12);
        let direction = projectile.direction;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].direction, direction);
    }

    #[test]
    fn physical_gun_rounds_are_evenly_paced_and_preserve_ammo_rate() {
        let mut s = fixture(false);
        let w = &mut s.own_mut().config.stations[0].weapon;
        w.source = "M61.JT".into();
        w.burst.actual_rounds_per_game = 2;
        w.burst.game_rounds_in_burst = 4;
        w.burst.game_burst_t = 1;
        s.own_mut().ammo[0] = 1000;
        let launcher = launcher();
        let mut fired_ticks = Vec::new();
        let mut tracers = 0;
        for tick in 0..120 {
            let events = s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher,
                }],
                |_, _| -10000.,
            );
            let fired = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::Fired {
                            aircraft: 0,
                            station: 0
                        }
                    )
                })
                .count();
            assert!(fired <= 1, "gun emitted simultaneous rounds at tick {tick}");
            if fired == 1 {
                fired_ticks.push(tick);
                tracers += usize::from(s.projectiles.last().unwrap().tracer);
            }
        }
        assert_eq!(fired_ticks.len(), 32);
        assert_eq!(s.own().rounds(0), 968);
        assert_eq!(tracers, 11);
        assert!(
            fired_ticks
                .windows(2)
                .all(|pair| (3..=4).contains(&(pair[1] - pair[0])))
        );
    }

    #[test]
    fn gun_cadence_keeps_fractional_rate_and_repress_phase() {
        let mut s = fixture(false);
        let w = &mut s.own_mut().config.stations[0].weapon;
        w.source = "M61.JT".into();
        w.burst.actual_rounds_per_game = 3;
        w.burst.game_rounds_in_burst = 7;
        w.burst.game_burst_t = 2;
        s.own_mut().ammo[0] = 2000;
        let launcher = launcher();
        let mut fired = 0;
        for _ in 0..1200 {
            let events = s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher,
                }],
                |_, _| -10000.,
            );
            let count = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::Fired {
                            aircraft: 0,
                            station: 0
                        }
                    )
                })
                .count();
            assert!(count <= 1);
            fired += count;
        }
        assert_eq!(fired, 420);
        assert_eq!(s.own().rounds(0), 2000 - fired as u16);
        assert_eq!(s.own().gun_cadence[0].ordinal, fired as u64);

        let before = s.own().shots;
        s.release(0);
        for _ in 0..1 {
            assert!(
                !s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher
                    }],
                    |_, _| -10000.
                )
                .iter()
                .any(|event| matches!(
                    event,
                    Event::Fired {
                        aircraft: 0,
                        station: 0
                    }
                ))
            );
        }
        let first = (0..120)
            .find(|_| {
                s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher,
                    }],
                    |_, _| -10000.,
                )
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        Event::Fired {
                            aircraft: 0,
                            station: 0
                        }
                    )
                })
            })
            .unwrap();
        assert!(first < 4);
        assert_eq!(s.own().shots, before + 1);

        // A release shorter than the physical shot gap cannot accelerate fire.
        let mut s = fixture(false);
        let w = &mut s.own_mut().config.stations[0].weapon;
        w.source = "M61.JT".into();
        w.burst.actual_rounds_per_game = 2;
        w.burst.game_rounds_in_burst = 4;
        w.burst.game_burst_t = 1;
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher
                }],
                |_, _| -10000.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
        s.release(0);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher,
            }],
            |_, _| -10000.,
        );
        for _ in 0..2 {
            assert!(
                !s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher
                    }],
                    |_, _| -10000.
                )
                .contains(&Event::Fired {
                    aircraft: 0,
                    station: 0
                })
            );
        }
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher
                }],
                |_, _| -10000.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
    }

    #[test]
    fn physical_round_damage_partitions_without_rounding_inflation() {
        let mut s = fixture(false);
        let w = &mut s.own_mut().config.stations[0].weapon;
        w.source = "M61.JT".into();
        w.burst.actual_rounds_per_game = 2;
        let mut p = Projectile {
            id: 0,
            owner: OWN,
            weapon: None,
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: None,
            station: 0,
            position: [0.; 3],
            previous: [0.; 3],
            direction: [0., 0., 1.],
            speed_f8: 0,
            launched_t: 0,
            target: None,
            fall: FallState::default(),
            gun_round: Some(0),
            tracer: true,
        };
        assert_eq!(projectile_damage(&p, w, 10), 2);
        p.gun_round = Some(1);
        assert_eq!(projectile_damage(&p, w, 10), 1);
        assert_eq!(projectile_damage(&p, w, 2), 0);
        // nr-nightfx: corrected record values, all truck/tank/SAM classes.
        for (source, before, after, expected) in [
            ("C_40.JT", 50, 100, [17, 16]),
            ("C_105.JT", 150, 300, [50, 50]),
            ("C_25.JT", 30, 30, [5, 5]),
        ] {
            w.source = source.into();
            for class in 0..5 {
                w.damage.by_class[class] = after;
                for (round, expected) in expected.into_iter().enumerate() {
                    p.gun_round = Some(round as u8);
                    assert_eq!(
                        projectile_damage(&p, w, i32::from(w.damage.by_class[class])),
                        expected
                    );
                    let old = projectile_damage(&p, w, before);
                    assert!(expected >= old);
                }
            }
        }
    }

    #[test]
    fn station_cycle_discards_queued_gun_rounds() {
        let mut s = fixture(false);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        s.own_mut().config.stations[0]
            .weapon
            .burst
            .game_rounds_in_burst = 4;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: launcher(),
            }],
            |_, _| -10000.,
        );
        assert!(s.own().gun_cadence[0].pending > 0);
        s.own_mut().select_next();
        assert_eq!(s.own().gun_cadence[0].pending, 0);
    }

    #[test]
    fn every_supported_aircraft_uses_its_canonical_gun_damage_and_dispersion() {
        let aircraft = AircraftId::ALL
            .into_iter()
            .chain([AircraftId::Faxx])
            .filter(|id| id.gun().is_some())
            .collect::<Vec<_>>();
        assert_eq!(
            aircraft.len(),
            AircraftId::SELECTABLE
                .iter()
                .filter(|id| id.gun().is_some())
                .count()
        );
        let launcher = launcher();
        for (number, id) in aircraft.into_iter().enumerate() {
            let mut state = fixture(false);
            state.own_mut().config.aircraft = id;
            state.own_mut().config.stations[0].weapon.source = id.gun().unwrap().into();
            let gun = &state.own().config.stations[0].weapon;
            assert!(is_gun(gun), "{} gun was not recognized", id.label());
            assert_eq!(scaled_weapon_damage(gun, 11), 3, "{} damage", id.label());
            let expected =
                projectile_launch_direction(gun, launcher.basis.forward, number as u32, OWN, 0);
            assert_eq!(
                expected,
                projectile_launch_direction(gun, launcher.basis.forward, number as u32, OWN, 0,),
                "{} deterministic direction",
                id.label()
            );
            let angle = dot(expected, launcher.basis.forward).clamp(-1., 1.).acos();
            assert!(
                angle <= GUN_DISPERSION_HALF_ANGLE + 1e-12,
                "{} dispersion angle {angle}",
                id.label()
            );
            state.next_shot = number as u32;
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher,
                }],
                |_, _| 0.,
            );
            assert_eq!(
                state.projectiles[0].direction,
                expected,
                "{} live gun path",
                id.label()
            );
        }
        let mut missile = fixture(true).own().configuration().stations[0]
            .weapon
            .clone();
        missile.source = "AIM120.JT".into();
        assert!(!is_gun(&missile));
        assert_eq!(scaled_weapon_damage(&missile, 11), 11);
        assert_eq!(
            projectile_launch_direction(&missile, launcher.basis.forward, 9, 4, 1),
            launcher.basis.forward
        );
    }
    #[test]
    fn live_gun_crosses_narrowphase_before_cockpit_kill_without_structural_loss() {
        let mut s = fixture(false);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        s.own_mut().config.stations[0].weapon.damage.by_class[0] = 6;
        s.targets.clear();
        let target_position = [0., 1000., 300.];
        s.targets.push(target(9, target_position, 20, 0x80));
        let position = [10., target_position[1] + 6., target_position[2] + 7.];
        s.projectiles.push(Projectile {
            id: 99,
            owner: OWN,
            weapon: None,
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: None,
            station: 0,
            position,
            previous: position,
            direction: [-1., 0., 0.],
            speed_f8: 1200 * 256,
            launched_t: 0,
            target: None,
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        });
        let events = s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert!(events.contains(&Event::Destroyed(9)));
        assert_eq!(
            s.targets[0].localized_damage.amounts[DamageSection::Cockpit as usize],
            2
        );
        assert_eq!(s.targets[0].localized_damage.structural_section, None);
    }
    #[test]
    fn store_pair_loadout_steps_migration_mass_and_weight_limits() {
        use crate::combat::{hardpoints::Rack, loadout::Loadout};
        use tore_formats::aircraft::Hardpoint;
        let mut configuration = fixture(true).own().configuration().clone();
        let station = &mut configuration.stations[0];
        station.weapon.source = "AIM120.JT".into();
        station.weapon.weight = 300;
        station.internal = false;
        station.count = 4;
        station.mount = [10., -2., 0.];
        station.rack = Rack {
            location: 4,
            paired: true,
            ..Rack::default()
        };
        configuration
            .stations
            .push(configuration.stations[0].clone());
        configuration.hardpoint_slots = vec![Some(0), Some(1)];
        let h = Hardpoint {
            location: 4,
            flags: 0x85,
            position: [30, -6, 0],
            store: Some("AIM120.JT".into()),
            count: 4,
            weight_class: 10,
        };
        let mut load = Loadout {
            aircraft: AircraftId::F18,
            configuration,
            quantities: vec![4, 0],
            fuel_lbs: 0.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 10000.,
            maximum_lbs: 20000.,
            cheat: false,
            tank_hardpoints: vec![],
            hardpoints: vec![h.clone(), h],
        };
        let weapon = load.configuration.stations[0].weapon.clone();
        assert_eq!(load.capacity_before_store_pairs(0, &weapon), 3);
        assert_eq!(load.capacity(0, &weapon), 2);
        load.select(0, weapon.clone()).unwrap();
        assert_eq!(load.quantities, [2, 0]);
        load.change(0, -1);
        assert_eq!(load.quantities, [0, 0]);
        load.change(0, 1);
        load.transfer(0, 1).unwrap();
        assert_eq!(load.quantities, [0, 2]);
        let pair_lbs = 2. * weapon.carried_weight_lb();
        assert_eq!(load.total_lbs(), load.empty_lbs + load.fuel_lbs + pair_lbs);
        let own = Ownship::new(1, Side(0), load.configuration.clone(), true).unwrap();
        let mut own = own;
        own.ammo = load.ammunition().unwrap();
        assert_eq!(own.payload_lbs(), pair_lbs);
        load.quantities[0] = 1;
        assert!(load.validate().is_err());
        load.configuration.stations[0].count = 3;
        load.normalize_saved_store_pairs().unwrap();
        assert_eq!(load.quantities, [2, 2]);
        assert_eq!(load.configuration.stations[0].count, 2);
        // The requested minimum pair can exceed the old row weight class,
        // but never bypasses aircraft takeoff weight validation.
        load.hardpoints[0].weight_class = 5;
        assert_eq!(load.capacity_before_store_pairs(0, &weapon), 1);
        assert_eq!(load.capacity(0, &weapon), 2);
        load.select(0, weapon).unwrap();
        load.maximum_lbs = 11000.;
        load.validate_for_editing().unwrap();
        assert!(
            load.validate()
                .unwrap_err()
                .to_string()
                .contains("too heavy")
        );
        load.configuration.stations[1].rack.paired = false;
        load.quantities = vec![2, 1];
        load.transfer(1, 0).unwrap();
        assert_eq!(
            load.quantities,
            [2, 1],
            "a single store cannot split a pair"
        );
    }

    #[test]
    fn preflight_transfers_conserve_counts_and_respect_capacity_steps() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        for (capacity, expected_step) in [(4, 1), (100, 1), (101, 10), (300, 10), (301, 100)] {
            let mut configuration = fixture(true).own().configuration().clone();
            configuration
                .stations
                .push(configuration.stations[0].clone());
            let hardpoint = Hardpoint {
                location: 4,
                flags: 8,
                position: [0; 3],
                store: Some(configuration.stations[0].weapon.source.clone()),
                count: capacity,
                weight_class: 0,
            };
            let mut load = Loadout {
                aircraft: AircraftId::F18,
                configuration,
                quantities: vec![capacity as u16, 0],
                fuel_lbs: 0.,
                internal_capacity_lbs: 1000.,
                empty_lbs: 10000.,
                maximum_lbs: 20000.,
                cheat: false,
                tank_hardpoints: vec![],
                hardpoints: vec![hardpoint.clone(), hardpoint],
            };
            load.transfer(0, 1).unwrap();
            assert_eq!(
                load.quantities,
                [capacity as u16 - expected_step, expected_step]
            );
            load.quantities = vec![2, capacity as u16 - 1];
            load.transfer(0, 1).unwrap();
            assert_eq!(load.quantities, [1, capacity as u16]);
            load.transfer(0, 1).unwrap();
            load.transfer(0, 0).unwrap();
            assert_eq!(load.quantities, [1, capacity as u16]);
            load.quantities[1] = 0;
            load.transfer(0, 1).unwrap();
            assert_eq!(load.quantities, [0, 1]);
            load.transfer(0, 1).unwrap();
            assert_eq!(load.quantities, [0, 1]);
            load.quantities[0] = 2;
            load.hardpoints[1].store = Some("OTHER.JT".into());
            assert!(load.transfer(0, 1).is_err());
            assert_eq!(load.quantities, [2, 1]);
            load.hardpoints[1].store = load.hardpoints[0].store.clone();
            load.configuration.stations[1].weapon.source = "REPLACED.JT".into();
            load.transfer(0, 1).unwrap();
            assert_eq!(
                load.quantities,
                [2 - expected_step.min(2), expected_step.min(2)]
            );
            assert_eq!(
                load.configuration.stations[1].weapon.source,
                load.configuration.stations[0].weapon.source
            );
        }
    }
    #[test]
    fn guns_only_removes_internal_bay_and_external_weapons_without_refilling_guns() {
        use crate::combat::loadout::Loadout;
        for aircraft in AircraftId::SELECTABLE
            .into_iter()
            .filter(|id| id.gun().is_some())
        {
            let mut configuration = fixture(true).own().configuration().clone();
            configuration.aircraft = aircraft;
            configuration.stations[0].weapon.source = "AIM120.JT".into();
            let mut bay = configuration.stations[0].clone();
            bay.internal = true;
            let mut gun = bay.clone();
            gun.weapon.source = aircraft.gun().unwrap().into();
            configuration.stations.extend([bay, gun]);
            let mut load = Loadout {
                aircraft,
                configuration,
                quantities: vec![2, 4, 7],
                fuel_lbs: 1000.,
                internal_capacity_lbs: 1000.,
                empty_lbs: 10000.,
                maximum_lbs: 20000.,
                cheat: false,
                tank_hardpoints: vec![],
                hardpoints: vec![],
            };
            load.restrict_to_guns();
            assert_eq!(load.quantities, [0, 0, 7]);
            // Configuration retains valid station capacities; accepted ammo
            // is stored separately and restored by the host on restart.
            assert!(load.configuration.stations.iter().all(|s| s.count == 11));
            load.restrict_to_guns();
            assert_eq!(load.quantities, [0, 0, 7]);
        }
    }
    #[test]
    fn preflight_draft_capacity_fuel_mass_and_clone_isolation() {
        use crate::combat::loadout::Loadout;
        use tore_formats::aircraft::Hardpoint;
        let mut config = fixture(false).own().configuration().clone();
        config.stations[0].weapon.source = "M61.JT".into();
        let mut load = Loadout {
            aircraft: AircraftId::F18,
            configuration: config,
            quantities: vec![11],
            fuel_lbs: 900.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 10000.,
            maximum_lbs: 11000.,
            cheat: false,
            tank_hardpoints: vec![],
            hardpoints: vec![Hardpoint {
                location: 2,
                flags: 8,
                position: [0; 3],
                store: Some("M61.JT".into()),
                count: 1000,
                weight_class: 0,
            }],
        };
        let original = load.clone();
        load.change(0, 1);
        assert_eq!(load.quantities[0], 111);
        load.fuel(true);
        assert_eq!(load.fuel_lbs, 1000.);
        assert!(load.validate().is_ok());
        load.fuel(false);
        load.fuel(false);
        load.fuel(false);
        assert_eq!(load.fuel_lbs, 0.);
        assert_eq!(original.quantities[0], 11);
        assert_eq!(original.fuel_lbs, 900.);
        let mut other = load.configuration.stations[0].weapon.clone();
        other.source = "OTHER.JT".into();
        assert!(load.select(0, other).is_err());
        assert_eq!(load.quantities[0], 111);
        load.fuel_lbs = f64::NAN;
        assert!(load.validate().is_err());
        load.fuel_lbs = 1000.;
        load.maximum_lbs = 10999.;
        assert!(load.validate().is_err());
        load.maximum_lbs = 11000.;
        load.quantities[0] = 1001;
        assert!(load.validate().is_err());
    }
    /// Synthetic sensor suite: a 90/50 nmi radar shape so the default 10-mile
    /// display selects TWS, plus the short visual channel every aircraft has.
    fn sensor_profiles() -> sensors::SensorProfiles {
        let volume = |nmi: f64| sensors::Volume {
            azimuth_rad: 1.,
            elevation_rad: 1.,
            minimum_ft: 0.,
            maximum_ft: nmi * sensors::FEET_PER_NAUTICAL_MILE,
            minimum_relative_ft: f64::NEG_INFINITY,
            maximum_relative_ft: f64::INFINITY,
        };
        sensors::SensorProfiles {
            aircraft: AircraftId::F18,
            radar: Some(sensors::RadarProfile {
                record: "SYNTHETIC.SEE".into(),
                search: volume(90.),
                track: volume(50.),
                look_down: 0.,
                preset: sensors::Preset::Advanced,
                notch: sensors::Preset::Advanced.notch(),
                resistance: sensors::Preset::Advanced.resistance(),
                band: 0,
                source_flags: [0; 2],
                source_doppler: [0; 3],
            }),
            infrared: None,
            visual: Some(sensors::profile::VisualProfile {
                record: "SYNTHETIC.VIS".into(),
                search: volume(10.),
                track: volume(5.),
            }),
            jammer: None,
            signature: sensors::SignatureProfile::default(),
        }
    }
    fn launcher() -> Launcher {
        Launcher {
            position: [0., 1000., 0.],
            basis: Basis::new(0., 0., 0.),
            speed_fps: 300.,
            velocity: [0., 0., 300.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: sensors::Controls::default(),
        }
    }
    #[test]
    fn player_chaff_decoys_only_radar_missiles_guiding_on_the_player() {
        let armed = |chance: u8| {
            let mut s = fixture(true);
            s.own_mut().config.stations[0]
                .weapon
                .seeker
                .chaff_flare_chance = chance;
            s.own_mut().config.ecm.chaff[1] = 100;
            s.own_mut().config.ecm.flare[1] = 100;
            s.own_mut().chaff = 2;
            s.own_mut().flares = 1;
            s.command(0, Command::Incoming, launcher());
            assert_eq!(s.projectiles[0].target, Some(0));
            s
        };
        // A flare cannot decoy a radar seeker, but it is still spent.
        let mut s = armed(100);
        s.command(0, Command::ReleaseFlare, launcher());
        assert_eq!((s.own().chaff, s.own().flares), (2, 0));
        assert_eq!(s.projectiles[0].target, Some(0));
        // One flare device is shown as a pair.
        assert_eq!((s.devices.flares.len(), s.devices.chaff.len()), (2, 0));
        assert!(s.effects.is_empty());
        // Chaff at 100 x 100 percent always decoys the radar missile.
        s.command(0, Command::ReleaseChaff, launcher());
        assert_eq!(s.own().chaff, 1);
        assert_eq!(s.projectiles[0].target, None);
        assert!(s.projectiles[0].guidance.is_none());
        assert_eq!((s.devices.flares.len(), s.devices.chaff.len()), (2, 1));
        // A resistant seeker keeps guiding.
        let mut s = armed(0);
        s.command(0, Command::ReleaseChaff, launcher());
        assert_eq!((s.own().chaff, s.projectiles[0].target), (1, Some(0)));
        // An empty dispenser releases nothing.
        let mut s = armed(100);
        s.own_mut().chaff = 0;
        s.command(0, Command::ReleaseChaff, launcher());
        assert_eq!(
            (s.devices.chaff.len(), s.projectiles[0].target),
            (0, Some(0))
        );
        // Unlimited ammo releases without spending.
        let mut s = armed(100);
        s.cheats.unlimited_ammo = true;
        s.command(0, Command::ReleaseChaff, launcher());
        assert_eq!((s.own().chaff, s.projectiles[0].target), (2, None));
    }
    #[test]
    fn countermeasures_tolerate_an_ai_missile_from_a_longer_loadout() {
        let mut s = fixture(true);
        s.own_mut().config.ecm.chaff[1] = 100;
        s.own_mut().chaff = 1;
        s.command(0, Command::Incoming, launcher());
        // An AI shooter's station index need not exist on the player's aircraft.
        let weapon = s.own().config.stations[0].weapon.clone();
        s.projectiles[0].weapon = Some(weapon);
        s.projectiles[0].station = s.own().config.stations.len();
        s.command(0, Command::ReleaseChaff, launcher());
        assert_eq!(s.own().chaff, 0);
    }
    #[test]
    fn every_released_device_sounds_once_from_its_aircraft() {
        use crate::acoustics::Kind;
        let heard = |s: &mut State| {
            s.take_sound_events()
                .into_iter()
                .map(|e| (e.kind, e.position, e.arrived, e.own))
                .collect::<Vec<_>>()
        };
        let mut s = fixture(true);
        s.own_mut().chaff = 1;
        s.own_mut().flares = 1;
        heard(&mut s);
        let at = launcher().position;
        s.command(0, Command::ReleaseChaff, launcher());
        s.command(0, Command::ReleaseFlare, launcher());
        // Empty dispensers release nothing, so nothing is heard.
        s.command(0, Command::ReleaseChaff, launcher());
        s.command(0, Command::ReleaseFlare, launcher());
        assert_eq!(
            heard(&mut s),
            [
                (Kind::Chaff, at, false, true),
                (Kind::Flare, at, false, true)
            ]
        );
        // An AI release is heard from that aircraft and shows the same cloud.
        s.device_released(
            super::super::countermeasures::Release {
                position: [10., 20., 30.],
                velocity: [0.; 3],
                basis: Basis::new(0., 0., 0.),
            },
            EffectKind::Chaff,
            4,
        );
        assert_eq!(
            heard(&mut s),
            [(Kind::Chaff, [10., 20., 30.], false, false)]
        );
        assert_eq!((s.devices.flares.len(), s.devices.chaff.len()), (2, 2));
        assert!(s.effects.is_empty());
    }
    #[test]
    fn releases_resets_and_the_players_decoy_rolls_are_noted_for_recordings() {
        let mut s = fixture(true);
        s.own_mut().config.stations[0]
            .weapon
            .seeker
            .chaff_flare_chance = 50;
        s.own_mut().config.ecm.chaff[1] = 100;
        (s.own_mut().chaff, s.own_mut().flares) = (2, 1);
        s.command(0, Command::Incoming, launcher());
        let missile = s.projectiles[0].id;
        s.command(0, Command::ReleaseChaff, launcher());
        let decoyed = s.projectiles[0].target.is_none();
        // A flare cannot decoy the radar missile, so it rolls nothing.
        s.command(0, Command::ReleaseFlare, launcher());
        s.device_released(
            super::super::countermeasures::Release {
                position: [10., 20., 30.],
                velocity: [1., 2., 3.],
                basis: Basis::new(0.5, 0., 0.),
            },
            EffectKind::Flare,
            4,
        );
        let l = launcher();
        let player = super::super::countermeasures::Release {
            position: l.position,
            velocity: l.velocity,
            basis: l.basis,
        };
        let notes = s.take_device_notes();
        assert_eq!(
            notes,
            [
                DeviceNote::Released(DeviceRelease {
                    owner: OWN,
                    kind: EffectKind::Chaff,
                    release: player,
                    number: 1,
                    tick: 0,
                    left: Some(1),
                }),
                DeviceNote::Released(DeviceRelease {
                    owner: OWN,
                    kind: EffectKind::Flare,
                    release: player,
                    number: 2,
                    tick: 0,
                    left: Some(0),
                }),
                DeviceNote::Released(DeviceRelease {
                    owner: 4,
                    kind: EffectKind::Flare,
                    release: super::super::countermeasures::Release {
                        position: [10., 20., 30.],
                        velocity: [1., 2., 3.],
                        basis: Basis::new(0.5, 0., 0.),
                    },
                    number: 3,
                    tick: 0,
                    left: None,
                }),
            ]
        );
        let rolls = s.take_decoy_rolls();
        assert_eq!(rolls.len(), 1);
        let roll = rolls[0];
        assert_eq!(
            (
                roll.projectile,
                roll.kind,
                roll.susceptibility,
                roll.effectiveness,
                roll.threshold
            ),
            (missile, EffectKind::Chaff, 50, 100, 50)
        );
        assert_eq!(roll.decoyed, decoyed);
        assert_eq!(roll.decoyed, roll.roll < 50);
        // The roll names the chaff cartridge it was against: the number of
        // the player's chaff release among the notes above.
        assert_eq!(roll.device, 1);
        assert!(s.take_device_notes().is_empty() && s.take_decoy_rolls().is_empty());
        // A range reset clears the devices, and says so.
        s.range_target(0, launcher());
        assert_eq!(s.take_device_notes(), [DeviceNote::Cleared(0)]);
        // Bounded when nobody drains them.
        for _ in 0..MAX_RELEASE_RECORDS + 5 {
            s.device_released(player, EffectKind::Chaff, 4);
        }
        let notes = s.take_device_notes();
        assert_eq!(notes.len(), MAX_RELEASE_RECORDS);
        assert!(
            matches!(notes[0], DeviceNote::Released(r) if r.number == 6),
            "{:?}",
            notes[0]
        );
    }
    #[test]
    fn an_ai_round_does_not_credit_the_player_score() {
        // Drive a real shot into a real target twice: once owned by the
        // player and once owned by an AI actor. The damage, the hit event and
        // the destroyed event are identical; only the player's counters differ.
        fn run(owner: u32) -> (u32, u32, Vec<Event>) {
            let mut s = fixture(false);
            let l = launcher();
            s.range_target(0, l);
            let mut collected = Vec::new();
            let mut strikes = Vec::new();
            for _ in 0..600 {
                for p in &mut s.projectiles {
                    // A fresh round of another aircraft starts 60 ft ahead,
                    // clear of the ownship's own volume, which it would hit.
                    if p.owner != owner {
                        p.owner = owner;
                        p.position[2] += 60.;
                        p.previous = p.position;
                    }
                }
                collected.extend(s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l,
                    }],
                    |_, _| 0.,
                ));
                strikes.extend(s.take_strikes());
            }
            // Every hit through the killing hit names its owner and victim
            // for the radio. Later wreck impacts produce no new damage call.
            let hits: Vec<_> = collected
                .iter()
                .take_while(|e| !matches!(e, Event::Destroyed(_)))
                .filter_map(|e| match e {
                    Event::Hit(id) => Some(*id),
                    _ => None,
                })
                .collect();
            assert_eq!(strikes.iter().map(|s| s.victim).collect::<Vec<_>>(), hits);
            assert!(strikes.iter().all(|s| s.owner == owner));
            assert_eq!(
                strikes.iter().filter(|s| s.destroyed).count(),
                collected
                    .iter()
                    .filter(|e| matches!(e, Event::Destroyed(_)))
                    .count()
            );
            assert!(s.take_strikes().is_empty(), "draining empties the log");
            (s.own().hits, s.own().kills, collected)
        }

        let (player_hits, player_kills, player_events) = run(OWN);
        assert!(
            player_hits > 0,
            "the fixture never scored a hit, so the test proves nothing"
        );
        assert!(player_events.iter().any(|e| matches!(e, Event::Hit(_))));

        let (ai_hits, ai_kills, ai_events) = run(5);
        assert_eq!(ai_hits, 0, "an AI round credited the player with a hit");
        assert_eq!(ai_kills, 0, "an AI round credited the player with a kill");
        // The host still needs the events for damage, debris and effects.
        let player_hit_events = player_events
            .iter()
            .filter(|e| matches!(e, Event::Hit(_)))
            .count();
        let ai_hit_events = ai_events
            .iter()
            .filter(|e| matches!(e, Event::Hit(_)))
            .count();
        assert_eq!(
            player_hit_events, ai_hit_events,
            "ownership must change the score, not the damage"
        );
        let _ = player_kills;
    }

    #[test]
    fn mission_dummies_keep_distinct_identity_and_straight_velocity() {
        let mut s = fixture(true);
        let mut config = s.own().configuration().clone();
        config.hit_points = 37;
        let basis = Basis::new(0.3, 0., 0.);
        for n in 0..29 {
            s.add_dummy(&config, [n as f64 * 500., 5000., 6000.], basis, Side(2));
        }
        let before = s.targets.clone();
        observe(&mut s, launcher(), 120);
        for (index, (a, b)) in before.iter().zip(&s.targets).enumerate() {
            assert_eq!(b.id, index as u32 + 1);
            assert_eq!(b.hp, 37);
            assert_eq!(b.signature, config.sensors.signature);
            assert_eq!(b.velocity, a.velocity);
            assert!(!b.radar_emitting && !b.jammer_active);
            for i in 0..3 {
                assert!((b.position[i] - a.position[i] - a.velocity[i]).abs() < 1e-7);
            }
        }
        s.range_target(0, launcher());
        assert_eq!(s.targets[0].id, 30);
    }
    /// Selection needs a current observation, so the shared sensors must have
    /// produced contacts before a designation command is applied.
    const ACQUISITION: usize = crate::sensors::track::ACQUISITION_STEPS as usize;
    fn observe(s: &mut State, l: Launcher, steps: usize) {
        for _ in 0..steps {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
    }
    pub(super) fn target(id: u32, position: Vector, hp: i32, category: u16) -> Target {
        Target {
            aircraft: Some(AircraftId::F18),
            role: TargetRole::Aircraft,
            heat: Heat::Unknown,
            radar_emitting: false,
            id,
            position,
            velocity: [0.; 3],
            basis: Basis::new(0., 0., 0.),
            configuration: sensors::Configuration::CLEAN,
            signature: sensors::SignatureProfile::default(),
            jammer: None,
            jammer_active: false,
            airborne: true,
            on_ground: false,
            radius: 20.,
            hp,
            initial_hp: hp,
            fragment_offsets: [[0.; 3]; 2],
            wreck: None,
            wreck_power: crate::wreck::Power::default(),
            fragment_released: false,
            localized_damage: LocalizedDamage::default(),
            faults: Default::default(),
            category,
            side: NO_SIDE,
        }
    }
    #[test]
    fn ground_projectile_damage_uses_object_class_and_destroys_once() {
        let mut s = fixture(false);
        s.targets.clear();
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        s.own_mut().config.stations[0].weapon.damage.by_class[2] = 17;
        let bounds = crate::airport::OrientedBox {
            center: [0., 1000., 300.],
            half: [30., 30., 30.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
        };
        s.add_ground_target(0x40000000, bounds, 17, 0x100, NO_SIDE)
            .unwrap();
        let mut destroyed = 0;
        for _ in 0..300 {
            destroyed += s
                .step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: launcher(),
                    }],
                    |_, _| 0.,
                )
                .iter()
                .filter(|e| **e == Event::Destroyed(0x40000000))
                .count();
        }
        assert_eq!(destroyed, 1);
        assert_eq!(s.targets[0].hp, 0);
        assert_eq!(s.history[0].class, 2);
        assert_eq!(s.history[1].class, 2);
        assert_eq!([s.history[0].applied, s.history[1].applied], [3, 2]);
        assert_eq!(s.history[0].applied + s.history[1].applied, 5);
        assert_eq!(s.targets[0].localized_damage, LocalizedDamage::default());
    }
    #[test]
    fn range_controls_preserve_imported_ground_geometry_and_damage() {
        let mut s = fixture(false);
        let bounds = crate::airport::OrientedBox {
            center: [100., 20., 500.],
            half: [10., 10., 30.],
            heading: 0.3,
            pitch: 0.,
            bank: 0.,
        };
        s.add_ground_target(0x40000000, bounds, 750, 0x100, NO_SIDE)
            .unwrap();
        s.targets
            .iter_mut()
            .find(|t| t.id == 0x40000000)
            .unwrap()
            .hp = 600;
        s.range_target(0, launcher());
        s.command(0, Command::TargetDistance(10000), launcher());
        s.command(0, Command::CycleClass, launcher());
        s.command(0, Command::ClearRange, launcher());
        assert_eq!(s.targets.len(), 1);
        let t = &s.targets[0];
        assert_eq!((t.id, t.hp, t.category), (0x40000000, 600, 0x100));
        assert_eq!(
            t.position,
            [
                bounds.center[0],
                bounds.center[1] + bounds.half[1] * 0.5,
                bounds.center[2]
            ]
        );
        assert_eq!(s.ground_bounds[&t.id], bounds);
    }
    #[test]
    fn swept_contact_handles_tunneling_moving_targets_and_nearest_root() {
        assert_eq!(
            segment_sphere([0., 0., -100.], [0., 0., 100.], 10.),
            Some(0.45)
        );
        assert_eq!(segment_sphere([0.; 3], [0.; 3], 1.), Some(0.));
        assert_eq!(segment_sphere([20., 0., -100.], [20., 0., 100.], 10.), None);
        assert_eq!(
            terrain_hit([0., 10., 0.], [0., -10., 100.], &|_, _| 0.),
            Some(0.5)
        );
    }
    #[test]
    fn source_debit_partial_last_round_empty_release_and_expiry() {
        let mut s = fixture(false);
        for _ in 0..300 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.own().ammo, [0]);
        assert_eq!(s.own().shots, 6);
        for _ in 0..600 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
        }
        assert!(s.projectiles.is_empty());
        assert!(s.effects.is_empty());
        let mut s = fixture(false);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        s.release(0);
        for _ in 0..60 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.own().shots, 1);
    }
    #[test]
    fn unlimited_ammo_fires_past_the_loaded_count_without_debit() {
        let mut s = fixture(false);
        s.cheats.unlimited_ammo = true;
        let loaded = s.own().ammo.clone();
        for _ in 0..300 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.own().ammo, loaded);
        assert!(s.own().shots > 6);
    }
    #[test]
    fn invulnerable_cockpit_hit_neither_damages_nor_kills_the_pilot() {
        let mut s = fixture(false);
        s.cheats.damage = crate::cheats::Damage::Invulnerable;
        s.own_mut().config.stations[0].weapon.source = AircraftId::F18.gun().unwrap().into();
        let l = launcher();
        s.command(0, Command::Incoming, l);
        let p = s.projectiles.last_mut().unwrap();
        p.position = std::array::from_fn(|i| {
            l.position[i] + l.basis.forward[i] * 100. + l.basis.up[i] * 11.
        });
        p.previous = p.position;
        p.age = 1;
        let hp = s.own().hp;
        let mut events = Vec::new();
        for _ in 0..120 {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            ));
        }
        assert!(s.projectiles.is_empty(), "the round still hits");
        assert_eq!(s.own().hp, hp);
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::PilotKilled { aircraft: 0 }
                | Event::OwnshipDestroyed { aircraft: 0 }
                | Event::OwnshipDamaged { .. }
        )));
    }
    #[test]
    fn detection_launch_and_inflight_lock_loss_are_distinct() {
        let mut s = fixture(true);
        let mut l = launcher();
        s.range_target(0, l);
        observe(&mut s, l, 1);
        s.designate_next(0, true);
        assert_eq!(s.own_view().designated(), Some(1));
        // Selection is immediate; the fire-control track is not.
        assert_eq!(s.own_view().readiness(l), Readiness::RadarAcquiring);
        observe(&mut s, l, ACQUISITION - 1);
        assert_eq!(s.own().sensors.acquired(), None);
        observe(&mut s, l, 1);
        assert_eq!(s.own().sensors.acquired(), Some(1));
        l.radar = false;
        assert!(!s.own_view().can_lock(l));
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().ammo, [11]);
        // Radar off drops the target completely; it has to be selected again.
        assert_eq!(s.own_view().designated(), None);
        l.radar = true;
        observe(&mut s, l, 1);
        s.designate_next(0, true);
        observe(&mut s, l, 60);
        assert!(s.own_view().can_lock(l));
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().ammo, [9]);
        assert_eq!(s.projectiles[0].target, Some(1));
        l.radar = false;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].target, None);
        assert!(s.projectiles[0].position[2] > 0.);
    }
    #[test]
    fn actual_target_damage_destroys_once_and_generates_effects() {
        let mut s = fixture(false);
        s.targets.push(target(7, [0., 1000., 150.], 20, 0x80));
        let mut kills = 0;
        let mut exploded = false;
        for _ in 0..180 {
            for e in s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            ) {
                if matches!(e, Event::Destroyed(7)) {
                    kills += 1;
                }
            }
            // The victim's own aircraft explosion, one second long.
            exploded |= s.effects.iter().any(|e| {
                e.kind == EffectKind::Destroyed
                    && e.blast
                        .is_some_and(|b| super::super::blast::explosion(b).is_some())
            });
        }
        assert_eq!(
            (s.own().hits, s.own().kills, kills, s.targets[0].hp),
            (6, 1, 1, 0) // Two damaging hits and four physical wreck impacts.
        );
        assert!(exploded);
        // The debrief ledger saw the same rounds, hits, damage and kill.
        let fired = s.ledger.total(|k| k.owner == OWN);
        assert_eq!(fired.launched, s.own().shots);
        assert_eq!((fired.hit, fired.damage), (6, 20));
        assert_eq!(
            s.ledger.kills(),
            [Kill {
                owner: OWN,
                victim: 7,
                category: 0x80,
                aircraft: s.targets[0].role == TargetRole::Aircraft,
            }]
        );
    }
    #[test]
    fn easy_aiming_widens_the_hit_volume_and_blasts_jolt_the_target() {
        let run = |easy: bool| {
            let mut s = fixture(false);
            s.cheats.easy_aiming = easy;
            // Between 1 and 1.5 target radii off the line of fire.
            s.targets.push(target(7, [25., 1000., 150.], 20, 0x80));
            let mut events = Vec::new();
            for _ in 0..180 {
                events.extend(s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: launcher(),
                    }],
                    |_, _| 0.,
                ));
            }
            (s.own().hits, events)
        };
        assert_eq!(run(false).0, 0);
        let (hits, events) = run(true);
        assert!(hits > 0);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Jolt(Jolt { target: 7, strength, .. }) if (*strength - 0.1).abs() < 1e-9
        )));
    }
    #[test]
    fn an_aircraft_target_rolls_system_faults_whatever_the_player_damage_cheat() {
        let mut s = fixture(false);
        assert_eq!(s.cheats.damage, crate::cheats::Damage::Normal);
        let mut t = target(7, [0., 1000., 150.], 200, 0x80);
        t.faults = SystemFaults::new(&s.own().config);
        t.faults.table = [0x1f; 45];
        s.targets.push(t);
        for _ in 0..600 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
            if s.targets[0].hp <= 150 {
                break;
            }
        }
        let t = &s.targets[0];
        assert!(t.hp > 0 && t.hp <= 150, "hp {}", t.hp);
        // A quarter of the hit points gone guarantees a control fault.
        assert!(t.faults.counts.iter().any(|n| *n > 0));
        assert_eq!(s.own().subsystem_counts, [0; 45]);
    }
    #[test]
    fn easy_aiming_missile_turns_faster_with_a_wider_cone() {
        let w = fixture(true).own().config.stations[0].weapon.clone();
        let eased = eased_weapon(&w);
        assert_eq!(
            f64::from(eased.movement.powered_turn_rate),
            (f64::from(w.movement.powered_turn_rate) * 1.5).round()
        );
        assert_eq!(
            f64::from(eased.seeker.zones[0].heading),
            (f64::from(w.seeker.zones[0].heading) * 1.25).round()
        );
        assert_eq!(eased.damage.by_class, w.damage.by_class);
    }
    #[test]
    fn an_incoming_missile_jolts_the_player_even_when_invulnerable() {
        let mut s = fixture(false);
        s.cheats.damage = crate::cheats::Damage::Invulnerable;
        let l = launcher();
        s.command(0, Command::Incoming, l);
        let p = s.projectiles.last_mut().unwrap();
        p.position = std::array::from_fn(|i| l.position[i] + l.basis.forward[i] * 100.);
        p.previous = p.position;
        p.age = 1;
        let mut events = Vec::new();
        for _ in 0..120 {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            ));
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Jolt(Jolt { target: 0, .. })))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { .. }))
        );
    }
    #[test]
    fn the_selection_ring_skips_stations_that_carry_nothing() {
        let mut s = fixture(false);
        let mut gun = s.own().config.stations[0].clone();
        gun.weapon.source = AircraftId::F18.gun().unwrap().into();
        s.own_mut().config.stations.push(gun.clone());
        s.own_mut().config.stations.push(gun);
        s.own_mut().ammo = vec![0, 100, 0];
        s.own_mut().armed = false;
        s.cycle_selection(0, true);
        assert_eq!(
            (s.own().armed, s.own().selected),
            (true, 1),
            "the empty station 0 is skipped"
        );
        s.cycle_selection(0, true);
        assert!(!s.own().armed, "station 2 is empty too, so NAV follows");
        s.cycle_selection(0, false);
        assert_eq!((s.own().armed, s.own().selected), (true, 1));
        // Unlimited ammunition fires past an empty station, so it stays reachable.
        s.cheats.unlimited_ammo = true;
        s.cycle_selection(0, false);
        assert_eq!((s.own().armed, s.own().selected), (true, 0));
    }
    /// A fixture with stations 0 and 1 loaded and station 2 a retained
    /// selection station that never held anything.
    fn fixture_with_retained_station() -> State {
        let mut s = fixture(false);
        let station = s.own().config.stations[0].clone();
        s.own_mut().config.stations.push(station.clone());
        s.own_mut().config.stations.push(station);
        s.own_mut().ammo = vec![2, 2, 0];
        s.own_mut().ever_loaded = vec![true, true, false];
        s.own_mut().armed = true;
        s.own_mut().selected = 0;
        s
    }
    #[test]
    fn next_weapon_skips_a_retained_station_that_was_never_loaded() {
        let mut s = fixture_with_retained_station();
        for cheat in [false, true] {
            s.cheats.unlimited_ammo = cheat;
            s.own_mut().selected = 0;
            s.own_mut().select_next();
            assert_eq!(s.own().selected, 1);
            s.own_mut().select_next();
            assert_eq!(s.own().selected, 0, "the retained station is not a stop");
        }
        // A station that ran dry in flight is still on the aircraft.
        s.own_mut().ammo[1] = 0;
        s.own_mut().selected = 0;
        s.own_mut().select_next();
        assert_eq!(s.own().selected, 1);
    }
    #[test]
    fn selection_ring_skips_a_never_loaded_station_even_with_unlimited_ammo() {
        let mut s = fixture_with_retained_station();
        for cheat in [false, true] {
            s.cheats.unlimited_ammo = cheat;
            s.own_mut().selected = 1;
            s.cycle_selection(0, true);
            assert!(!s.own().armed, "past station 1 comes NAV, not station 2");
            s.cycle_selection(0, false);
            assert_eq!((s.own().armed, s.own().selected), (true, 1));
            s.cycle_selection(0, false);
            s.cycle_selection(0, false);
            s.cycle_selection(0, false);
            assert_ne!(s.own().selected, 2, "backwards never lands on it either");
            assert!(!s.own().carries(2, cheat));
        }
        // A loaded station that runs dry stays selectable under the cheat only.
        s.own_mut().ammo[1] = 0;
        assert!(!s.own().carries(1, false));
        assert!(s.own().carries(1, true));
    }
    #[test]
    fn advance_from_empty_never_lands_on_a_never_loaded_station() {
        let mut s = fixture_with_retained_station();
        s.cheats.unlimited_ammo = true;
        s.own_mut().ammo = vec![0, 0, 0];
        s.own_mut().selected = 0;
        s.own_mut().advance_from_empty(false, true);
        assert_eq!(s.own().selected, 0, "a dry loaded station still carries");
        s.own_mut().selected = 2;
        s.own_mut().advance_from_empty(false, true);
        assert_ne!(
            (s.own().armed, s.own().selected),
            (true, 2),
            "leaves the retained station"
        );
    }
    #[test]
    fn guns_only_leaves_the_player_the_gun_and_nav() {
        let mut s = fixture(false);
        let mut gun = s.own().config.stations[0].clone();
        gun.weapon.source = AircraftId::F18.gun().unwrap().into();
        s.own_mut().config.stations.push(gun);
        s.own_mut().ammo.push(100);
        s.own_mut().armed = true;
        s.own_mut().selected = 0;
        s.cheats.guns_only = true;
        s.own_mut().enforce_guns_only(true);
        assert_eq!(
            (s.own().armed, s.own().selected),
            (true, 1),
            "moved to the gun"
        );
        s.cycle_selection(0, true);
        assert!(!s.own().armed, "NAV follows the gun");
        s.cycle_selection(0, true);
        assert_eq!(
            (s.own().armed, s.own().selected),
            (true, 1),
            "the missile is skipped"
        );
        s.cycle_selection(0, false);
        s.cycle_selection(0, false);
        assert_eq!((s.own().armed, s.own().selected), (true, 1));
        s.cheats.guns_only = false;
        s.cycle_selection(0, false);
        assert_eq!((s.own().armed, s.own().selected), (true, 0));
    }
    #[test]
    fn midair_collisions_destroy_everyone_but_an_invulnerable_player_unless_ignored() {
        let run = |ignore: bool, invulnerable: bool| {
            let mut s = fixture(false);
            if invulnerable {
                s.cheats.damage = crate::cheats::Damage::Invulnerable;
            }
            s.cheats.ignore_midair_collisions = ignore;
            // Two AI aircraft closing head-on, and one well clear.
            let mut a = target(7, [0., 5000., 1000.], 20, 0x80);
            a.velocity = [0., 0., 600.];
            let mut b = target(8, [0., 5000., 1035.], 20, 0x80);
            b.velocity = [0., 0., -600.];
            let clear = target(9, [3000., 5000., 1000.], 20, 0x80);
            // A third aircraft sitting on the player.
            let on_player = target(10, [5., 1000., 5.], 20, 0x80);
            s.targets.extend([a, b, clear, on_player]);
            let events = s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            );
            (s, events)
        };
        let (s, events) = run(false, false);
        let hp: Vec<_> = s.targets.iter().map(|t| t.hp).collect();
        assert_eq!(hp, [0, 0, 20, 0]);
        assert_eq!(s.own().hp, 0);
        assert!(events.contains(&Event::OwnshipDestroyed { aircraft: 0 }));
        assert!(events.contains(&Event::Destroyed(7)) && events.contains(&Event::Destroyed(8)));
        assert_eq!(s.own().kills, 0);
        // Invulnerable spares only the player.
        let (s, events) = run(false, true);
        assert!(s.own().hp > 0 && !events.contains(&Event::OwnshipDestroyed { aircraft: 0 }));
        assert_eq!(s.targets[3].hp, 0);
        let (s, events) = run(true, false);
        assert!(s.targets.iter().all(|t| t.hp == 20) && s.own().hp > 0);
        assert!(!events.iter().any(|e| matches!(e, Event::Destroyed(_))));
    }
    #[test]
    fn fixed_ticks_replay_across_presentation_rates_and_pause() {
        let run = |fps: usize| {
            let mut s = fixture(false);
            s.targets.push(target(7, [0., 1000., 100000.], 100, 0x8000));
            s.targets[0].hp = 50;
            (s.own_mut().chaff, s.own_mut().flares) = (1, 1);
            s.command(0, Command::ReleaseFlare, launcher());
            s.command(0, Command::ReleaseChaff, launcher());
            let mut clock = crate::flight::Clock { remainder: 0. };
            let mut tick = 0;
            for frame in 0..fps * 4 {
                // No call into either clock or combat while paused. Resume has
                // no elapsed wall-time backlog. Inputs are indexed by sim tick.
                if frame >= fps && frame < fps * 2 {
                    continue;
                }
                for _ in 0..clock.steps(1. / fps as f64) {
                    s.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: tick < 90,
                            launcher: launcher(),
                        }],
                        |_, _| 0.,
                    );
                    tick += 1;
                }
            }
            let (ammo, shots) = (s.own().ammo.clone(), s.own().shots);
            (
                tick,
                ammo,
                s.projectiles,
                s.targets,
                s.effects,
                shots,
                s.smoke,
                s.devices,
                s.debris,
            )
        };
        assert_eq!(run(30), run(60));
        assert_eq!(run(60), run(144));
    }
    #[test]
    fn capacity_failure_does_not_debit_and_dead_launcher_cannot_fire() {
        let mut s = fixture(false);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        s.own_mut().config.stations[0].weapon.movement.remove_t = u16::MAX;
        let mut l = launcher();
        l.alive = false;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().shots, 0);
        l.alive = true;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        let p = s.projectiles[0].clone();
        s.projectiles = vec![p; MAX_PROJECTILES];
        let ammo = s.own().ammo.clone();
        for _ in 0..30 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l,
                }],
                |_, _| 0.,
            );
        }
        assert_eq!(s.own().ammo, ammo);
        s.projectiles.clear();
        for _ in 0..14 {
            assert!(
                !s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l
                    }],
                    |_, _| -10000.
                )
                .iter()
                .any(|event| matches!(
                    event,
                    Event::Fired {
                        aircraft: 0,
                        station: 0
                    }
                ))
            );
        }
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| -10000.
            )
            .iter()
            .any(|event| matches!(
                event,
                Event::Fired {
                    aircraft: 0,
                    station: 0
                }
            ))
        );
        assert_eq!(s.own().rounds(0), ammo[0] - 1);
    }
    #[test]
    fn a_rocket_without_a_seeker_smokes_only_while_its_motor_burns() {
        // Bug B6: a rocket pod's rounds have no seeker signature and still
        // smoke while their motor burns; bug B7: the same rule says when
        // the motor is out.
        use super::super::smoke::Kind;
        let l = launcher();
        let mut s = fixture(true);
        let weapon = &mut s.own_mut().config.stations[0].weapon;
        weapon.seeker.signature = 0;
        weapon.movement.ignite_t = 1;
        weapon.movement.fuel_t = 2;
        s.command(0, Command::Incoming, l);
        s.projectiles[0].incoming = None;
        s.projectiles[0].target = None;
        s.projectiles[0].guidance = None;
        s.projectiles[0].motion = None;
        s.projectiles[0].position = [0., 10000., 100000.];
        s.projectiles[0].direction = [0., 0., 1.];
        assert!(!s.motor_powered(&s.projectiles[0]));
        let mut powered = Vec::new();
        for _ in 0..120 {
            observe(&mut s, l, 1);
            let Some(p) = s.projectiles.first() else {
                break;
            };
            powered.push(s.motor_powered(p));
        }
        assert!(powered.iter().any(|on| *on), "the motor lights");
        assert!(!powered.last().copied().unwrap_or(true), "and burns out");
        assert!(!s.smoke.puffs.is_empty());
        assert!(s.smoke.puffs.iter().all(|p| p.kind == Kind::Missile));
        // Once out, no more smoke is added.
        let before = s.smoke.puffs.len();
        observe(&mut s, l, 60);
        assert!(s.smoke.puffs.len() <= before);
    }
    #[test]
    fn motor_smoke_uses_powered_phase_and_aircraft_smoke_uses_health() {
        use super::super::smoke::Kind;
        let l = launcher();
        let mut s = fixture(true);
        s.own_mut().config.stations[0].weapon.movement.ignite_t = 1;
        s.own_mut().config.stations[0].weapon.movement.fuel_t = 2;
        s.command(0, Command::Incoming, l);
        s.projectiles[0].incoming = None;
        s.projectiles[0].target = None;
        s.projectiles[0].position = [0., 10000., 100000.];
        s.projectiles[0].direction = [0., 0., 1.];
        // Exercise the spec vector motor, keeping this fixture far from contacts.
        s.projectiles[0].motion = Some(Motion::new(
            &s.own().config.stations[0].weapon.movement,
            [0., 0., 1000.],
            10000.,
        ));
        observe(&mut s, l, 30);
        assert!(s.smoke.puffs.is_empty());
        observe(&mut s, l, 30);
        assert_eq!(s.smoke.puffs.len(), 4);
        assert!(s.smoke.puffs.iter().all(|p| p.kind == Kind::Missile));
        observe(&mut s, l, 30);
        assert_eq!(s.smoke.puffs.len(), 4);
        s.projectiles.clear();
        observe(&mut s, l, 480);
        assert!(s.smoke.puffs.is_empty());
        s.targets.push(target(1, [0., 5000., 100000.], 100, 0x8000));
        s.targets[0].hp = 51;
        observe(&mut s, l, 12);
        assert!(s.smoke.puffs.is_empty());
        s.targets[0].hp = 50;
        observe(&mut s, l, 12);
        assert_eq!(s.smoke.puffs.len(), 1);
        s.targets[0].hp = 0;
        observe(&mut s, l, 12);
        assert_eq!(s.smoke.puffs.len(), 2);
        s.targets[0].airborne = false;
        observe(&mut s, l, 12);
        assert_eq!(s.smoke.puffs.len(), 2);
        s.range_target(0, l);
        assert!(s.smoke.puffs.is_empty());
        let mut gun = fixture(false);
        observe(&mut gun, l, 6);
        gun.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert!(gun.smoke.puffs.is_empty());
    }
    #[test]
    fn detached_piece_is_emitted_once_and_removed_with_one_ground_effect() {
        let mut s = fixture(false);
        let l = launcher();
        s.targets.push(target(7, [0., 5., 100000.], 100, 0x8000));
        s.targets[0].hp = 0;
        s.targets[0]
            .localized_damage
            .record(DamageSection::LeftWing, 75, 100);
        s.targets[0].velocity = [30., 0., 10.];
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.debris.len(), 1);
        assert_eq!(s.debris[0].owner, 7);
        assert_eq!(s.debris[0].velocity, s.targets[0].velocity);
        let mut impacts = 0;
        for _ in 0..300 {
            let had_piece = !s.debris.is_empty();
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            if had_piece && s.debris.is_empty() {
                impacts += 1;
                assert_eq!(
                    s.effects
                        .iter()
                        .filter(|e| e.kind == EffectKind::DebrisImpact)
                        .count(),
                    1
                );
            }
        }
        assert_eq!(impacts, 1);
        assert!(s.debris.is_empty());
        assert!(s.effects.iter().all(|e| e.kind != EffectKind::DebrisImpact));
        s.targets[0].hp = 0;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert!(
            s.debris.is_empty(),
            "further damage must not duplicate the same lost part"
        );
        s.own_mut().hp = s.own().config.damage_capacity / 2;
        let capacity = s.own().config.damage_capacity;
        s.own_mut()
            .localized_damage
            .record(DamageSection::Nose, capacity, capacity);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert!(
            s.debris.is_empty(),
            "a live aircraft keeps catastrophic parts"
        );
        s.own_mut().hp = 0;
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.debris.len(), 1);
        assert_eq!(s.debris[0].owner, 0);
        assert_eq!(s.debris[0].velocity, l.velocity);
        let reset = State::new(s.own().configuration().clone(), true).unwrap();
        assert!(reset.debris.is_empty());
    }
    #[test]
    fn native_damage_category_switch_is_exact_not_a_mask() {
        for (category, index) in [
            (0x80, 0),
            (0x2000, 1),
            (0x100, 2),
            (0x400, 3),
            (0x40, 4),
            (0x200, 4),
            (0x800, 4),
            (0x1000, 4),
            (0x4000, 0),
            (0x8000, 0),
            (0x240, 0),
        ] {
            assert_eq!(damage_class(category), index);
        }
    }
    #[test]
    fn all_damage_classes_report_nominal_applied_and_cumulative_hp() {
        for (index, category) in [0x80, 0x2000, 0x100, 0x400, 0x40].into_iter().enumerate() {
            let mut s = fixture(false);
            s.own_mut().config.stations[0].weapon.damage.by_class = [3, 7, 9, 11, 25];
            s.targets.push(target(7, [0., 1000., 150.], 20, category));
            for _ in 0..180 {
                s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: launcher(),
                    }],
                    |_, _| 0.,
                );
            }
            assert!(s.history.iter().all(|h| h.class == index));
            assert_eq!(
                s.history.iter().map(|h| h.applied).sum::<i32>(),
                20 - s.targets[0].hp
            );
            assert!(s.history.iter().all(|h| h.applied <= h.nominal));
        }
    }
    #[test]
    fn failed_station_keeps_mass_and_jettison_cannot_remove_internal_gun() {
        let mut s = fixture(true);
        let mass = s.own().payload_lbs();
        s.command(0, Command::FailStation, launcher());
        assert_eq!(s.own_view().readiness(launcher()), Readiness::StationFailed);
        assert_eq!(s.own().rounds(0), 11);
        assert_eq!(s.own().payload_lbs(), mass);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().shots, 0);
        s.command(0, Command::Jettison, launcher());
        assert_eq!(s.own().payload_lbs(), 0.);
        assert_eq!(s.own().ammo[0], 0x8000); // Unloading cannot repair a failed station.
        let mut gun = fixture(false);
        gun.command(0, Command::Jettison, launcher());
        assert_eq!(gun.own().rounds(0), 11);
    }
    #[test]
    fn readiness_reports_inhibits_without_ammunition_consumption() {
        let mut s = fixture(true);
        let l = launcher();
        assert_eq!(s.own_view().readiness(l), Readiness::NoTarget);
        s.command(0, Command::ToggleArm, l);
        assert_eq!(s.own_view().readiness(l), Readiness::Safe);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.own().rounds(0), 11);
        s.command(0, Command::ToggleArm, l);
        s.range_target(0, l);
        observe(&mut s, l, 1);
        s.designate_next(0, true);
        observe(&mut s, l, ACQUISITION);
        assert_eq!(s.own_view().readiness(l), Readiness::Ready);
        let z = &mut s.own_mut().config.stations[0].weapon.seeker.zones[1];
        z.minimum_range = 4000;
        assert_eq!(s.own_view().readiness(l), Readiness::Ready);
        s.own_mut().config.stations[0].weapon.seeker.zones[1].minimum_range = 0;
        s.own_mut().config.stations[0].weapon.seeker.zones[1].maximum_range = 2000;
        assert_eq!(s.own_view().readiness(l), Readiness::MaximumRange);
    }
    #[test]
    fn replacement_clears_old_engagement_and_uses_fresh_identity() {
        let mut s = fixture(true);
        s.range_target(0, launcher());
        observe(&mut s, launcher(), 1);
        s.designate_next(0, true);
        observe(&mut s, launcher(), ACQUISITION);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        let ammo = s.own().ammo.clone();
        assert!(!s.projectiles.is_empty());
        s.range_target(0, launcher());
        assert_eq!(s.targets[0].id, 2);
        assert!(
            s.projectiles.is_empty() && s.effects.is_empty() && s.own_view().designated().is_none()
        );
        assert_eq!(s.own().ammo, ammo);
    }
    #[test]
    fn independent_tracking_retains_a_wreck_until_its_body_is_retired() {
        let mut s = fixture(true);
        s.own_mut().config.stations[0].weapon.flags &= !0x200;
        s.range_target(0, launcher());
        observe(&mut s, launcher(), 1);
        s.designate_next(0, true);
        observe(&mut s, launcher(), ACQUISITION);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: launcher(),
            }],
            |_, _| 0.,
        );
        let mut l = launcher();
        l.radar = false;
        assert_eq!(s.own_view().readiness(l), Readiness::RadarOff);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].target, Some(1));
        s.targets[0].hp = 0;
        let events = s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].target, Some(1));
        assert!(!events.contains(&Event::TrackLost(1)));
        s.targets[0].airborne = false;
        let events = s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].target, None);
        assert!(events.contains(&Event::TrackLost(1)));
    }
    #[test]
    fn terrain_visibility_gates_designation_launch_scope_and_tracking() {
        let mut s = fixture(true);
        let l = launcher();
        s.range_target(0, l);
        observe(&mut s, l, 1);
        s.designate_next(0, true);
        observe(&mut s, l, ACQUISITION);
        let wall = |_: f64, z: f64| {
            if (1000. ..2000.).contains(&z) {
                2000.
            } else {
                0.
            }
        };
        // Masking removes the observation, so selection and the launch
        // permission end together and no round is consumed.
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            wall,
        );
        assert!(s.own().sensors.contacts().is_empty());
        assert_eq!(s.own_view().designated(), None);
        assert_eq!(s.own_view().readiness(l), Readiness::NoTarget);
        assert_eq!(s.own().rounds(0), 11);
        s.designate_next(0, true);
        assert_eq!(s.own_view().designated(), None);
        observe(&mut s, l, ACQUISITION + 1);
        s.designate_next(0, true);
        observe(&mut s, l, ACQUISITION);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: true,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.projectiles[0].target, Some(1));
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l
                }],
                wall
            )
            .contains(&Event::TrackLost(1))
        );
    }
    #[test]
    fn manual_command_tape_is_identical_with_pause_and_render_cadence() {
        let run = |fps: usize| {
            let mut s = fixture(true);
            let l = launcher();
            let mut clock = crate::flight::Clock { remainder: 0. };
            let mut tick = 0;
            for frame in 0..fps * 5 {
                if (fps..fps * 2).contains(&frame) {
                    continue;
                }
                for _ in 0..clock.steps(1. / fps as f64) {
                    let command = match tick {
                        0 => Some(Command::ReplaceTarget),
                        1 => Some(Command::Designate),
                        10 | 20 => Some(Command::ToggleArm),
                        40 => Some(Command::ClearDesignation),
                        50 => Some(Command::Designate),
                        60 => Some(Command::FailStation),
                        90 => Some(Command::Jettison),
                        100 => Some(Command::CycleClass),
                        _ => None,
                    };
                    if let Some(command) = command {
                        s.command(0, command, l);
                    }
                    s.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: tick % 30 == 0,
                            launcher: l,
                        }],
                        |_, _| 0.,
                    );
                    tick += 1;
                }
            }
            (tick, format!("{s:?}"))
        };
        assert_eq!(run(30), run(60));
        assert_eq!(run(60), run(144));
    }
    #[test]
    fn cycling_classes_restores_exact_source_category() {
        let mut s = fixture(false);
        s.own_mut().config.target_category = 0x8000;
        s.range_category = s.own().config.target_category;
        for _ in 0..5 {
            s.command(0, Command::CycleClass, launcher());
        }
        assert_eq!(s.range_category, 0x8000);
        assert_eq!(s.targets[0].category, 0x8000);
    }

    #[test]
    fn incoming_contacts_player_ecm_and_replay_are_connected() {
        let mut s = fixture(true);
        let mut l = launcher();
        l.jammer = true;
        s.own_mut().config.ecm.radar_deception_chance = 100;
        s.command(0, Command::Incoming, l);
        s.projectiles[0].position = l.position;
        let ammo = s.own().ammo.clone();
        let mut replay = s.clone();
        let events = s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert!(events.contains(&Event::Defeated(0)));
        assert_eq!(s.own().hp, s.own().config.damage_capacity);
        assert_eq!(s.own().ammo, ammo);
        assert_eq!(
            events,
            replay.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l
                }],
                |_, _| 0.
            )
        );
        assert_eq!(format!("{s:?}"), format!("{replay:?}"));
        s.own_mut().ecm_failed = true; // A powered request cannot bypass equipment failure.
        s.command(0, Command::Incoming, l);
        s.projectiles[0].position = l.position;
        let events = s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { .. }))
        );
        assert!(s.own().hp < s.own().config.damage_capacity);
    }
    #[test]
    fn automatic_station_radar_ecm_failures_keep_mass_and_reset() {
        for index in [36, 37, 38] {
            let mut s = fixture(false);
            s.cheats.damage = crate::cheats::Damage::Realistic;
            s.own_mut().config.system_damage = [0; 45];
            s.own_mut().config.system_damage[index] = 0x1f;
            s.own_mut().config.damage_capacity = 1;
            s.own_mut().hp = 10000;
            let mass = s.own().payload_lbs();
            let mut events = vec![];
            for _ in 0..30 {
                s.damage_own(4, &mut events);
            }
            assert_eq!(s.own().subsystem_counts[index], 1);
            assert_eq!(s.own().last_subsystem, Some(index));
            if index == 36 {
                assert_ne!(s.own().ammo[0] & 0x8000, 0);
            }
            if index == 37 {
                assert!(s.own().radar_failed);
            }
            if index == 38 {
                assert!(s.own().ecm_failed);
            }
            assert_eq!(mass, s.own().payload_lbs());
            let reset = State::new(s.own().config.clone(), true).unwrap();
            assert!(!reset.own().radar_failed && !reset.own().ecm_failed);
            assert_eq!(reset.own().subsystem_counts, [0; 45]);
        }
    }

    #[test]
    fn sequential_launches_keep_their_own_targets_under_one_cockpit_track() {
        let mut s = fixture(true);
        // Fire and forget: the weapon needs support at launch, not after it.
        s.own_mut().config.stations[0].weapon.flags &= !0x200;
        let l = launcher();
        s.targets.push(target(1, [400., 1000., 3000.], 20, 0x80));
        s.targets.push(target(2, [-400., 1000., 3000.], 20, 0x80));
        observe(&mut s, l, 1);
        s.command(0, Command::DesignateTarget(1), l);
        observe(&mut s, l, ACQUISITION);
        assert_eq!(s.own().sensors.acquired(), Some(1));
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
        s.release(0);
        // Selecting the second target releases the first illumination at once.
        s.command(0, Command::DesignateTarget(2), l);
        assert_eq!(s.own().sensors.acquired(), None);
        assert_eq!(s.own_view().readiness(l), Readiness::RadarAcquiring);
        observe(&mut s, l, ACQUISITION);
        assert_eq!(s.own().sensors.acquired(), Some(2));
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
        let targets: Vec<_> = s.projectiles.iter().map(|p| p.target).collect();
        assert_eq!(targets, [Some(1), Some(2)]);
        assert_eq!(s.own().sensors.acquired(), Some(2));
        assert_eq!(s.own_view().designated(), Some(2));
    }
    #[test]
    fn a_continuous_lock_weapon_loses_support_when_the_cockpit_switches_target() {
        let mut s = fixture(true);
        assert!(s.own().config.stations[0].weapon.flags & 0x200 != 0);
        let l = launcher();
        s.targets.push(target(1, [400., 1000., 3000.], 20, 0x80));
        s.targets.push(target(2, [-400., 1000., 3000.], 20, 0x80));
        observe(&mut s, l, 1);
        s.command(0, Command::DesignateTarget(1), l);
        observe(&mut s, l, ACQUISITION);
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
        assert_eq!(s.projectiles[0].target, Some(1));
        s.release(0);
        s.command(0, Command::DesignateTarget(2), l);
        // Designating another contact is never illumination of the first.
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::TrackLost(1))
        );
        assert_eq!(s.projectiles[0].target, None);
    }
    #[test]
    fn a_parked_aircraft_is_not_a_player_radar_contact_until_it_flies() {
        // Manual p.208: grounded aircraft do not appear on enemy radar.
        let mut s = fixture(true);
        let l = launcher();
        s.targets.push(target(1, [0., 1000., 3000.], 20, 0x80));
        s.targets[0].on_ground = true;
        observe(&mut s, l, 1);
        assert!(s.own().sensors.contact(1).is_none());
        s.command(0, Command::DesignateTarget(1), l);
        assert_eq!(s.own_view().designated(), None);
        s.targets[0].on_ground = false;
        observe(&mut s, l, 1);
        assert!(s.own().sensors.contact(1).is_some());
    }
    #[test]
    fn a_destroyed_aircraft_stays_a_contact_until_its_wreck_reaches_the_ground() {
        let mut s = fixture(true);
        let l = launcher();
        s.targets.push(target(1, [0., 1000., 3000.], 20, 0x80));
        observe(&mut s, l, 1);
        s.command(0, Command::DesignateTarget(1), l);
        observe(&mut s, l, ACQUISITION);
        s.targets[0].hp = 0;
        observe(&mut s, l, 1);
        // A wreck remains selectable and shootable while its body is present.
        assert!(s.own().sensors.contact(1).is_some());
        assert_eq!(s.own_view().designated(), Some(1));
        assert_eq!(s.own_view().readiness(l), Readiness::Ready);
        assert!(s.targets[0].airborne);
        for _ in 0..1200 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            if !s.targets[0].airborne {
                break;
            }
        }
        // A grounded wreck ends the air-to-air observation and selection on
        // the next step, since observations are produced before movement.
        assert!(!s.targets[0].airborne);
        s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        assert_eq!(s.targets[0].position[1], 0.);
        assert!(s.own().sensors.contact(1).is_none());
        assert_eq!(s.own_view().designated(), None);
        assert_eq!(s.own().kills, 0);
    }
    #[test]
    fn automatic_radar_failure_inhibits_launch_and_breaks_illumination() {
        let mut s = fixture(true);
        s.cheats.damage = crate::cheats::Damage::Realistic;
        let l = launcher();
        s.range_target(0, l);
        observe(&mut s, l, 1);
        s.designate_next(0, true);
        observe(&mut s, l, ACQUISITION);
        let id = s.own_view().designated().expect("selected fixture target");
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::Fired {
                aircraft: 0,
                station: 0
            })
        );
        s.own_mut().config.system_damage = [0; 45];
        s.own_mut().config.system_damage[37] = 0x1f;
        s.own_mut().config.damage_capacity = 1;
        s.own_mut().hp = 10000;
        let mut events = vec![];
        for _ in 0..30 {
            s.damage_own(4, &mut events);
        }
        assert!(s.own().radar_failed);
        assert_eq!(s.own_view().readiness(l), Readiness::RadarFailed);
        let ammo = s.own().ammo.clone();
        assert!(
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l
                }],
                |_, _| 0.
            )
            .contains(&Event::TrackLost(id))
        );
        assert_eq!(s.own().ammo, ammo);
    }

    fn guided_weapon(state: &State, source: &str, signature: u8) -> Weapon {
        let mut weapon = state.own().config.stations[0].weapon.clone();
        weapon.source = source.into();
        weapon.flags = if matches!(source, "AIM7.JT" | "R530.JT") {
            0x241
        } else {
            0x41
        };
        weapon.seeker.signature = signature;
        for zone in &mut weapon.seeker.zones {
            zone.minimum_range = 0;
            zone.maximum_range = 100_000;
            zone.minimum_altitude = -100_000;
            zone.maximum_altitude = 100_000;
            zone.heading = i16::MAX;
            zone.pitch = i16::MAX;
        }
        weapon
    }

    fn owned_shot(
        weapon: Weapon,
        owner: u32,
        target: u32,
        position: Vector,
        observation: seeker::Observation,
    ) -> Projectile {
        let profile = missiles::Profile::for_weapon(&weapon).unwrap();
        Projectile {
            id: 91,
            owner,
            weapon: Some(weapon.clone()),
            guidance: Some(Flight::from_supported_launch(
                profile,
                LaunchMode::Cued,
                observation,
                position,
            )),
            motion: Some(Motion::new(&weapon.movement, [0., 0., 600.], position[1])),
            guidance_ticks: Some(profile.guidance_ticks),
            age: 0,
            incoming: (target == OWN).then_some(OWN),
            station: 0,
            position,
            previous: position,
            direction: [0., 0., -1.],
            speed_f8: 600 * 256,
            launched_t: 0,
            target: Some(target),
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        }
    }

    #[test]
    fn ai_active_seeker_acquires_player_without_player_sensor_data() {
        let mut state = fixture(true);
        let weapon = guided_weapon(&state, "AIM120.JT", 3);
        let player = launcher();
        let observation = seeker::Observation {
            id: OWN,
            position: player.position,
            velocity: player.velocity,
            quality: 1.,
            off_axis: 0.,
            range: 3_000.,
        };
        state.projectiles.push(owned_shot(
            weapon,
            7,
            OWN,
            [0., player.position[1], 3_000.],
            observation,
        ));
        state.set_actor_supports([ActorSupport {
            owner: 7,
            observation: Some(observation),
            supported: true,
            radar_position: [0., 1000., 5000.],
            radar_emitting: true,
        }]);
        for _ in 0..missiles::DWELL {
            state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: player,
                }],
                |_, _| 0.,
            );
        }
        let flight = state.projectiles[0].guidance.as_ref().unwrap();
        assert_eq!(flight.seeker.target, Some(OWN));
        assert_eq!(flight.seeker.status, Status::Pitbull);
        assert!(state.missile_snapshots(&[(0, player)])[0].radar_acquired);
    }

    #[test]
    fn supported_ai_shot_uses_only_its_owner_support_and_reacquires() {
        let state = fixture(true);
        let weapon = guided_weapon(&state, "R530.JT", 3);
        let target = target(44, [0., 1000., 4000.], 20, 0x80);
        let observation = seeker::Observation {
            id: target.id,
            position: target.position,
            velocity: target.velocity,
            quality: 1.,
            off_axis: 0.,
            range: 4000.,
        };
        let mut shot = owned_shot(weapon.clone(), 7, target.id, [0., 1000., 0.], observation);
        for _ in 0..missiles::DWELL {
            guide_owned(
                &mut shot,
                &weapon,
                std::slice::from_ref(&target),
                std::iter::empty(),
                None,
                &|_, _| false,
                &|_, _| 0.,
            );
        }
        assert!(!shot.guidance.as_ref().unwrap().seeker.acquired);
        let support = ActorSupport {
            owner: 7,
            observation: Some(observation),
            supported: true,
            radar_position: [0., 1000., -1000.],
            radar_emitting: true,
        };
        for _ in 0..missiles::DWELL {
            guide_owned(
                &mut shot,
                &weapon,
                std::slice::from_ref(&target),
                std::iter::empty(),
                Some(&support),
                &|_, _| false,
                &|_, _| 0.,
            );
        }
        assert_eq!(
            shot.guidance.as_ref().unwrap().seeker.status,
            Status::Locked
        );
        guide_owned(
            &mut shot,
            &weapon,
            std::slice::from_ref(&target),
            std::iter::empty(),
            None,
            &|_, _| false,
            &|_, _| 0.,
        );
        assert_eq!(
            shot.guidance.as_ref().unwrap().seeker.status,
            Status::Memory
        );
        for _ in 0..missiles::DWELL {
            guide_owned(
                &mut shot,
                &weapon,
                std::slice::from_ref(&target),
                std::iter::empty(),
                Some(&support),
                &|_, _| false,
                &|_, _| 0.,
            );
        }
        assert_eq!(
            shot.guidance.as_ref().unwrap().seeker.status,
            Status::Locked
        );
    }

    #[test]
    fn active_notch_enters_memory_instead_of_destroying_flight() {
        let state = fixture(true);
        let weapon = guided_weapon(&state, "AIM120.JT", 3);
        let mut crossing = target(5, [0., 0., 60_000.], 20, 0x80);
        crossing.signature.radar = 100.;
        crossing.velocity = [800., 0., 0.];
        let basis = Basis::new(0., 0., 0.);
        assert!(!missiles::active_radar_visible(
            &weapon,
            [0., 5000., 0.],
            basis,
            &crossing,
            crossing.position[1],
        ));
        crossing.velocity = [0., 0., 800.];
        assert!(missiles::active_radar_visible(
            &weapon,
            [0., 5000., 0.],
            basis,
            &crossing,
            crossing.position[1],
        ));
        let profile = missiles::Profile::for_weapon(&weapon).unwrap();
        let mut seeker = seeker::Seeker::new(Some(crossing.id));
        seeker.acquired = true;
        seeker.status = Status::Pitbull;
        seeker.step(profile, &[]);
        assert_eq!(seeker.status, Status::Memory);
        assert_eq!(seeker.target, Some(crossing.id));
    }

    #[test]
    fn snapshots_keep_compatibility_missile_bodies_visible() {
        let mut state = fixture(true);
        let weapon = guided_weapon(&state, "AIM120.JT", 3);
        let observation = seeker::Observation {
            id: 8,
            position: [0., 1000., 5000.],
            velocity: [0.; 3],
            quality: 1.,
            off_axis: 0.,
            range: 5000.,
        };
        let mut projectile = owned_shot(weapon, 8, 8, [0., 1000., 0.], observation);
        projectile.guidance = None;
        projectile.motion = None;
        state.projectiles.push(projectile);
        let snapshots = state.missile_snapshots(&[(0, launcher())]);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].guidance, missiles::Guidance::Active);
        assert!(!snapshots[0].radar_active);
        assert!(!snapshots[0].radar_acquired);
    }

    #[test]
    fn player_supported_snapshot_carries_actual_support() {
        let mut state = fixture(true);
        state.own_mut().config.stations[0].weapon = guided_weapon(&state, "R530.JT", 3);
        let player = launcher();
        state.range_target(0, player);
        observe(&mut state, player, 1);
        state.designate_next(0, true);
        observe(&mut state, player, ACQUISITION);
        let id = state.own_view().designated().unwrap();
        let contact = state.own().sensors.observation(id).unwrap();
        let observation = seeker::Observation {
            id,
            position: contact.position,
            velocity: contact.velocity,
            quality: 1.,
            off_axis: 0.,
            range: missiles::length(sub(contact.position, player.position)),
        };
        state.projectiles.push(owned_shot(
            state.own().config.stations[0].weapon.clone(),
            OWN,
            id,
            player.position,
            observation,
        ));
        let snapshot = state.missile_snapshots(&[(0, player)])[0];
        assert!(snapshot.supported);
        assert_eq!(snapshot.supporting_radar_position, Some(player.position));
    }
}

/// Easy aiming's copy of a player missile: a wider seeker cone and a faster
/// turn. The weapon data itself is unchanged.
fn eased_weapon(w: &Weapon) -> Weapon {
    let scale = |v: i16, k: f64| (f64::from(v) * k).round().clamp(0., f64::from(i16::MAX)) as i16;
    let mut w = w.clone();
    let turn = crate::cheats::EASY_AIMING_TURN;
    w.movement.powered_turn_rate = scale(w.movement.powered_turn_rate, turn);
    w.movement.unpowered_turn_rate = scale(w.movement.unpowered_turn_rate, turn);
    for zone in &mut w.seeker.zones {
        zone.heading = scale(zone.heading, crate::cheats::EASY_AIMING_CONE);
        zone.pitch = scale(zone.pitch, crate::cheats::EASY_AIMING_CONE);
    }
    w
}

fn energy_decoy(
    p: &mut Projectile,
    w: &Weapon,
    target: &Target,
    scene: &missiles::decoys::Scene,
    seed: u64,
    obscured: &dyn Fn(Vector, Vector) -> bool,
) -> Option<DecoyRoll> {
    use missiles::{decoys, energy::FT};
    let flight = p.guidance.as_ref()?;
    if !flight.enabled || !flight.seeker.acquired || flight.seeker.observation.is_none() {
        return None;
    }
    let energy = p.motion.as_mut()?.energy.as_mut()?;
    let kind = match w.seeker.signature {
        2 => decoys::Kind::Flare,
        3 => decoys::Kind::Chaff,
        _ => return None,
    };
    let to_device = |d: &decoys::Device| sub(d.position, energy.flight.position);
    let device = scene
        .devices
        .iter()
        .filter(|d| {
            d.owner == target.id
                && d.kind == kind
                && d.alive
                && (kind != decoys::Kind::Flare || d.age >= 12)
                && energy.decoy.eligible(d.episode)
        })
        .filter(|d| {
            let feet = d.position.map(|v| v / FT);
            let delta = to_device(d);
            let settings = missiles::energy_seeker::Settings::for_weapon(w);
            let line = unit(delta);
            dot(line, p.direction) >= missiles::half_angle(w.seeker.zones[0].heading).cos()
                && dot(line, energy.tracking.head.unwrap_or(p.direction)) >= settings.fov.cos()
                && !obscured(p.position, feet)
        })
        .max_by(|a, b| {
            let signal =
                |d: &decoys::Device| d.strength() / dot(to_device(d), to_device(d)).max(100.);
            signal(a)
                .total_cmp(&signal(b))
                .then_with(|| b.id.cmp(&a.id))
        })
        .copied()?;
    let imaging = tore_formats::missile_data::reference(&w.source).is_some_and(|r| r.imaging);
    let line = unit(sub(target.position, p.position));
    let probability = if kind == decoys::Kind::Flare {
        let facing = dot(target.basis.forward, line.map(|v| -v));
        let heat = target.signature.infrared.max(0.) / 100.
            * target.heat.factor()
            * (0.7 - 0.3 * facing.clamp(-1., 1.));
        let distance = missiles::length(sub(target.position, p.position)) * FT;
        let target_signal = heat / distance.powi(2).max(100.);
        let flare_signal =
            device.strength() / dot(to_device(&device), to_device(&device)).max(100.);
        let generation = if imaging {
            0.08
        } else if !missiles::energy_seeker::all_aspect(w) {
            1.
        } else {
            0.6
        };
        decoys::infrared_probability(
            w.seeker.chaff_flare_chance,
            device.effectiveness,
            flare_signal / target_signal.max(1e-15),
            generation,
            imaging,
        )
    } else {
        let clutter = (-line[1] / 5f64.to_radians().sin()).clamp(0., 1.);
        let notch = decoys::notch(target.velocity.map(|v| v * FT), line, clutter);
        let generation = if flight.profile.guidance == Guidance::Active {
            0.35
        } else if w.seeker.chaff_flare_chance > 100 {
            1.
        } else {
            0.7
        };
        decoys::radar_probability(
            w.seeker.chaff_flare_chance,
            device.effectiveness,
            notch,
            generation,
        )
    };
    let (threshold, bound) = decoys::chance_units(kind, probability);
    let random =
        decoys::mix(seed ^ decoys::mix(u64::from(p.id)) ^ decoys::mix(device.episode)) >> 11;
    let roll = (random as f64 / ((1u64 << 53) as f64) * f64::from(bound)).floor() as u16;
    let decoyed =
        energy
            .decoy
            .evaluate(device, f64::from(threshold) / f64::from(bound), seed, p.id)?;
    if decoyed {
        energy.target = Some(device.observation());
        energy.flight.terminal = true;
        energy.flight.waypoint = None;
        energy.body.cruise_feet = None;
        p.target = None;
        let flight = p.guidance.as_mut().unwrap();
        flight.unguided = true;
        flight.seeker.observation = None;
        flight.seeker.acquired = false;
        flight.seeker.target = None;
        flight.seeker.status = Status::Lost;
    }
    Some(DecoyRoll {
        aircraft: target.id,
        projectile: p.id,
        kind: if kind == decoys::Kind::Flare {
            EffectKind::Flare
        } else {
            EffectKind::Chaff
        },
        device: device.id,
        susceptibility: w.seeker.chaff_flare_chance,
        effectiveness: device.effectiveness,
        threshold,
        bound,
        roll,
        decoyed,
    })
}

/// John's permanent own-launcher exclusion includes powered A-G guidance.
fn launcher_immune(w: &Weapon) -> bool {
    w.flags & 1 != 0 && !super::guided::is_glide_bomb(w)
}

/// One tick of actor-owned guidance, shared with offline calibration probes.
pub fn guide_owned<'a>(
    p: &mut Projectile,
    w: &Weapon,
    targets: &'a [Target],
    others: impl Iterator<Item = &'a Target> + Clone,
    support: Option<&ActorSupport>,
    obscured: &dyn Fn(Vector, Vector) -> bool,
    ground: &dyn Fn(f64, f64) -> f64,
) {
    if let Some(flight) = &mut p.guidance
        && (p.target == Some(p.owner)
            || flight.seeker.target == Some(p.owner)
            || flight.seeker.observation.is_some_and(|o| o.id == p.owner))
    {
        flight.seeker.target = None;
        flight.seeker.candidate = None;
        flight.seeker.observation = None;
        flight.seeker.acquired = false;
        flight.last_intercept = None;
        flight.solution = None;
        p.target = None;
        if let Some(energy) = p.motion.as_mut().and_then(|m| m.energy.as_mut()) {
            energy.target = None;
        }
    }
    if let Some(energy) = p.motion.as_mut().and_then(|motion| motion.energy.as_mut()) {
        if let Some(pursuit) = &mut energy.decoy.pursuit {
            pursuit.device.step(energy.wind, &|x, z| {
                ground(x / missiles::energy::FT, z / missiles::energy::FT) * missiles::energy::FT
            });
            energy.target = pursuit.target();
            p.target = None;
            let flight = p.guidance.as_mut().unwrap();
            flight.unguided = true;
            flight.seeker.observation = None;
            return;
        }
        if energy.flight.lost {
            p.target = None;
            let flight = p.guidance.as_mut().unwrap();
            flight.unguided = true;
            flight.seeker.observation = None;
            flight.seeker.status = Status::Lost;
            return;
        }
    }
    let flight = p.guidance.as_mut().unwrap();
    let profile = flight.profile;
    let support = if p.motion.is_some_and(|m| m.energy.is_some())
        && !matches!(profile.guidance, Guidance::Active | Guidance::Supported)
    {
        None
    } else {
        support
    };
    if flight.unguided {
        return;
    }
    let guidance_end = p
        .motion
        .and_then(|m| m.energy)
        .map_or(p.guidance_ticks.unwrap_or(profile.guidance_ticks), |e| {
            u64::from(e.body.guidance_ticks)
        });
    if p.age >= guidance_end {
        flight.seeker.status = Status::Expired;
        flight.seeker.observation = None;
        return;
    }
    let supported = flight
        .seeker
        .target
        .or_else(|| {
            (profile.guidance == Guidance::Supported)
                .then(|| support.and_then(|s| s.observation.map(|o| o.id)))
                .flatten()
        })
        .filter(|id| *id != p.owner)
        .filter(|id| {
            support.is_some_and(|answer| {
                answer.supported
                    && answer.radar_emitting
                    && answer.observation.is_some_and(|o| o.id == *id)
            })
        })
        .filter(|id| {
            targets
                .iter()
                .chain(others.clone())
                .find(|t| t.id == *id)
                .is_some_and(|t| flight.eligible(w, t))
        })
        .and_then(|id| {
            support
                .and_then(|answer| answer.observation)
                .filter(|o| o.id == id)
        });
    // Cued surface shots retain the A-G permanent-loss policy. An uncued
    // player SARH release waits for later illumination instead.
    if profile.rides_air_ground() && flight.mode == LaunchMode::Cued && supported.is_none() {
        agm::blind(p);
        return;
    }
    let flight = p.guidance.as_mut().unwrap();
    // An anti-radiation surface missile keeps homing on its acquired object
    // after the radar stops, on angles only, while the object stays inside
    // its zone-1 cone (retail data).
    let held = flight
        .seeker
        .target
        .filter(|_| profile.holds_on_angles() && flight.seeker.acquired);
    // Only shared supported observations may update an initially silent shot.
    if !flight.seeker.acquired
        && (p.motion.is_none_or(|m| m.energy.is_none()) || !flight.enabled)
        && let Some(contact) = supported
        && (flight.last_intercept.is_none() || p.age.is_multiple_of(12))
    {
        if p.motion.is_some_and(|m| m.energy.is_some()) {
            flight.solution = None;
            flight.last_intercept = Some(contact.position);
        } else {
            flight.solution = Some(missiles::lead(
                p.position,
                missiles::length(p.motion.unwrap().velocity),
                contact.position,
                contact.velocity,
            ));
            flight.last_intercept = Some(flight.solution.map_or(contact.position, |s| s.point));
        }
    }
    if profile.guidance == Guidance::Active && !flight.enabled {
        flight.enabled = flight
            .last_intercept
            .zip(profile.activation_ft)
            .is_some_and(|(point, distance)| missiles::length(sub(point, p.position)) <= distance);
    }
    if flight.enabled {
        let cap = (flight.mode == LaunchMode::Boresight
            && !flight.seeker.acquired
            && !matches!(profile.guidance, Guidance::Active | Guidance::Supported))
        .then(|| profile.search_cap());
        let basis = Basis::new(
            p.direction[0].atan2(p.direction[2]),
            p.direction[1].atan2(p.direction[0].hypot(p.direction[2])),
            0.,
        );
        let view = seeker::View {
            position: p.position,
            basis,
            cap,
            obscured,
        };
        let mut observations: Vec<_> = targets
            .iter()
            .chain(others)
            .filter(|t| t.id != p.owner && flight.eligible(w, t))
            .filter(|t| {
                profile.guidance != Guidance::Supported || supported.is_some_and(|o| o.id == t.id)
            })
            .filter_map(|t| {
                let observed = (if p.motion.is_some_and(|m| m.energy.is_some()) {
                    seeker::observe_energy(w, profile, &view, t)
                } else {
                    seeker::observe(w, profile, &view, t)
                })
                .or_else(|| {
                    (held == Some(t.id)
                        && agm::inside_angles(&w.seeker.zones[1], view.position, basis, t.position))
                    .then(|| {
                        let delta = sub(t.position, view.position);
                        seeker::Observation {
                            id: t.id,
                            position: t.position,
                            velocity: t.velocity,
                            quality: 1.,
                            off_axis: dot(unit(delta), basis.forward).clamp(-1., 1.).acos(),
                            range: missiles::length(delta),
                        }
                    })
                })?;
                if profile.guidance == Guidance::Supported {
                    supported
                } else if profile.guidance == Guidance::Active
                    && !missiles::active_radar_visible(
                        w,
                        p.position,
                        basis,
                        t,
                        t.position[1] - ground(t.position[0], t.position[2]),
                    )
                {
                    None
                } else {
                    Some(observed)
                }
            })
            .collect();
        if let Some(energy) = p.motion.as_mut().and_then(|m| m.energy.as_mut()) {
            let candidate = flight
                .seeker
                .target
                .or(flight.seeker.candidate)
                .and_then(|id| observations.iter().find(|o| o.id == id))
                .or_else(|| {
                    observations
                        .iter()
                        .min_by(|a, b| seeker::compare_returns(a, b, profile))
                })
                .copied();
            if let Some(o) = candidate {
                let line = unit(sub(o.position, p.position));
                let radar = profile.role == TargetRole::Aircraft
                    && matches!(profile.guidance, Guidance::Active | Guidance::Supported);
                let clutter = if radar {
                    (-line[1] / 5f64.to_radians().sin()).clamp(0., 1.)
                } else {
                    0.
                };
                let notch = missiles::decoys::notch(
                    o.velocity.map(|v| v * missiles::energy::FT),
                    line,
                    clutter,
                );
                let settings = missiles::energy_seeker::Settings::for_weapon(w);
                if energy.tracking.observes(
                    p.direction,
                    line,
                    notch,
                    settings.rate,
                    settings.fov,
                    settings.gimbal,
                ) {
                    observations.retain(|contact| contact.id == o.id);
                } else {
                    observations.clear();
                }
            }
        }
        if profile.guidance == Guidance::Supported && !observations.is_empty() {
            flight.seeker.target = supported.map(|o| o.id);
            flight.seeker.candidate = flight.seeker.target;
            flight.seeker.dwell = missiles::DWELL;
        }
        flight.seeker.step(profile, &observations);
        if flight.seeker.acquired && flight.seeker.observation.is_some() {
            flight.qualified_target = flight.seeker.target;
        }
        p.target = flight.seeker.target;
        if let Some(o) = flight
            .seeker
            .observation
            .filter(|_| matches!(flight.seeker.status, Status::Locked | Status::Pitbull))
            && (flight.last_intercept.is_none() || p.age.is_multiple_of(12))
        {
            if p.motion.is_some_and(|m| m.energy.is_some()) {
                flight.solution = None;
                flight.last_intercept = Some(o.position);
            } else {
                flight.solution = Some(missiles::lead(
                    p.position,
                    missiles::length(p.motion.unwrap().velocity),
                    o.position,
                    o.velocity,
                ));
                flight.last_intercept = Some(flight.solution.map_or(o.position, |s| s.point));
            }
        }
    } else {
        flight.seeker.status = Status::Midcourse;
    }
    // Remembered intercept is frozen on loss. Reacquisition may continue for the
    // full guidance lifetime, per John's 2026-09-17 revision.
    let can_steer = profile.guidance != Guidance::Supported || supported.is_some();
    if can_steer && let Some(point) = flight.last_intercept {
        let motion = p.motion.unwrap();
        // Keep searching for the same target, but do not turn back around a
        // stale point after passing it. A fresh observation can guide again.
        let observation = if motion.energy.is_some() && flight.enabled {
            flight
                .seeker
                .observation
                .filter(|_| matches!(flight.seeker.status, Status::Locked | Status::Pitbull))
        } else {
            flight.seeker.observation.or(supported)
        };
        let measured = observation.is_some();
        if !measured && dot(sub(point, p.position), motion.velocity) <= 0. {
            return;
        }
        if motion.energy.is_some() {
            let (position, velocity) =
                observation.map_or((point, [0.; 3]), |o| (o.position, o.velocity));
            let motion = p.motion.as_mut().unwrap();
            motion.track(position, velocity);
            motion.energy.as_mut().unwrap().observed = observation.is_some();
            return;
        }
        let desired = unit(sub(
            motion.aim_over_terrain(p.position, point, ground),
            p.position,
        ));
        let heading = missiles::commanded_heading(p.direction, motion.path(), desired);
        p.direction = missiles::steer(&w.movement, p.age, p.direction, heading);
    }
}

#[cfg(test)]
fn guide(
    p: &mut Projectile,
    w: &Weapon,
    targets: &[Target],
    sensors: &Sensors,
    obscured: &dyn Fn(Vector, Vector) -> bool,
) {
    let support = p
        .guidance
        .as_ref()
        .and_then(|flight| flight.seeker.target)
        .map(|id| ActorSupport {
            owner: p.owner,
            supported: sensors.supports(id),
            observation: sensors.observation(id).map(|contact| seeker::Observation {
                id,
                position: contact.position,
                velocity: contact.velocity,
                quality: 1.,
                off_axis: 0.,
                range: missiles::length(sub(contact.position, p.position)),
            }),
            radar_position: [0.; 3],
            radar_emitting: true,
        });
    guide_owned(
        p,
        w,
        targets,
        std::iter::empty(),
        support.as_ref(),
        obscured,
        &|_, _| 0.,
    );
}

#[cfg(test)]
#[path = "missile_tests.rs"]
mod missile_tests;

/// Two ownships in one synthetic scene: each is an aircraft to the other.
#[cfg(test)]
mod pair_tests {
    use super::tests::fixture;
    use super::*;

    /// Ownship 0 faces north at the origin; ownship 1 faces it, `gap` feet north.
    fn pair(gap: f64) -> (State, [Launcher; 2]) {
        pair_of_sides(gap, Side(2))
    }
    /// The same scene, with ownship 1 flying for `side` (ownship 0 is on side 1).
    fn pair_of_sides(gap: f64, side: Side) -> (State, [Launcher; 2]) {
        let mut s = fixture(false);
        let config = s.own().configuration().clone();
        s.add_ownship(Ownship::new(1, side, config, true).unwrap())
            .unwrap();
        let facing = |position: Vector, yaw: f64| Launcher {
            position,
            basis: Basis::new(yaw, 0., 0.),
            speed_fps: 300.,
            velocity: Basis::new(yaw, 0., 0.).forward.map(|v| v * 300.),
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: sensors::Controls::default(),
        };
        let launchers = [
            facing([0., 1000., 0.], 0.),
            facing([0., 1000., gap], std::f64::consts::PI),
        ];
        (s, launchers)
    }
    fn step(s: &mut State, launchers: &[Launcher; 2], held: [bool; 2]) -> Vec<Event> {
        let inputs: Vec<_> = (0..2)
            .map(|n| OwnshipInput {
                aircraft: n as u32,
                held: held[n],
                launcher: launchers[n],
            })
            .collect();
        s.step(&inputs, |_, _| 0.)
    }

    #[test]
    fn each_ownship_detects_the_other_and_neither_detects_itself() {
        let (mut s, l) = pair(6000.);
        for _ in 0..200 {
            step(&mut s, &l, [false; 2]);
        }
        let seen = |aircraft: u32, id: u32| {
            s.ownship(aircraft)
                .unwrap()
                .sensors
                .observation(id)
                .is_some()
        };
        assert!(seen(0, 1), "ownship 0 must see ownship 1");
        assert!(seen(1, 0), "ownship 1 must see ownship 0");
        assert!(!seen(0, 0) && !seen(1, 1));
        // Designating the other shows it as the cockpit's target.
        s.command(0, Command::DesignateTarget(1), l[0]);
        assert_eq!(
            s.view(0).unwrap().display_target().map(|t| t.id),
            Some(1),
            "the other ownship is a contact of the cockpit"
        );
        assert!(s.view(1).unwrap().display_target().is_none());
    }

    #[test]
    fn human_visual_pick_of_a_remote_friendly_survives_without_radar() {
        let (mut s, mut l) = pair_of_sides(6000., Side(1));
        l[0].radar = false;
        for _ in 0..2 {
            step(&mut s, &l, [false; 2]);
        }
        assert!(s.own().sensors.contacts().is_empty());
        assert!(s.own().sensors.visual().iter().any(|c| c.id == 1));
        s.command(0, Command::DesignateVisualTarget(1), l[0]);
        for _ in 0..120 {
            step(&mut s, &l, [false; 2]);
        }
        assert_eq!(s.own().designated(), Some(1));
        assert_eq!(s.own().sensors.acquired(), None);
        assert_eq!(s.view(0).unwrap().display_target().map(|t| t.id), Some(1));
        s.command(0, Command::DesignateVisualTarget(999), l[0]);
        assert_eq!(
            s.own().designated(),
            Some(1),
            "invalid picks leave the selection alone"
        );
        l[1].position[2] = 200. * 6076.;
        step(&mut s, &l, [false; 2]);
        assert_eq!(s.own().designated(), None);
    }

    #[test]
    fn a_gun_round_from_one_ownship_hits_the_other_and_never_its_shooter() {
        let (mut s, l) = pair(600.);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        let mut events = Vec::new();
        for _ in 0..300 {
            events.extend(step(&mut s, &l, [true, false]));
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 1, .. })),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 0, .. }))
        );
        assert_eq!(
            s.ownship(0).unwrap().hp,
            s.ownship(0).unwrap().config.damage_capacity
        );
        assert!(s.ownship(1).unwrap().hp < s.ownship(1).unwrap().config.damage_capacity);
        let strikes = s.take_strikes();
        assert!(strikes.iter().all(|k| k.owner == 0 && k.victim == 1));
        assert!(!strikes.is_empty());
        assert!(s.ownship(0).unwrap().shots > 0);
    }

    #[test]
    fn the_shooter_is_credited_with_an_ownship_it_shoots_down() {
        let (mut s, l) = pair(600.);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        // Ownship 1 is one hit from destruction.
        s.ownship_mut(1).unwrap().hp = 1;
        for _ in 0..300 {
            step(&mut s, &l, [true, false]);
        }
        assert_eq!(s.ownship(1).unwrap().hp, 0);
        let kills = s.ledger.kills();
        assert_eq!(kills.len(), 1, "{kills:?}");
        assert_eq!((kills[0].owner, kills[0].victim), (0, 1));
        assert!(kills[0].aircraft);
        // A hit that does not destroy it is the last hit, credited if the
        // aircraft is lost another way.
        let (mut s, l) = pair(600.);
        s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
        for _ in 0..300 {
            step(&mut s, &l, [true, false]);
        }
        assert!(s.ownship(1).unwrap().hp > 0);
        assert!(s.ledger.kills().is_empty());
        let credit = s.ledger.credit(1).expect("the last shooter to hit it");
        assert_eq!((credit.owner, credit.victim), (0, 1));
    }

    #[test]
    fn with_friendly_fire_off_ownships_of_one_side_spare_each_other() {
        for (setting, side, hurt) in [
            (FriendlyFire::On, Side(1), true),
            (FriendlyFire::Off, Side(1), false),
            (FriendlyFire::Off, Side(2), true),
        ] {
            let (mut s, l) = pair_of_sides(600., side);
            s.friendly_fire = setting;
            s.own_mut().config.stations[0].weapon.source = "M61.JT".into();
            let mut events = Vec::new();
            for _ in 0..300 {
                events.extend(step(&mut s, &l, [true, false]));
            }
            let hit = events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 1, .. }));
            assert_eq!(hit, hurt, "{setting:?} {side:?}");
        }
    }

    #[test]
    fn a_midair_between_two_ownships_destroys_both() {
        let (mut s, l) = pair(30.);
        let events = step(&mut s, &l, [false; 2]);
        for aircraft in [0, 1] {
            assert!(
                events.contains(&Event::OwnshipDestroyed { aircraft }),
                "{events:?}"
            );
            assert_eq!(s.ownship(aircraft).unwrap().hp, 0);
        }
    }

    #[test]
    fn invulnerable_ownships_survive_a_midair() {
        let (mut s, l) = pair(30.);
        s.cheats.damage = crate::cheats::Damage::Invulnerable;
        let events = step(&mut s, &l, [false; 2]);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::OwnshipDestroyed { .. }))
        );
    }
}

/// The hit rule: a gun round can hit any aircraft but its shooter; a missile
/// or bomb can hit any aircraft once its fuze has armed, its launcher included.
#[cfg(test)]
mod hit_rule_tests {
    use super::tests::{fixture, target};
    use super::*;

    fn launcher() -> Launcher {
        Launcher {
            position: [0., 1000., 0.],
            basis: Basis::new(0., 0., 0.),
            speed_fps: 300.,
            velocity: [0., 0., 300.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: sensors::Controls::default(),
        }
    }
    /// An unguided round of the fixture's missile record, already armed
    /// unless `arms_after` says otherwise.
    fn shell(
        state: &State,
        owner: u32,
        from: Vector,
        toward: Vector,
        aimed_at: Option<u32>,
        arms_after: u16,
    ) -> Projectile {
        let mut weapon = state.own().config.stations[0].weapon.clone();
        weapon.damage.fuze_arm_t = arms_after;
        weapon.seeker.signature = 0;
        weapon.flags = 0x14;
        let direction = unit(sub(toward, from));
        Projectile {
            id: 900 + owner,
            owner,
            weapon: Some(weapon),
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: aimed_at,
            station: 0,
            position: from,
            previous: from,
            direction,
            speed_f8: 1200 * 256,
            launched_t: 0,
            target: None,
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        }
    }
    fn run(s: &mut State, ticks: usize) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..ticks {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            ));
        }
        events
    }
    fn scene() -> State {
        let mut s = fixture(true);
        s.targets.clear();
        s
    }
    fn damaged(events: &[Event]) -> bool {
        events
            .iter()
            .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 0, .. }))
    }

    #[test]
    fn guns_and_missiles_hit_wrecks_once_without_another_kill() {
        for gun in [false, true] {
            for present in [false, true] {
                for ownship in [false, true] {
                    let mut s = scene();
                    s.friendly_fire = FriendlyFire::Off;
                    let victim = if ownship { 0 } else { 5 };
                    if ownship {
                        s.own_mut().hp = 0;
                    } else {
                        let mut wreck = target(victim, [0., 1000., 0.], 100, 0x80);
                        wreck.hp = 0;
                        wreck.airborne = present;
                        wreck.side = s.own().side;
                        s.targets.push(wreck);
                    }
                    let owner = if ownship { 7 } else { 0 };
                    let mut round = shell(
                        &s,
                        owner,
                        [0., 1000., 100.],
                        [0., 1000., 0.],
                        Some(victim),
                        0,
                    );
                    if gun {
                        round.weapon.as_mut().unwrap().source = "M61.JT".into();
                    }
                    s.projectiles.push(round);
                    let mut input = launcher();
                    if ownship {
                        input.alive = false;
                        input.body_present = present;
                    } else {
                        input.position[0] = 10000.;
                    }
                    let mut events = Vec::new();
                    for _ in 0..30 {
                        events.extend(s.step(
                            &[OwnshipInput {
                                aircraft: 0,
                                held: false,
                                launcher: input,
                            }],
                            |_, _| 0.,
                        ));
                    }
                    let context = format!("gun={gun} present={present} ownship={ownship}");
                    assert_eq!(
                        events.iter().filter(|e| **e == Event::Hit(victim)).count(),
                        usize::from(present),
                        "{context}: {events:?}"
                    );
                    assert_eq!(s.projectiles.is_empty(), present, "{context}");
                    assert!(s.ledger.kills().is_empty(), "{context}");
                    assert!(s.history.is_empty(), "{context}");
                    assert!(
                        !events.iter().any(|e| matches!(
                            e,
                            Event::Destroyed(_)
                                | Event::OwnshipDestroyed { .. }
                                | Event::OwnshipDamaged { .. }
                                | Event::Jolt(_)
                        )),
                        "{context}: {events:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_missile_aimed_at_someone_else_hits_the_ownship_in_its_path() {
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        let round = shell(&s, 5, [0., 1000., 300.], [0., 1000., 0.], None, 0);
        s.projectiles.push(round);
        let events = run(&mut s, 30);
        assert!(damaged(&events), "{events:?}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Jolt(Jolt { target: 0, .. })))
        );
    }

    #[test]
    fn a_missile_aimed_at_the_ownship_hits_another_aircraft_in_its_path() {
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        s.targets.push(target(6, [0., 1000., 600.], 100, 0x80));
        // Fired by 5 at the ownship, it meets 6 first.
        let round = shell(&s, 5, [0., 1000., 850.], [0., 1000., 0.], Some(0), 0);
        s.projectiles.push(round);
        let events = run(&mut s, 30);
        assert!(events.contains(&Event::Hit(6)), "{events:?}");
        assert!(!damaged(&events));
    }

    #[test]
    fn a_decoyed_missile_no_longer_keeps_its_hit_for_the_ownship() {
        let mut s = scene();
        s.targets.push(target(6, [0., 1000., 600.], 100, 0x80));
        // Aimed at the ownship, decoyed away (no target left), and the wingman
        // is in the way.
        let round = shell(&s, 5, [0., 1000., 900.], [0., 1000., 0.], Some(0), 0);
        s.projectiles.push(round);
        s.projectiles[0].target = None;
        let events = run(&mut s, 30);
        assert!(events.contains(&Event::Hit(6)), "{events:?}");
        assert!(!damaged(&events));
    }

    /// Flies `round` one tick, resolves it as spoofed the way the decoy
    /// code does, then lets it coast on: it keeps flying but no longer seeks.
    fn decoyed_then_coasting(s: &mut State, mut round: Projectile) -> Vec<Event> {
        round.target = None;
        let id = round.id;
        s.projectiles.push(round);
        let mut events = run(s, 1);
        s.ledger.resolve(id, Resolution::Spoofed);
        events.extend(run(s, 30));
        events
    }

    #[test]
    fn a_decoyed_missile_that_kills_an_aircraft_after_all_is_a_recorded_hit() {
        let mut s = scene();
        s.targets.push(target(6, [0., 1000., 900.], 5, 0x8000));
        let round = shell(&s, 0, [0., 1000., 600.], [0., 1000., 900.], Some(6), 0);
        let events = decoyed_then_coasting(&mut s, round);
        assert!(events.contains(&Event::Destroyed(6)), "{events:?}");
        // The kill is backed by a hit, with its damage, and the spoof is
        // withdrawn: the missile resolved once.
        let tally = s.ledger.total(|k| k.owner == 0);
        assert_eq!(
            (tally.launched, tally.hit, tally.spoofed, tally.failed()),
            (1, 1, 0, 0)
        );
        assert_eq!(tally.damage, 5);
        let kills = s.ledger.kills();
        assert_eq!(kills.len(), 1, "{kills:?}");
        assert_eq!((kills[0].owner, kills[0].victim), (0, 6));
        assert_eq!(s.own().hits, 1);
        // The wreck is not hit again for another kill or another hit.
        let events = run(&mut s, 30);
        assert!(!events.contains(&Event::Destroyed(6)));
        assert_eq!(s.ledger.kills().len(), 1);
    }

    #[test]
    fn a_decoyed_missile_that_only_damages_an_aircraft_is_its_last_attacker() {
        let mut s = scene();
        s.targets.push(target(6, [0., 1000., 900.], 500, 0x8000));
        let round = shell(&s, 0, [0., 1000., 600.], [0., 1000., 900.], Some(6), 0);
        decoyed_then_coasting(&mut s, round);
        assert!(s.targets[0].hp < 500);
        let tally = s.ledger.total(|k| k.owner == 0);
        assert_eq!((tally.hit, tally.spoofed), (1, 0));
        assert!(s.ledger.kills().is_empty());
        // Lost another way, the aircraft goes to the shooter whose recorded
        // hit damaged it.
        let credit = s.ledger.credit(6).expect("the hit that damaged it");
        assert_eq!((credit.owner, credit.victim), (0, 6));
    }

    #[test]
    fn a_decoyed_missile_that_kills_the_ownship_is_a_recorded_hit() {
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x8000));
        s.ownship_mut(0).unwrap().hp = 1;
        let round = shell(&s, 5, [0., 1000., 300.], [0., 1000., 0.], Some(0), 0);
        let events = decoyed_then_coasting(&mut s, round);
        assert!(events.contains(&Event::OwnshipDestroyed { aircraft: 0 }));
        let tally = s.ledger.total(|k| k.owner == 5);
        assert_eq!((tally.launched, tally.hit, tally.spoofed), (1, 1, 0));
        let kills = s.ledger.kills();
        assert_eq!(kills.len(), 1, "{kills:?}");
        assert_eq!((kills[0].owner, kills[0].victim), (5, 0));
    }

    #[test]
    fn a_decoyed_missile_that_only_touches_a_wreck_stays_spoofed() {
        let mut s = scene();
        let mut wreck = target(6, [0., 1000., 900.], 100, 0x8000);
        wreck.hp = 0;
        s.targets.push(wreck);
        let round = shell(&s, 0, [0., 1000., 600.], [0., 1000., 900.], Some(6), 0);
        decoyed_then_coasting(&mut s, round);
        let tally = s.ledger.total(|k| k.owner == 0);
        assert_eq!((tally.hit, tally.spoofed), (0, 1));
        assert!(s.ledger.kills().is_empty());
        assert!(s.ledger.credit(6).is_none());
    }

    #[test]
    fn a_hit_that_does_no_damage_is_a_hit_but_not_the_last_attacker() {
        let mut s = scene();
        s.targets.push(target(6, [0., 1000., 900.], 100, 0x8000));
        let mut round = shell(&s, 0, [0., 1000., 600.], [0., 1000., 900.], Some(6), 0);
        round.weapon.as_mut().unwrap().damage.by_class = Default::default();
        s.projectiles.push(round);
        run(&mut s, 30);
        assert_eq!(s.targets[0].hp, 100);
        assert_eq!(s.ledger.total(|k| k.owner == 0).hit, 1);
        assert!(s.ledger.credit(6).is_none());
        assert!(s.ledger.kills().is_empty());
    }

    #[test]
    fn a_missile_can_hit_its_own_launcher_only_once_armed() {
        for (arms_after, hits) in [(0, true), (2, false)] {
            let mut s = scene();
            // The ownship's own missile doubles back over it.
            let round = shell(&s, 0, [0., 1000., 200.], [0., 1000., 0.], None, arms_after);
            s.projectiles.push(round);
            let events = run(&mut s, 40);
            assert_eq!(
                damaged(&events),
                hits,
                "armed after {arms_after}: {events:?}"
            );
            // An AI aircraft's missile can too.
            let mut s = scene();
            s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
            let round = shell(
                &s,
                5,
                [0., 1000., 1100.],
                [0., 1000., 900.],
                None,
                arms_after,
            );
            s.projectiles.push(round);
            let events = run(&mut s, 40);
            assert_eq!(
                events.contains(&Event::Hit(5)),
                hits,
                "armed after {arms_after}: {events:?}"
            );
        }
    }

    #[test]
    fn a_round_leaving_its_own_launcher_is_not_a_hit() {
        // Real rockets have no arming delay and start inside the aircraft's
        // volume; they must not explode on the pylon.
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        let from = [0., 1000., 0.];
        let round = shell(&s, 0, from, [0., 1000., 100.], None, 0);
        s.projectiles.push(round);
        let round = shell(&s, 5, [0., 1000., 900.], [0., 1000., 1000.], None, 0);
        s.projectiles.push(round);
        let events = run(&mut s, 30);
        assert!(
            !damaged(&events) && !events.contains(&Event::Hit(5)),
            "{events:?}"
        );
    }

    #[test]
    fn a_gun_round_never_hits_its_shooter() {
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        for (owner, from) in [(0, [0., 1000., 200.]), (5, [0., 1000., 1100.])] {
            let mut round = shell(&s, owner, from, [0., 1000., 1000. - from[2]], None, 0);
            round.weapon.as_mut().unwrap().source = "M61.JT".into();
            s.projectiles.push(round);
        }
        // Each round flies back over its own aircraft.
        s.projectiles[0].direction = [0., 0., -1.];
        s.projectiles[1].direction = [0., 0., -1.];
        let events = run(&mut s, 30);
        assert!(!damaged(&events), "{events:?}");
        assert!(!events.contains(&Event::Hit(5)), "{events:?}");
    }

    #[test]
    fn damage_follows_the_pilot_of_the_aircraft_hit() {
        // A hit on an ownship takes twice the aircraft's hit points, spread
        // 80 to 119 percent, and reports an ownship event; the same weapon on
        // an AI row takes the plain damage and reports a hit.
        let mut s = scene();
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        s.targets.push(target(6, [0., 1000., 1500.], 100, 0x80));
        let round = shell(&s, 5, [0., 1000., 300.], [0., 1000., 0.], None, 0);
        s.projectiles.push(round);
        let mut round = shell(&s, 5, [0., 1000., 1400.], [0., 1000., 1500.], None, 0);
        // Rounds in flight have their own numbers: a bomb's path is kept by it.
        round.id += 1;
        s.projectiles.push(round);
        let events = run(&mut s, 30);
        assert!(
            damaged(&events) && events.contains(&Event::Hit(6)),
            "{events:?}"
        );
        let capacity = s.own().config.damage_capacity;
        assert!(s.own().hp < capacity);
        let ai = s.targets.iter().find(|t| t.id == 6).unwrap();
        assert_eq!(ai.hp, 100 - 10, "the AI row takes the unrolled damage");
        // Invulnerable still spares the ownship, not the AI row.
        let mut s = scene();
        s.cheats.damage = crate::cheats::Damage::Invulnerable;
        s.targets.push(target(5, [0., 1000., 900.], 100, 0x80));
        let round = shell(&s, 5, [0., 1000., 300.], [0., 1000., 0.], None, 0);
        s.projectiles.push(round);
        let events = run(&mut s, 30);
        assert!(!damaged(&events));
        assert_eq!(s.own().hp, capacity);
    }
}

/// The friendly-fire setting: with it off no round damages an aircraft of its
/// shooter's side, the shooter included; collisions stay as they are.
#[cfg(test)]
mod friendly_fire_tests {
    use super::tests::{fixture, target};
    use super::*;

    fn launcher() -> Launcher {
        Launcher {
            position: [0., 1000., 0.],
            basis: Basis::new(0., 0., 0.),
            speed_fps: 300.,
            velocity: [0., 0., 300.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: sensors::Controls::default(),
        }
    }
    /// A missile record round from `owner`, armed, flying from `from` to `toward`.
    fn shell(state: &State, owner: u32, from: Vector, toward: Vector) -> Projectile {
        let mut weapon = state.own().config.stations[0].weapon.clone();
        weapon.seeker.signature = 0;
        weapon.flags = 0x14;
        Projectile {
            id: 900 + owner,
            owner,
            weapon: Some(weapon),
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: None,
            station: 0,
            position: from,
            previous: from,
            direction: unit(sub(toward, from)),
            speed_f8: 1200 * 256,
            launched_t: 0,
            target: None,
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        }
    }
    fn run(s: &mut State, ticks: usize) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..ticks {
            events.extend(s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: launcher(),
                }],
                |_, _| 0.,
            ));
        }
        events
    }
    /// The ownship (side 1) and AI aircraft 5 on `side` ahead of it; a round
    /// from 5 heads back toward the ownship, and one from the ownship toward 5.
    fn scene(setting: FriendlyFire, side: Side) -> State {
        let mut s = fixture(true);
        s.targets.clear();
        let mut ai = target(5, [0., 1000., 900.], 100, 0x80);
        ai.side = side;
        s.targets.push(ai);
        s.friendly_fire = setting;
        let toward_ai = shell(&s, 0, [0., 1000., 300.], [0., 1000., 900.]);
        let toward_own = shell(&s, 5, [0., 1000., 600.], [0., 1000., 0.]);
        s.projectiles.extend([toward_ai, toward_own]);
        s
    }
    fn hurt_ownship(events: &[Event]) -> bool {
        events
            .iter()
            .any(|e| matches!(e, Event::OwnshipDamaged { .. }))
    }

    #[test]
    fn with_it_on_rounds_hurt_aircraft_of_their_shooters_side() {
        assert_eq!(FriendlyFire::default(), FriendlyFire::On);
        let mut s = scene(FriendlyFire::On, Side(1));
        let events = run(&mut s, 100);
        assert!(
            hurt_ownship(&events) && events.contains(&Event::Hit(5)),
            "{events:?}"
        );
    }

    #[test]
    fn with_it_off_a_round_spares_its_shooters_side_and_hurts_the_others() {
        let mut s = scene(FriendlyFire::Off, Side(1));
        let events = run(&mut s, 100);
        assert!(
            !hurt_ownship(&events) && !events.contains(&Event::Hit(5)),
            "{events:?}"
        );
        assert_eq!(s.own().hp, s.own().config.damage_capacity);
        // The same scene with 5 on the other side: both hits stand.
        let mut s = scene(FriendlyFire::Off, Side(2));
        let events = run(&mut s, 100);
        assert!(
            hurt_ownship(&events) && events.contains(&Event::Hit(5)),
            "{events:?}"
        );
    }

    #[test]
    fn with_it_off_a_missile_spares_its_own_launcher() {
        for (setting, hits) in [(FriendlyFire::On, true), (FriendlyFire::Off, false)] {
            let mut s = fixture(true);
            s.targets.clear();
            s.friendly_fire = setting;
            // The ownship's own armed missile doubles back over it.
            let round = shell(&s, 0, [0., 1000., 200.], [0., 1000., 0.]);
            s.projectiles.push(round);
            let events = run(&mut s, 100);
            assert_eq!(hurt_ownship(&events), hits, "{setting:?}: {events:?}");
        }
    }

    #[test]
    fn collisions_ignore_the_setting() {
        for setting in [FriendlyFire::On, FriendlyFire::Off] {
            let mut s = fixture(true);
            s.targets.clear();
            s.friendly_fire = setting;
            let mut ai = target(5, [0., 1000., 20.], 100, 0x80);
            ai.side = Side(1);
            s.targets.push(ai);
            let events = run(&mut s, 2);
            assert!(
                events.contains(&Event::Destroyed(5)),
                "{setting:?}: {events:?}"
            );
            assert!(events.contains(&Event::OwnshipDestroyed { aircraft: 0 }));
        }
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "live_checkpoint.rs"]
mod checkpoint;
