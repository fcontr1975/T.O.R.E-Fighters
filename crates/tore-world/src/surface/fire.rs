//! Surface units fighting: each tick every armed unit and SAM battery runs
//! its engagement controller ([`tore_sim::ai::surface`]) against the hostile
//! aircraft, and the hostile ground units and ships, it can see, and the
//! shots it asks for become projectiles through `live::State::fire_surface`
//! (docs/spec/surface-defenses.md, "Surface AI", "SAM batteries", "AAA and
//! flak", "Surface against surface").
//!
//! [`Arsenal`] is what a unit fights with, read once from its NT and JT
//! records when the terrain builds (the AAA tuning table applied to its
//! guns); [`step`] is the tick. The changing state (controllers, magazines,
//! rails, radars) lives in [`super::SurfaceState`], which combat checkpoints.
//! The radar half (who emits, radar on and off, HARM shutdown, the RWR lock
//! feed) is [`super::emitters`].
//!
//! Fitted rules here (agent decisions, 2026-10-10, recorded in the spec):
//!
//! - A gun mount has its own controller and magazine (a ship's fore and aft
//!   CIWS turn and fire on their own); a unit's missile mounts of one record
//!   share one controller, and each salvo round leaves from a loaded rail
//!   whose arc covers the target.
//! - Rounds leave from the mount: the unit's pose this tick (a moving unit
//!   fires from where it is) plus the hardpoint offset at real size
//!   ([`super::mount_position_ft`]) turned by the unit's attitude, at least
//!   [`MUZZLE_CLEARANCE_FT`] above the terrain. Radars and sights look from
//!   [`EYE_HEIGHT_FT`] above the unit's reference point. Nothing depends on
//!   the shape's extents.
//! - A mount arc of 0 on an axis is unrestricted (every land vehicle reads 0
//!   for heading); otherwise the target's bearing (guns: and elevation) must
//!   lie within the half-arc of the mount's rest direction, relative to the
//!   hull. A missile's launch pitch is the line to the target clamped between
//!   10 degrees and the rail's arc.
//! - Line of sight samples the terrain every 500 ft (at most 64 samples).
//! - A gun's round ends a little past the target ([`gun_end_range`]); a flak
//!   shell's time fuze is its time of flight to the lead point.
//! - The barrage zone fires each burst with its record's random-fire chance,
//!   at the nearest hostile's lead point clamped into its fire zone and
//!   scattered by the record's offset-fire angles.
//!
//! Surface against surface (John, 2026-10-11: "If units are in range of
//! opposing sides they should shoot at each other"):
//!
//! - A weapon aims at what its record's target flags allow (retail): the air
//!   flag for aircraft, the surface flag for ground units and ships
//!   ([`WeaponArms::aims_at_aircraft`], [`WeaponArms::aims_at_surface`]). So
//!   tank, armoured car and small-arms guns fight both, flak and SAMs stay
//!   anti-aircraft, the SS-N-9 fights ships (agent decision: it is
//!   anti-ship), and a land AAA gun, whose record has only the air flag, may
//!   also fire at the ground after any aircraft (John, 2026-10-11: "If it can
//!   shoot at ground targets let it"; [`WeaponArms::ground_fire`]).
//! - What it may aim at is a unit of the other side that the scene holds:
//!   a ground vehicle, a site, a truck, a radar or a ship, never a building,
//!   a parked aircraft or the invisible barrage zone ([`surface_contacts`]).
//!   The template's `react` masks (ATTACK and DEFEND per class) do not gate
//!   it: John's rule is that units in range shoot (opinionated, John).
//! - Preference: an air-defence unit (a SAM or AAA class word) engages
//!   aircraft first, every other unit ground units and ships first. A
//!   controller sees only the eligible contacts of the preferred kind while
//!   there is one, nearest first, ties by id, so the choice is a function of
//!   the positions alone (agent decision).
//! - A gun aims at a surface target with a quarter of its air aim error
//!   ([`tore_sim::ai::surface::surface_aim_error_deg`], fitted). Missile
//!   locks and painting feed aircraft only: a ground unit has no RWR.
//! - A squad's small arms aim at an aircraft with four times the visual guns'
//!   error ([`tore_sim::ai::surface::small_arms_aim_error_deg`], fitted,
//!   agent, 2026-10-12) and lead it by 30 percent of its speed
//!   ([`observation`], [`tore_sim::ai::surface::SMALL_ARMS_LEAD`], fitted,
//!   agent, 2026-10-12): riflemen have no sight on a jet, and a squad rarely
//!   hits a fast one.
use super::{
    BatterySystem, Surface, SurfaceState, UnitId, UnitKind,
    catalog::Catalog,
    emitters::{self, RadarRule},
    units::{MountStock, Seen},
};
use crate::resources::ResourceSource;
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::{
    surface_unit::{Ammo, MountKind, SurfaceUnit, class},
    weapons::{Seeker, Weapon},
};
use tore_sim::{
    ai::surface::{
        self as control, Arm, Controller, FireRequest, Inputs, Phase, Profile, Reserve, Stock,
        Timing,
    },
    attitude::{self, Basis, Vector},
    combat::{
        gunsight::{self, TargetObservation},
        live::{self, ActorSupport, Side, SurfaceShot, opposed},
        missiles::{self, seeker},
        surface_guns::{self, GunTuning},
    },
};

/// Height above a unit's reference point its radar or sight looks from,
/// feet (fitted).
pub const EYE_HEIGHT_FT: f64 = 10.;
/// A round always leaves at least this high above the terrain under it, so
/// a mount at ground level does not fire into the ground (fitted).
pub const MUZZLE_CLEARANCE_FT: f64 = 5.;
/// Line of sight samples the terrain at least this often, feet.
const LOS_STEP_FT: f64 = 500.;
const LOS_MAX_SAMPLES: usize = 64;
/// A missile's launch pitch is never below this, degrees (fitted).
const MIN_LAUNCH_PITCH_DEG: f64 = 10.;
/// The angle unit of the NT records: 16,384 is 90 degrees.
const UNITS_PER_DEGREE: f64 = 182.;
/// A blind SA-2 or SA-3 battery's optical backup reaches at most half its
/// launch range and never beyond 10 nm (defined, agent; default, pending
/// John).
pub const OPTICAL_MAX_RANGE_FT: f64 = 10. * 6_076.;
/// A visual gun refreshes its observation of the target this often (0.5 s,
/// fitted).
const VISUAL_REFRESH_TICKS: u64 = 60;
/// Feet per `zoneDist` and `searchDist` unit (inference, spec "The units of
/// the movement and range words").
const DISTANCE_UNIT_FT: f64 = 256.;

/// What an armed unit's controller fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A guided missile with a reviewed profile.
    Missile,
    /// A gun of the AAA tuning table. `radar`: a radar-directed gun (record
    /// flag 0x4000 with a radar seeker), refreshed every tick; otherwise it
    /// eyeballs its target every 0.5 s. `flak`: its shells burst in the air.
    Gun { radar: bool, flak: bool },
    /// The invisible barrage zone (`A_M1939`): random fire around the
    /// nearest hostile while one is within `activation_ft`.
    Barrage { chance: u8, activation_ft: f64 },
}

/// A range and relative-altitude band, feet: a weapon's detection zone
/// (`zone0`) or launch zone (`zone1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub min_range: f64,
    pub max_range: f64,
    pub min_altitude: f64,
    pub max_altitude: f64,
}

impl Band {
    fn of(zone: &tore_formats::weapons::Zone) -> Self {
        Self {
            min_range: f64::from(zone.minimum_range.max(0)),
            max_range: f64::from(zone.maximum_range.max(0)),
            min_altitude: f64::from(zone.minimum_altitude),
            max_altitude: f64::from(zone.maximum_altitude),
        }
    }
    fn contains(&self, range: f64, altitude: f64) -> bool {
        range >= self.min_range
            && range <= self.max_range
            && altitude >= self.min_altitude
            && altitude <= self.max_altitude
    }
}

/// One mount's arc, from its hardpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MountArc {
    /// Hardpoint index: the round's station and the state's mount.
    pub index: usize,
    /// Rest direction relative to the hull, `[heading, pitch]` degrees.
    pub rest: [f64; 2],
    /// Half-arc about the rest direction, degrees; 0 is unrestricted.
    pub limit: [f64; 2],
    /// The hardpoint's position relative to the hull at real size, feet:
    /// right, up, forward.
    pub offset: Vector,
    /// Where the barrel of a land gun ends, feet from the unit's ground point
    /// as right, up, forward ([`super::muzzle`]); the shell and its flash
    /// start there instead of at `offset`. `None` for a ship's gun and a
    /// launcher.
    pub barrel: Option<Vector>,
}

impl MountArc {
    fn covers_heading(&self, relative_bearing_deg: f64) -> bool {
        self.limit[0] <= 0. || wrap_deg(relative_bearing_deg - self.rest[0]).abs() <= self.limit[0]
    }
    fn covers_elevation(&self, elevation_deg: f64) -> bool {
        self.limit[1] <= 0. || (elevation_deg - self.rest[1]).abs() <= self.limit[1]
    }
    /// The launch pitch of a missile from this rail for a line at
    /// `elevation_deg`.
    fn launch_pitch(&self, elevation_deg: f64) -> f64 {
        let [low, high] = self.launch_pitches();
        elevation_deg.clamp(low, high.max(low))
    }
    /// The lowest and highest launch pitch of a missile from this rail,
    /// degrees.
    pub(crate) fn launch_pitches(&self) -> [f64; 2] {
        if self.limit[1] > 0. {
            [
                (self.rest[1] - self.limit[1]).max(MIN_LAUNCH_PITCH_DEG),
                (self.rest[1] + self.limit[1]).min(90.),
            ]
        } else {
            [MIN_LAUNCH_PITCH_DEG, 90.]
        }
    }
}

/// Whether `arc` covers a target at `relative_bearing_deg` from the hull.
pub(crate) fn covers(arc: &MountArc, relative_bearing_deg: f64) -> bool {
    arc.covers_heading(relative_bearing_deg)
}

/// One controller's weapon: a gun mount, or every missile mount of one
/// record on a unit.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponArms {
    /// The record, with the AAA tuning table applied to a gun.
    pub record: Weapon,
    pub kind: Kind,
    pub tuning: Option<&'static GunTuning>,
    pub mounts: Vec<MountArc>,
    pub profile: Profile,
    /// Where it can see a target: the record's `zone0`.
    pub detection: Band,
    /// Where it may fire: `zone1`, a missile's range scaled by experience, a
    /// gun's capped by its shells' reach.
    pub launch: Band,
    /// A gun's magazine, rounds.
    pub magazine: u32,
    /// Fired by its battery's controller, not its own.
    pub battery: bool,
    /// A land anti-aircraft gun whose record carries only the air flag, which
    /// may still fire at ground units and ships in reach, after any aircraft
    /// (opinionated, John 2026-10-11: "If it can shoot at ground targets let
    /// it").
    pub ground_fire: bool,
}

/// One surface unit's arms: its weapons, its radar and its place.
#[derive(Clone, Debug, PartialEq)]
pub struct Arms {
    pub unit: UnitId,
    /// The unit's reference point, feet: the resolved position with the
    /// terrain height under it.
    pub position: Vector,
    /// Hull heading, radians.
    pub heading: f64,
    pub skill: i32,
    pub side: Side,
    /// The `react` attack mask (aircraft class bits); 0 attacks any class.
    pub react: u32,
    /// `searchDist`, feet, when the unit has one.
    pub search_limit: Option<f64>,
    pub ship: bool,
    pub weapons: Vec<WeaponArms>,
    /// What it emits as, when it is a radar.
    pub radar: Option<RadarRule>,
    /// The battery it belongs to (launcher or radar), by index in
    /// `Surface::batteries`.
    pub battery: Option<usize>,
    /// Its hardpoints with their full loads: rails a truck refills to, a
    /// gun's magazine. Indexed like [`super::SurfaceUnitState::mounts`].
    pub loads: Vec<MountStock>,
    /// The NT's search, unready, attack and retarget times (quarter
    /// seconds), for a battery's controller.
    pub npc: [i32; 4],
    /// A shoulder-fired SAM team (MANPADS, the record's
    /// [`is_shoulder_fired_team`](tore_formats::surface_unit::SurfaceUnit::is_shoulder_fired_team)):
    /// off the AI's threat picture until an aircraft spots it
    /// (docs/spec/ai-strike.md, "Hidden shoulder-fired teams").
    pub team: bool,
}

impl Arms {
    /// Where the unit stands this tick: its mover's pose for a unit that
    /// follows a route, else where the mission put it.
    pub fn place(&self, state: Option<&super::SurfaceUnitState>) -> Place {
        match state.and_then(|s| s.mover) {
            Some(mover) => {
                let origin = mover.position();
                Place {
                    origin,
                    eye: [origin[0], origin[1] + EYE_HEIGHT_FT, origin[2]],
                    basis: mover.basis(),
                    heading: mover.attitude()[0],
                    velocity: mover.velocity(),
                }
            }
            None => Place {
                origin: self.position,
                eye: [
                    self.position[0],
                    self.position[1] + EYE_HEIGHT_FT,
                    self.position[2],
                ],
                basis: Basis::new(self.heading, 0., 0.),
                heading: self.heading,
                velocity: [0.; 3],
            },
        }
    }
}

/// A unit's pose for this tick's fight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// The unit's reference point on the ground, feet.
    pub origin: Vector,
    /// Where its radar or sight looks from.
    pub eye: Vector,
    pub basis: Basis,
    /// Hull heading, radians.
    pub heading: f64,
    pub velocity: Vector,
}

impl Place {
    /// The muzzle of mount `arc`: the hardpoint offset turned by the unit's
    /// attitude, kept [`MUZZLE_CLEARANCE_FT`] above the terrain.
    pub fn mount(&self, arc: &MountArc, ground: &dyn Fn(f64, f64) -> f64) -> Vector {
        let [x, y, z] = arc.offset;
        let b = &self.basis;
        let mut point: Vector = std::array::from_fn(|i| {
            self.origin[i] + b.right[i] * x + b.up[i] * y + b.forward[i] * z
        });
        point[1] = point[1].max(ground(point[0], point[2]) + MUZZLE_CLEARANCE_FT);
        point
    }
}

impl Place {
    /// Where a shell leaves `arc` while the gun aims along `bearing`
    /// (radians, as `direction_at` reads it): the end of its drawn barrel
    /// when it has one, as the gun turns toward its target about its up axis,
    /// else the hardpoint ([`Place::mount`]). Kept above the terrain.
    pub fn muzzle(&self, arc: &MountArc, bearing: f64, ground: &dyn Fn(f64, f64) -> f64) -> Vector {
        let Some([x, y, z]) = arc.barrel else {
            return self.mount(arc, ground);
        };
        let turned = Basis::new(bearing, 0., 0.);
        let mut point: Vector = std::array::from_fn(|i| {
            self.origin[i] + turned.right[i] * x + turned.up[i] * y + turned.forward[i] * z
        });
        point[1] = point[1].max(ground(point[0], point[2]) + MUZZLE_CLEARANCE_FT);
        point
    }
}

/// A SAM battery's fight: its radar, launchers and the missile they fire.
#[derive(Clone, Debug, PartialEq)]
pub struct BatteryArms {
    /// Index in `Surface::batteries`.
    pub index: usize,
    pub system: BatterySystem,
    pub radar: UnitId,
    pub launchers: Vec<UnitId>,
    /// The launchers' missile, profile and zones, at the battery's best
    /// skill.
    pub weapon: WeaponArms,
    pub skill: i32,
    pub side: Side,
    pub react: u32,
}

/// Every armed unit, radar and battery of the surface, ascending unit id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arsenal {
    pub units: Vec<Arms>,
    pub batteries: Vec<BatteryArms>,
    /// The heavy flak guns' teams ([`flak_teams`]), in order of their first
    /// gun's id.
    pub flak_teams: Vec<FlakTeam>,
    /// Records that could not be read, with why: the unit fights without them.
    pub unreadable: Vec<(UnitId, String)>,
}

impl Arsenal {
    pub fn arms(&self, unit: UnitId) -> Option<&Arms> {
        self.units
            .binary_search_by_key(&unit, |arms| arms.unit)
            .ok()
            .map(|at| &self.units[at])
    }
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Reads every armed unit's records. `height` is the terrain under a
    /// point (x, z). Records missing from the import leave the weapon out and
    /// are listed in [`Self::unreadable`].
    pub fn load(
        surface: &Surface,
        resources: &dyn ResourceSource,
        height: &dyn Fn(f64, f64) -> f64,
    ) -> Self {
        let mut catalog = Catalog::new(resources);
        let mut arsenal = Arsenal::default();
        let battery_of: BTreeMap<UnitId, usize> = surface
            .batteries
            .iter()
            .enumerate()
            .flat_map(|(index, battery)| {
                battery
                    .launchers
                    .iter()
                    .chain([&battery.radar])
                    .map(move |id| (*id, index))
            })
            .collect();
        for unit in &surface.units {
            let named = emitters::named_radar(&unit.resource);
            let battery = battery_of.get(&unit.id).copied();
            if unit.kind != UnitKind::Active && !named && battery.is_none() {
                continue;
            }
            let record = match catalog.entry(&unit.resource) {
                Ok(entry) => entry.unit.clone(),
                Err(why) => {
                    arsenal.unreadable.push((unit.id, why));
                    None
                }
            };
            let ship = unit.class & class::SHIP != 0;
            let position = [
                f64::from(unit.position[0]),
                height(f64::from(unit.position[0]), f64::from(unit.position[2]))
                    + f64::from(unit.position[1]),
                f64::from(unit.position[2]),
            ];
            let mut arms = Arms {
                unit: unit.id,
                position,
                heading: f64::from(unit.angles[0]).to_radians(),
                skill: unit.skill.clamp(0, 3),
                side: unit.side,
                react: unit.react.map_or(0, |react| react[0]),
                search_limit: unit
                    .search_dist
                    .filter(|d| *d > 0)
                    .map(|d| f64::from(d) * DISTANCE_UNIT_FT),
                ship,
                weapons: Vec::new(),
                radar: None,
                battery,
                loads: Vec::new(),
                npc: [0; 4],
                team: false,
            };
            let in_battery_launcher =
                battery.is_some_and(|b| surface.batteries[b].radar != unit.id);
            let mut sensor_range = None;
            if let Some(nt) = &record {
                arms.team = nt.is_shoulder_fired_team();
                arms.npc = [
                    nt.npc.search_frequency,
                    nt.npc.unready_attack,
                    nt.npc.attack,
                    nt.npc.retarget,
                ];
                arms.loads = nt
                    .mounts
                    .iter()
                    .map(|mount| MountStock {
                        loaded: match mount.ammo() {
                            Ammo::Rounds(n) => n,
                            Ammo::Unlimited => tore_formats::surface_unit::UNLIMITED_ROUNDS as u32,
                        },
                        reserve: Some(0),
                        ordinal: 0,
                    })
                    .collect();
                for mount in &nt.mounts {
                    if mount.kind == MountKind::Sensor
                        && let Some(name) = &mount.store
                    {
                        match resources.get(name).map(|bytes| Seeker::parse(name, bytes)) {
                            Some(Ok(seeker)) => {
                                sensor_range = Some(Band::of(&seeker.zones[0]));
                            }
                            Some(Err(error)) => arsenal
                                .unreadable
                                .push((unit.id, format!("{name}: {error}"))),
                            None => arsenal
                                .unreadable
                                .push((unit.id, format!("missing {name}; re-import media"))),
                        }
                    }
                }
                // A land gun's shell leaves the end of its drawn barrel.
                let barrel = if ship {
                    None
                } else {
                    nt.shape
                        .as_deref()
                        .and_then(|name| super::muzzle::barrel_ft(name, resources.get(name)?))
                };
                arms.weapons = weapons_of(
                    nt,
                    &unit.resource,
                    barrel,
                    resources,
                    unit.skill,
                    ship,
                    in_battery_launcher,
                    &mut arms.loads,
                    &mut |why| arsenal.unreadable.push((unit.id, why)),
                );
            }
            arms.radar = emitters::rule(
                &arms,
                named,
                sensor_range,
                battery.map(|b| surface.batteries[b].radar == unit.id),
            );
            arsenal.units.push(arms);
        }
        arsenal.units.sort_by_key(|arms| arms.unit);
        arsenal.flak_teams = flak_teams(&arsenal.units);
        // Each battery fights with its first live launcher's missile, at the
        // battery's best skill.
        for (index, battery) in surface.batteries.iter().enumerate() {
            let skill = battery
                .launchers
                .iter()
                .filter_map(|id| surface.unit(*id))
                .map(|unit| unit.skill)
                .max()
                .unwrap_or(super::resolve::DEFAULT_SKILL)
                .clamp(0, 3);
            let Some(lead) = battery.launchers.iter().find_map(|id| arsenal.arms(*id)) else {
                continue;
            };
            let Some(missile) = lead
                .weapons
                .iter()
                .find(|w| w.kind == Kind::Missile)
                .cloned()
            else {
                continue;
            };
            let [search, unready, attack, retarget] = lead.npc;
            let mut weapon = missile;
            weapon.profile.timing = Timing::from_quarters(
                search,
                unready,
                attack,
                weapon.record.guidance.track_t,
                retarget,
                skill,
            );
            weapon.launch = launch_band(&weapon.record, Kind::Missile, skill);
            arsenal.batteries.push(BatteryArms {
                index,
                system: battery.system,
                radar: battery.radar,
                launchers: battery.launchers.clone(),
                weapon,
                skill,
                side: battery.side,
                react: lead.react,
            });
        }
        arsenal
    }
}

/// Half extents of the target volume a shapeless unit (the barrage zone)
/// stands in, feet: a 60 ft square, 20 ft tall, on the ground (fitted).
pub const SHAPELESS_HALF_FT: [f64; 3] = [30., 10., 30.];

/// Combat target volumes for the armed units the scene cannot draw: the
/// invisible barrage zones. A legal bomb target (an AAA kill), never drawn.
pub fn shapeless_targets(surface: &Surface) -> Vec<tore_sim::airport::StaticObject> {
    surface
        .units
        .iter()
        .filter(|unit| !unit.in_scene)
        .filter_map(|unit| {
            let arms = surface.arsenal.arms(unit.id)?;
            arms.weapons
                .iter()
                .any(|w| matches!(w.kind, Kind::Barrage { .. }))
                .then(|| tore_sim::airport::StaticObject {
                    id: unit.id.0,
                    source: tore_sim::airport::SourceKey {
                        layout: String::new(),
                        ordinal: unit.id.0,
                    },
                    name: unit.name.clone(),
                    object_type: unit.resource.clone(),
                    bounds: tore_sim::airport::OrientedBox {
                        center: [
                            arms.position[0],
                            arms.position[1] + SHAPELESS_HALF_FT[1],
                            arms.position[2],
                        ],
                        half: SHAPELESS_HALF_FT,
                        heading: arms.heading,
                        pitch: 0.,
                        bank: 0.,
                    },
                    hit_points: unit.hit_points.max(1),
                    category: unit.class,
                    radar_signature: 0.,
                    infrared_signature: 0.,
                    type_flags: 0,
                    max_visible: 0,
                    runway: false,
                })
        })
        .collect()
}

/// A missile's launch band: its `zone1`, the maximum range scaled by
/// experience; a gun's: `zone1` capped by its shells' reach.
fn launch_band(record: &Weapon, kind: Kind, skill: i32) -> Band {
    let mut band = Band::of(&record.seeker.zones[1]);
    match kind {
        Kind::Missile => {
            band.max_range *= f64::from(control::launch_range_percent(skill)) / 100.;
        }
        Kind::Gun { .. } | Kind::Barrage { .. } => {
            band.max_range = band.max_range.min(surface_guns::reach_ft(record));
        }
    }
    band
}

impl WeaponArms {
    /// Whether the weapon may be aimed at aircraft: its record's air target
    /// flag ([`live::engages_aircraft`], retail).
    pub fn aims_at_aircraft(&self) -> bool {
        live::engages_aircraft(&self.record)
    }
    /// The highest a target aircraft may stand above the hull's horizontal
    /// plane for the weapon to engage it, degrees: a tank gun's
    /// [`surface_guns::TANK_MAX_ELEVATION_DEG`] (opinionated, John,
    /// 2026-10-24). `None` for every other weapon, whose mount arcs alone
    /// limit it.
    pub fn air_ceiling_deg(&self) -> Option<f64> {
        self.tank_gun()
            .then_some(surface_guns::TANK_MAX_ELEVATION_DEG)
    }
    /// A main battle tank's gun ([`surface_guns::is_tank_gun`]).
    pub fn tank_gun(&self) -> bool {
        matches!(self.kind, Kind::Gun { .. }) && surface_guns::is_tank_gun(&self.record.source)
    }
    /// Whether the weapon may be aimed at some ground unit or ship: its
    /// record's surface target flag ([`live::engages_surface`], retail) or
    /// [`Self::ground_fire`] (a land AAA gun), for a gun whose rounds strike
    /// (not flak, not the barrage zone) or a missile.
    pub fn aims_at_any_surface(&self) -> bool {
        (live::engages_surface(&self.record) || self.ground_fire)
            && match self.kind {
                Kind::Gun { flak, .. } => !flak,
                Kind::Barrage { .. } => false,
                Kind::Missile => true,
            }
    }
    /// Whether the weapon may be aimed at a ground unit or ship of OBJECT
    /// class `class_word`: [`Self::aims_at_any_surface`], and an anti-ship
    /// missile ([`missiles::ANTI_SHIP`], the SS-N-9) at ships only.
    pub fn aims_at_surface(&self, class_word: u16) -> bool {
        self.aims_at_any_surface()
            && (self.kind != Kind::Missile
                || !missiles::ANTI_SHIP.contains(&self.record.source.as_str())
                || class_word & class::SHIP != 0)
    }

    /// A gun mount's weapon: `record` tuned by the AAA tuning table for
    /// `unit` (`ZSU23`), its controller timed by the NT's `npc` times
    /// (search, unready, attack, retarget; quarter seconds) at `skill`.
    /// `None` when the record has no row in the table. Returns the mount's
    /// full stock too: a magazine and, on land, two spare ones.
    pub fn gun(
        mut record: Weapon,
        unit: &str,
        arc: MountArc,
        skill: i32,
        ship: bool,
        npc: [i32; 4],
        zone_dist: i32,
    ) -> Option<(Self, MountStock)> {
        let row = surface_guns::apply(unit, &mut record)?;
        let [search, unready, attack, retarget] = npc;
        let timing = Timing::from_quarters(
            search,
            unready,
            attack,
            record.guidance.track_t,
            retarget,
            skill,
        );
        let kind = if record.burst.random_fire_percent > 0 && zone_dist > 0 {
            Kind::Barrage {
                chance: u8::try_from(record.burst.random_fire_percent.clamp(0, 100)).unwrap_or(0),
                activation_ft: f64::from(zone_dist) * DISTANCE_UNIT_FT,
            }
        } else {
            Kind::Gun {
                radar: record.flags & 0x4000 != 0 && record.seeker.signature == 3,
                flak: live::is_flak(&record),
            }
        };
        let stock = MountStock {
            loaded: row.magazine,
            reserve: if ship {
                None
            } else {
                row.mount.reserve_magazines()
            },
            ordinal: 0,
        };
        let burst = u32::from(record.burst.game_rounds_in_burst.max(1))
            * u32::from(record.burst.actual_rounds_per_game.max(1));
        Some((
            Self {
                profile: Profile {
                    timing,
                    arm: Arm::Gun {
                        burst,
                        burst_ticks: u64::from(record.burst.game_burst_t.max(1)) * 30,
                        pause: u64::from(record.burst.reload_t) * 30,
                        opening: u32::from(record.burst.startup_shots),
                        opening_gap: u64::from(row.opening_gap_quarters()) * 30,
                        swap: u64::from(row.magazine_reload_s) * 120,
                    },
                },
                detection: Band::of(&record.seeker.zones[0]),
                launch: launch_band(&record, kind, skill),
                kind,
                tuning: Some(row),
                mounts: vec![arc],
                magazine: row.magazine,
                battery: false,
                // A land gun that aims at aircraft only (a ship's AAA keeps
                // to the air).
                ground_fire: !ship
                    && !live::engages_surface(&record)
                    && matches!(kind, Kind::Gun { flak: false, .. }),
                record,
            },
            stock,
        ))
    }

    /// A missile weapon fired from `arc` (more rails join with
    /// [`Self::mounts`]): salvo, spacing and zones from the record, the
    /// launch range scaled by `skill`. `None` for a record without a reviewed
    /// guidance profile (ASROC), which never fires.
    pub fn missile(
        record: Weapon,
        arc: MountArc,
        skill: i32,
        npc: [i32; 4],
        battery: bool,
    ) -> Option<Self> {
        missiles::Profile::for_weapon(&record)?;
        let [search, unready, attack, retarget] = npc;
        Some(Self {
            profile: Profile {
                timing: Timing::from_quarters(
                    search,
                    unready,
                    attack,
                    record.guidance.track_t,
                    retarget,
                    skill,
                ),
                arm: Arm::Missile {
                    salvo: u32::from(record.burst.game_rounds_in_burst.max(1)),
                    gap: u64::from(record.burst.game_burst_t.max(1)) * 30,
                    spacing: u64::from(record.burst.reload_t.max(1)) * 30,
                },
            },
            detection: Band::of(&record.seeker.zones[0]),
            launch: launch_band(&record, Kind::Missile, skill),
            kind: Kind::Missile,
            tuning: None,
            mounts: vec![arc],
            magazine: 0,
            battery,
            ground_fire: false,
            record,
        })
    }
}

/// A hardpoint's arc from its NT words.
pub fn mount_arc(index: usize, mount: &tore_formats::surface_unit::Mount) -> MountArc {
    MountArc {
        index,
        rest: mount.slew.map(|v| f64::from(v) / UNITS_PER_DEGREE),
        limit: mount.slew_limit.map(|v| f64::from(v) / UNITS_PER_DEGREE),
        offset: super::mount_position_ft(mount),
        barrel: None,
    }
}

/// The controllers of one NT: a controller per gun mount and one per missile
/// record, each with its record read and tuned. Unreviewed missiles (ASROC)
/// are left out: they never fire.
#[allow(clippy::too_many_arguments)]
fn weapons_of(
    nt: &SurfaceUnit,
    resource: &str,
    barrel: Option<Vector>,
    resources: &dyn ResourceSource,
    skill: i32,
    ship: bool,
    in_battery: bool,
    loads: &mut [MountStock],
    unreadable: &mut dyn FnMut(String),
) -> Vec<WeaponArms> {
    let unit_name = resource.trim_end_matches(".NT").trim_end_matches(".nt");
    let npc = [
        nt.npc.search_frequency,
        nt.npc.unready_attack,
        nt.npc.attack,
        nt.npc.retarget,
    ];
    let mut weapons: Vec<WeaponArms> = Vec::new();
    for (index, mount) in nt.mounts.iter().enumerate() {
        if mount.kind != MountKind::Weapon {
            continue;
        }
        let Some(name) = &mount.store else { continue };
        let record = match resources.get(name).map(|bytes| Weapon::parse(name, bytes)) {
            Some(Ok(record)) => record,
            Some(Err(error)) => {
                unreadable(format!("{name}: {error}"));
                continue;
            }
            None => {
                unreadable(format!("missing {name}; re-import media"));
                continue;
            }
        };
        let mut arc = mount_arc(index, mount);
        if surface_guns::is_surface_gun(&record.source) {
            arc.barrel = barrel;
            if let Some((gun, stock)) =
                WeaponArms::gun(record, unit_name, arc, skill, ship, npc, nt.npc.zone_dist)
            {
                loads[index] = stock;
                weapons.push(gun);
            }
            continue;
        }
        if let Some(group) = weapons
            .iter_mut()
            .find(|w| w.kind == Kind::Missile && w.record.source == record.source)
        {
            group.mounts.push(arc);
            continue;
        }
        if let Some(missile) = WeaponArms::missile(record, arc, skill, npc, in_battery) {
            weapons.push(missile);
        }
    }
    weapons
}

/// An aircraft the surface may engage, as the world sees it this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aircraft {
    pub id: u32,
    pub side: Side,
    pub position: Vector,
    pub velocity: Vector,
    /// The PT class word (fighter 0x8000, bomber 0x4000).
    pub category: u16,
    /// In flight: not parked, rolling or wrecked on the ground.
    pub airborne: bool,
    /// A radio-frequency jammer is on.
    pub jammer: bool,
}

/// What a surface controller may engage this tick: an aircraft, or a
/// ground unit or ship of a side ([`surface_contacts`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: u32,
    pub side: Side,
    /// The aircraft, or the unit's aim point (the upper half of its box,
    /// where its combat row stands), feet.
    pub position: Vector,
    pub velocity: Vector,
    /// An aircraft's PT class word; a unit's OBJECT class word.
    pub category: u16,
    /// An aircraft in flight; never a surface unit.
    pub airborne: bool,
    /// A radio-frequency jammer is on (aircraft only).
    pub jammer: bool,
    /// A surface unit, not an aircraft.
    pub surface: bool,
}

impl From<&Aircraft> for Contact {
    fn from(a: &Aircraft) -> Self {
        Self {
            id: a.id,
            side: a.side,
            position: a.position,
            velocity: a.velocity,
            category: a.category,
            airborne: a.airborne,
            jammer: a.jammer,
            surface: false,
        }
    }
}

/// The surface units a controller may engage this tick, ascending id: every
/// unit of a side that stands (hit points above 0) and that the scene holds
/// (`in_scene`, so never the invisible barrage zone), at its combat row
/// (aim point and velocity), with its class word. Buildings and other
/// structures, and parked aircraft, are never aimed at (agent decision:
/// a crew fights vehicles, sites and ships); a round may still strike one
/// of the other side that stands in its way.
pub fn surface_contacts(surface: &Surface, live: &live::State) -> Vec<Contact> {
    let mut out: Vec<Contact> = live
        .targets
        .iter()
        .filter(|t| t.role == missiles::TargetRole::Surface && t.hp > 0 && t.side != live::NO_SIDE)
        .filter_map(|t| {
            let unit = surface.unit(UnitId(t.id))?;
            (unit.kind != UnitKind::Structure && unit.in_scene).then_some(Contact {
                id: t.id,
                side: t.side,
                position: t.position,
                velocity: t.velocity,
                category: unit.class,
                airborne: false,
                jammer: false,
                surface: true,
            })
        })
        .collect();
    out.sort_by_key(|c| c.id);
    out
}

/// A unit that engages ground units and ships before aircraft: anything but
/// an air-defence unit (a SAM or AAA class word). Agent decision,
/// 2026-10-11.
pub fn prefers_surface(class_word: u16) -> bool {
    class_word & (class::SAM | class::AAA) == 0
}

/// Whether `id` is a surface object's id (a layout or template unit, an
/// added truck or radar), not an aircraft's.
pub fn is_surface_id(id: u32) -> bool {
    (super::LAYOUT_OBJECT_BASE..super::SURFACE_UNIT_END).contains(&id)
}

/// The world around the surface for one tick.
pub struct Scene<'a> {
    pub tick: u64,
    pub aircraft: &'a [Aircraft],
    /// Terrain height at (x, z).
    pub ground: &'a dyn Fn(f64, f64) -> f64,
    /// Clear, cloudy, dawn or sunset: a blind SA-2 or SA-3 battery may use its
    /// optical backup.
    pub daylight: bool,
    /// The highest `ground` reaches over each square, when the host has it:
    /// a sight line wholly above it is clear without sampling.
    pub ceiling: Option<&'a tore_sim::ground_ceiling::GroundCeiling>,
}

impl Scene<'_> {
    /// [`line_of_sight`] over the scene's ground, answered from the ground
    /// ceiling when the whole line is above it (the samples could only
    /// agree).
    pub fn line_of_sight(&self, from: Vector, to: Vector) -> bool {
        self.ceiling
            .is_some_and(|ceiling| ceiling.clear_above(from, to))
            || line_of_sight(from, to, self.ground)
    }
}

/// One line of the surface trace (`--surface-trace`): what the controllers
/// did this tick. Presentation only; never read back.
#[derive(Clone, Debug, PartialEq)]
pub enum Trace {
    Phase {
        unit: UnitId,
        weapon: usize,
        phase: Phase,
        target: Option<u32>,
    },
    Shot {
        unit: UnitId,
        mount: usize,
        record: String,
        target: u32,
        rounds: u32,
        refused: u32,
        opening: bool,
        flak: bool,
    },
    Swap {
        unit: UnitId,
        mount: usize,
    },
    /// A truck rearmed every empty rail of the unit.
    Rearm {
        unit: UnitId,
    },
    /// A truck added a spare magazine to the gun on hardpoint `mount`.
    Refill {
        unit: UnitId,
        mount: usize,
    },
    Radar {
        unit: UnitId,
        on: bool,
    },
    Shutdown {
        unit: UnitId,
        missile: u32,
        rolled: bool,
    },
    Battery {
        battery: usize,
        phase: Phase,
        target: Option<u32>,
        optical: bool,
    },
    /// A flak team's detected groups, or what it does about them, changed
    /// ([`flak_teams`]).
    FlakTeam {
        team: usize,
        /// Its live guns.
        guns: Vec<UnitId>,
        groups: Vec<FlakGroup>,
    },
}

/// What one surface tick hands the world.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stepped {
    /// Fire-control answers for the surface missiles, merged with the AI
    /// wings' in one `set_actor_supports` call.
    pub supports: Vec<ActorSupport>,
}

/// A gun burst that began this tick (not flak: its shells are not drawn), as
/// a networked host tells clients of it (protocol 22): the client remakes
/// the rounds from the first round's direction, the burst's schedule and the
/// target's drawn motion ([`gun_aim_point`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BurstNote {
    pub unit: UnitId,
    /// The hardpoint the rounds leave from.
    pub mount: usize,
    /// The unit's controller that fires it, by index in its arms' weapons.
    pub engager: usize,
    pub target: u32,
    /// The first round's direction, with the burst's aim error.
    pub direction: Vector,
    /// Rounds in the burst and the ticks they spread over: round k leaves
    /// at the first tick at or after `k * span / rounds` from the first.
    pub rounds: u32,
    pub span: u64,
}

/// The degrees in -180..180.
fn wrap_deg(degrees: f64) -> f64 {
    (degrees + 180.).rem_euclid(360.) - 180.
}

fn sub(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn length(v: Vector) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Bearing (radians, world heading) and elevation (degrees) of `d`.
fn bearing_elevation(d: Vector) -> (f64, f64) {
    (d[0].atan2(d[2]), d[1].atan2(d[0].hypot(d[2])).to_degrees())
}

/// The elevation of `d` above the horizontal plane of a hull with `basis`,
/// degrees: the vehicle's own frame, so a tank on a slope elevates its gun
/// from its own deck.
pub fn hull_elevation_deg(basis: &Basis, d: Vector) -> f64 {
    let up = attitude::dot(d, basis.up);
    let flat = attitude::dot(d, basis.right).hypot(attitude::dot(d, basis.forward));
    up.atan2(flat).to_degrees()
}

/// The angular rate of a target at `offset` from a gun moving at
/// `own_velocity`, the target moving at `velocity`, degrees a second: how fast
/// the line of sight turns.
pub fn angular_rate_deg_s(offset: Vector, velocity: Vector, own_velocity: Vector) -> f64 {
    let range2 = attitude::dot(offset, offset);
    if range2 < 1. {
        return 0.;
    }
    let relative = sub(velocity, own_velocity);
    let turn = attitude::cross(offset, relative);
    (length(turn) / range2).to_degrees()
}

/// No terrain between `from` and `to`.
pub fn line_of_sight(from: Vector, to: Vector, ground: &dyn Fn(f64, f64) -> f64) -> bool {
    let d = sub(to, from);
    let samples = ((length(d) / LOS_STEP_FT).ceil() as usize).clamp(2, LOS_MAX_SAMPLES);
    (1..samples).all(|i| {
        let t = i as f64 / samples as f64;
        let p = [from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t];
        p[1] > ground(p[0], p[2])
    })
}

/// Whether `aircraft` is a hostile the unit (or battery) may attack: an
/// enemy of its side, airborne, of a class its react mask names.
fn hostile(side: Side, react: u32, aircraft: &Contact) -> bool {
    !aircraft.surface
        && side != live::NO_SIDE
        && aircraft.side != live::NO_SIDE
        && aircraft.side != side
        && aircraft.airborne
        && (react == 0 || aircraft.category == 0 || u32::from(aircraft.category) & react != 0)
}

/// Whether a controller of a unit of `side` with `weapon` may aim at
/// `contact` at all: a hostile aircraft its react mask names, for a weapon
/// that aims at aircraft, or a ground unit or ship of the other side of a
/// class the weapon aims at.
fn wanted(side: Side, react: u32, weapon: &WeaponArms, contact: &Contact) -> bool {
    if contact.surface {
        opposed(side, contact.side) && weapon.aims_at_surface(contact.category)
    } else {
        weapon.aims_at_aircraft() && hostile(side, react, contact)
    }
}

/// This tick's contacts: the aircraft first, then the ground units and
/// ships ([`surface_contacts`]).
#[derive(Clone, Copy)]
struct Contacts<'a> {
    all: &'a [Contact],
    /// How many of `all` are aircraft, at its start.
    aircraft: usize,
}

impl<'a> Contacts<'a> {
    /// The contacts `weapon` may aim at by kind, in order: the aircraft
    /// only for a weapon that never aims at the surface, the ground units
    /// and ships only for one that never aims at aircraft. The others would
    /// all fail [`wanted`], so leaving them out changes no answer.
    fn for_weapon(self, weapon: &WeaponArms) -> &'a [Contact] {
        debug_assert!(self.all[..self.aircraft].iter().all(|c| !c.surface));
        debug_assert!(self.all[self.aircraft..].iter().all(|c| c.surface));
        match (weapon.aims_at_aircraft(), weapon.aims_at_any_surface()) {
            (true, true) => self.all,
            (true, false) => &self.all[..self.aircraft],
            (false, true) => &self.all[self.aircraft..],
            (false, false) => &[],
        }
    }
}

/// What a unit's controller, or a battery's, sees.
struct Look<'a> {
    /// Where it looks from.
    from: Vector,
    side: Side,
    react: u32,
    weapon: &'a WeaponArms,
    band: Band,
    /// The detection range: a hostile inside it wakes the controller.
    range: f64,
    search_limit: Option<f64>,
    /// It engages ground units and ships before aircraft.
    prefer_surface: bool,
    line_of_sight_needed: bool,
    /// A tank gun's hull and elevation ceiling: an aircraft higher above
    /// the hull's horizontal plane is not eligible
    /// ([`WeaponArms::air_ceiling_deg`]).
    ceiling: Option<(Basis, f64)>,
}

impl Look<'_> {
    /// Whether any hostile it may aim at is inside its range, and the
    /// eligible ones (inside the detection band, the search limit and line of
    /// sight) of the preferred kind while there is one, else of the other,
    /// nearest first, ties by id.
    fn sees(&self, contacts: Contacts<'_>, scene: &Scene<'_>) -> (bool, Vec<u32>) {
        let seen = self.sees_among(contacts.for_weapon(self.weapon), scene);
        // Every surface test checks the narrowed list against every contact.
        #[cfg(test)]
        assert_eq!(seen, self.sees_among(contacts.all, scene));
        seen
    }

    fn sees_among(&self, contacts: &[Contact], scene: &Scene<'_>) -> (bool, Vec<u32>) {
        let mut in_range = false;
        let mut eligible: Vec<(bool, f64, u32)> = Vec::new();
        let surface = self.weapon.aims_at_any_surface();
        for contact in contacts {
            if (contact.surface && !surface) || !wanted(self.side, self.react, self.weapon, contact)
            {
                continue;
            }
            let d = sub(contact.position, self.from);
            let distance = length(d);
            if distance > self.range {
                continue;
            }
            in_range = true;
            if !self.band.contains(distance, d[1])
                || self.search_limit.is_some_and(|limit| distance > limit)
                || (!contact.surface
                    && self
                        .ceiling
                        .is_some_and(|(basis, ceiling)| hull_elevation_deg(&basis, d) > ceiling))
                || (self.line_of_sight_needed && !scene.line_of_sight(self.from, contact.position))
            {
                continue;
            }
            eligible.push((contact.surface != self.prefer_surface, distance, contact.id));
        }
        eligible.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        if let Some(&(other_kind, _, _)) = eligible.first() {
            eligible.retain(|e| e.0 == other_kind);
        }
        (
            in_range,
            eligible.into_iter().map(|(_, _, id)| id).collect(),
        )
    }
}

/// The surface's random draws: SplitMix64 kept in the state.
pub(crate) fn draw(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
/// A uniform draw in 0..1.
fn unit_draw(state: &mut u64) -> f64 {
    (draw(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// The farther a gun round may fly past its target before it is removed:
/// 15 percent beyond, at least 500 ft (fitted).
pub fn gun_end_range(range: f64) -> f64 {
    (range * 1.15).max(range + 500.)
}

/// `direction` turned by `right` and `up` radians.
fn deflect(direction: Vector, right: f64, up: f64) -> Vector {
    let forward = attitude::unit(direction);
    let reference = if forward[1].abs() < 0.9 {
        [0., 1., 0.]
    } else {
        [1., 0., 0.]
    };
    let r = attitude::unit(attitude::cross(reference, forward));
    let u = attitude::unit(attitude::cross(forward, r));
    attitude::unit(std::array::from_fn(|i| {
        forward[i] + r[i] * right.tan() + u[i] * up.tan()
    }))
}

/// A direction at `heading` (radians) and `pitch` (degrees).
pub(crate) fn direction_at(heading: f64, pitch_deg: f64) -> Vector {
    let pitch = pitch_deg.to_radians();
    [
        heading.sin() * pitch.cos(),
        pitch.sin(),
        heading.cos() * pitch.cos(),
    ]
}

/// The surface's tick: radars, batteries, then every unit's own
/// controllers. Returns the fire-control answers for the world to merge with
/// the AI wings'. Does nothing for a surface with no armed unit.
pub fn step(
    surface: &Surface,
    state: &mut SurfaceState,
    live: &mut live::State,
    scene: &Scene<'_>,
) -> Stepped {
    let arsenal = &surface.arsenal;
    state.trace.clear();
    state.locks.clear();
    state.painting.clear();
    state.bursts.clear();
    if arsenal.is_empty() {
        return Stepped::default();
    }
    state.arm(arsenal);
    // Where every armed unit stands this tick: a moving unit fires and looks
    // from its current pose, any other from where the mission put it (kept
    // from tick to tick while the arsenal is the same).
    let key = |arms: &Arms| {
        [
            arms.position[0].to_bits(),
            arms.position[1].to_bits(),
            arms.position[2].to_bits(),
            arms.heading.to_bits(),
        ]
    };
    if state.static_places.len() != arsenal.units.len()
        || state
            .static_places
            .iter()
            .zip(&arsenal.units)
            .any(|((unit, bits, _), arms)| *unit != arms.unit || *bits != key(arms))
    {
        state.static_places = arsenal
            .units
            .iter()
            .map(|arms| (arms.unit, key(arms), arms.place(None)))
            .collect();
    }
    let places: BTreeMap<UnitId, Place> = arsenal
        .units
        .iter()
        .zip(&state.static_places)
        .map(|(arms, (_, _, fixed))| {
            let unit = state.unit(arms.unit);
            let place = if unit.is_some_and(|u| u.mover.is_some()) {
                arms.place(unit)
            } else {
                *fixed
            };
            (arms.unit, place)
        })
        .collect();
    // Every surface test checks the kept places against working them out.
    #[cfg(test)]
    assert!(arsenal.units.iter().all(|arms| {
        format!("{:?}", places[&arms.unit]) == format!("{:?}", arms.place(state.unit(arms.unit)))
    }));
    state.places = places;
    // Each surface id's combat row, and whether it stands.
    let rows: BTreeMap<u32, usize> = live
        .targets
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            t.role == missiles::TargetRole::Surface && arsenal.arms(UnitId(t.id)).is_some()
        })
        .map(|(index, t)| (t.id, index))
        .collect();
    let alive = |live: &live::State, id: UnitId| {
        rows.get(&id.0)
            .is_some_and(|index| live.targets[*index].hp > 0)
    };
    // What the controllers may aim at: the aircraft, then the ground units
    // and ships of a side.
    let ground = surface_contacts(surface, live);
    let list: Vec<Contact> = scene
        .aircraft
        .iter()
        .map(Contact::from)
        .chain(ground.iter().copied())
        .collect();
    let contacts = Contacts {
        all: &list,
        aircraft: scene.aircraft.len(),
    };
    // Radars first: who emits this tick, HARM shutdowns.
    let radars = emitters::step(surface, state, live, scene, &rows, &ground);
    let mut supports = Vec::new();
    // In-flight surface missiles by (owner, target).
    let mut in_flight: BTreeSet<(u32, u32)> = BTreeSet::new();
    for projectile in &live.projectiles {
        // The projectile's own fields before the surface round lookup.
        if let Some(target) = projectile.target
            && projectile.guidance.is_some()
            && let Some(round) = live.surface_round(projectile.id)
            && !round.flak
        {
            in_flight.insert((projectile.owner, target));
        }
    }
    let aircraft_by_id: BTreeMap<u32, Contact> = list.iter().map(|c| (c.id, *c)).collect();
    let observe = |from: Vector, id: u32| {
        aircraft_by_id.get(&id).map(|a| seeker::Observation {
            id,
            position: a.position,
            velocity: a.velocity,
            quality: 1.0,
            off_axis: 0.0,
            range: length(sub(a.position, from)),
        })
    };

    // SAM batteries: one controller on the radar.
    for battery in &arsenal.batteries {
        let radar = arsenal.arms(battery.radar);
        // The battery is blind while its radar is destroyed or shut down
        // after a HARM; a radar that is merely off (no hostile near) is not.
        let shut = state.unit(battery.radar).is_some_and(|unit| {
            unit.radar
                .shutdown_until
                .is_some_and(|until| scene.tick < until)
        });
        let radar_ok = alive(live, battery.radar) && !shut;
        let radar_on = radar_ok && radars.get(&battery.radar).copied().unwrap_or(false);
        let optical = !radar_ok
            && matches!(battery.system, BatterySystem::Sa2 | BatterySystem::Sa3)
            && scene.daylight;
        let blind = !radar_ok && !optical;
        let launchers: Vec<&Arms> = battery
            .launchers
            .iter()
            .filter(|id| alive(live, **id))
            .filter_map(|id| arsenal.arms(*id))
            .collect();
        // Detection from the radar, or with the optical backup from the
        // first live launcher.
        let eye = |arms: &Arms| state.place(arms).eye;
        let from = if radar_ok {
            radar.map(eye)
        } else {
            launchers.first().map(|arms| eye(arms))
        };
        let weapon = &battery.weapon;
        let mut launch = weapon.launch;
        if optical {
            launch.max_range = (launch.max_range * 0.5).min(OPTICAL_MAX_RANGE_FT);
        }
        let (in_range, eligible) = match from {
            Some(from) => {
                let range = radar
                    .and_then(|r| r.radar.as_ref())
                    .map_or(weapon.detection.max_range, |rule| {
                        rule.range().max(weapon.detection.max_range)
                    });
                let mut band = weapon.detection;
                if optical {
                    band.max_range = launch.max_range;
                }
                Look {
                    from,
                    side: battery.side,
                    react: battery.react,
                    weapon,
                    band,
                    range: if optical { band.max_range } else { range },
                    search_limit: None,
                    prefer_surface: false,
                    line_of_sight_needed: true,
                    ceiling: None,
                }
                .sees(contacts, scene)
            }
            None => (false, Vec::new()),
        };
        let loaded: u32 = launchers.iter().map(|arms| state.rails(arms, weapon)).sum();
        let supply = launchers
            .iter()
            .any(|arms| state.unit(arms.unit).is_some_and(|u| u.supply));
        let Some(slot) = state.batteries.get_mut(battery.index) else {
            continue;
        };
        slot.optical = optical;
        let target = slot.controller.target();
        // The launcher that may fire at the target now: a loaded rail whose
        // arc covers it, line of sight and the target in its launch zone,
        // nearest first.
        let chosen = target.and_then(|target| {
            let aircraft = aircraft_by_id.get(&target)?;
            launchers
                .iter()
                .filter_map(|arms| {
                    let rail = state.rail_for(arms, weapon, aircraft.position)?;
                    let from = state.place(arms).eye;
                    let d = sub(aircraft.position, from);
                    let distance = length(d);
                    let band = if live.weapon_rules == missiles::Rules::Energy {
                        Band {
                            max_range: f64::INFINITY,
                            ..launch
                        }
                    } else {
                        launch
                    };
                    (band.contains(distance, d[1])
                        && scene.line_of_sight(from, aircraft.position)
                        && energy_gate(live, state, arms, weapon, rail, aircraft, scene))
                    .then_some((distance, arms.unit, rail))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        });
        let slot = &mut state.batteries[battery.index];
        let before = (slot.controller.phase(), slot.controller.target());
        let outcome = slot.controller.advance(
            &weapon.profile,
            &Inputs {
                tick: scene.tick,
                hostile_in_range: in_range,
                eligible: &eligible,
                gates: chosen.is_some(),
                stock: Stock {
                    loaded,
                    reserve: Reserve::Magazines(0),
                    supply,
                },
                blind,
                slow: optical,
                hold: false,
                lay: 0,
            },
        );
        let phase = slot.controller.phase();
        let now_target = slot.controller.target();
        if (phase, now_target) != before {
            state.trace.push(Trace::Battery {
                battery: battery.index,
                phase,
                target: now_target,
                optical,
            });
        }
        if let (Some(fire), Some((_, launcher, rail))) = (outcome.fire, chosen) {
            let arms = arsenal.arms(launcher).expect("a chosen launcher is armed");
            // The track comes from the radar, or the launcher's own sight.
            let own = state.place(arms).eye;
            let track_from = if radar_on {
                radar.map_or(own, |r| state.place(r).eye)
            } else {
                own
            };
            fire_missiles(
                state,
                live,
                arms,
                weapon,
                rail,
                fire,
                observe(track_from, fire.target),
                scene,
            );
        }
        // Support for the battery's missiles: from the radar while it is
        // alive, on and sees the target. With the optical backup the launcher
        // guides without an emitter.
        let lock = slot_lock(&state.batteries[battery.index].controller);
        for arms in &launchers {
            let targets: Vec<u32> = in_flight
                .range((arms.unit.0, 0)..=(arms.unit.0, u32::MAX))
                .map(|(_, target)| *target)
                .chain(lock)
                .collect();
            for target in targets {
                let own = state.place(arms).eye;
                let (radar_position, emitting, supported) = if radar_on {
                    let position = radar.map_or(own, |r| state.place(r).eye);
                    let sees = aircraft_by_id.get(&target).is_some_and(|a| {
                        length(sub(a.position, position)) <= weapon.detection.max_range
                            && scene.line_of_sight(position, a.position)
                    });
                    (position, true, sees)
                } else if optical {
                    let sees = aircraft_by_id.get(&target).is_some_and(|a| {
                        length(sub(a.position, own)) <= launch.max_range
                            && scene.line_of_sight(own, a.position)
                    });
                    (own, false, sees)
                } else {
                    (own, false, false)
                };
                let Some(observation) = observe(radar_position, target) else {
                    continue;
                };
                supports.push(ActorSupport {
                    owner: arms.unit.0,
                    observation: Some(observation),
                    supported,
                    radar_position,
                    radar_emitting: emitting,
                });
            }
        }
        // The radar is the battery's only emitter: its lock tone and
        // painting, never the launchers'.
        if radar_on
            && let Some(target) = lock
            && !is_surface_id(target)
            && state.batteries[battery.index].controller.phase().locked()
        {
            state.locks.push((target, weapon.record.seeker.signature));
            state.painting.push((target, battery.radar.0));
        }
    }

    // Flak teams: what each team sees, and which groups it fires at.
    let plans = plan_teams(arsenal, state, &|id| alive(live, id), &list, scene);
    let team_of: BTreeMap<UnitId, usize> = plans
        .iter()
        .enumerate()
        .flat_map(|(team, plan)| plan.guns.iter().map(move |g| (g.unit, team)))
        .collect();

    // Every unit's own controllers.
    for arms in &arsenal.units {
        if arms.weapons.iter().all(|w| w.battery) || !alive(live, arms.unit) {
            continue;
        }
        let radar_on = radars.get(&arms.unit).copied().unwrap_or(false);
        let prefer_surface = surface
            .unit(arms.unit)
            .is_some_and(|unit| prefers_surface(unit.class));
        for (index, weapon) in arms.weapons.iter().enumerate() {
            if weapon.battery {
                continue;
            }
            step_weapon(
                arms,
                index,
                weapon,
                state,
                live,
                &View {
                    scene,
                    contacts,
                    by_id: &aircraft_by_id,
                    prefer_surface,
                    team: team_of
                        .get(&arms.unit)
                        .map(|team| &plans[*team])
                        .filter(|plan| {
                            plan.guns
                                .iter()
                                .any(|g| g.unit == arms.unit && g.index == index)
                        }),
                },
                radar_on,
                &in_flight,
                &mut supports,
            );
        }
    }
    Stepped { supports }
}

/// The target a controller holds a lock or launch on, for support.
fn slot_lock(controller: &Controller) -> Option<u32> {
    controller
        .phase()
        .locked()
        .then(|| controller.target())
        .flatten()
}

/// What a unit's controllers see this tick: the scene, every contact (by
/// id too) and whether the unit engages ground units and ships first.
struct View<'a> {
    scene: &'a Scene<'a>,
    contacts: Contacts<'a>,
    by_id: &'a BTreeMap<u32, Contact>,
    prefer_surface: bool,
    /// The flak team the unit fights in, with its picture this tick.
    team: Option<&'a TeamPlan>,
}

#[allow(clippy::too_many_arguments)]
fn step_weapon(
    arms: &Arms,
    index: usize,
    weapon: &WeaponArms,
    state: &mut SurfaceState,
    live: &mut live::State,
    seen: &View<'_>,
    radar_on: bool,
    in_flight: &BTreeSet<(u32, u32)>,
    supports: &mut Vec<ActorSupport>,
) {
    let scene = seen.scene;
    let aircraft_by_id = seen.by_id;
    let place = state.place(arms);
    let muzzle = place.eye;
    let barrage = match weapon.kind {
        Kind::Barrage { activation_ft, .. } => Some(activation_ft),
        _ => None,
    };
    let (in_range, eligible) = match barrage {
        // The barrage needs no sight of its target: any hostile within the
        // activation radius.
        Some(activation) => Look {
            from: muzzle,
            side: arms.side,
            react: arms.react,
            weapon,
            band: Band {
                min_range: 0.,
                max_range: activation,
                min_altitude: f64::MIN,
                max_altitude: f64::MAX,
            },
            range: activation,
            search_limit: None,
            prefer_surface: false,
            line_of_sight_needed: false,
            ceiling: None,
        }
        .sees(seen.contacts, scene),
        None => match seen.team {
            // A flak team's gun: the team's shared picture, ordered so the
            // team's fire spreads over the group.
            Some(plan) => (
                !plan.detected.is_empty(),
                plan.order(arms.unit, aircraft_by_id),
            ),
            None => Look {
                from: muzzle,
                side: arms.side,
                react: arms.react,
                weapon,
                band: weapon.detection,
                range: weapon.detection.max_range,
                search_limit: arms.search_limit,
                prefer_surface: seen.prefer_surface,
                line_of_sight_needed: true,
                ceiling: weapon.air_ceiling_deg().map(|c| (place.basis, c)),
            }
            .sees(seen.contacts, scene),
        },
    };
    let Some(unit) = state.unit(arms.unit) else {
        return;
    };
    let supply = unit.supply;
    let engager = &unit.engagers[index];
    let target = engager.controller.target();
    // The launch gates for the current target, and where to fire.
    let mut rail = None;
    let gates = match (target.and_then(|t| aircraft_by_id.get(&t)), weapon.kind) {
        (None, _) => false,
        (Some(_), Kind::Barrage { .. }) => true,
        (Some(aircraft), Kind::Missile) => {
            let d = sub(aircraft.position, muzzle);
            rail = state.rail_for(arms, weapon, aircraft.position);
            let band = if live.weapon_rules == missiles::Rules::Energy {
                Band {
                    max_range: f64::INFINITY,
                    ..weapon.launch
                }
            } else {
                weapon.launch
            };
            rail.is_some()
                && band.contains(length(d), d[1])
                && scene.line_of_sight(muzzle, aircraft.position)
                && energy_gate(live, state, arms, weapon, rail.unwrap(), aircraft, scene)
        }
        (Some(aircraft), Kind::Gun { .. }) => {
            let d = sub(aircraft.position, muzzle);
            let (bearing, elevation) = bearing_elevation(d);
            let arc = &weapon.mounts[0];
            weapon.launch.contains(length(d), d[1])
                && arc.covers_heading((bearing - place.heading).to_degrees())
                && arc.covers_elevation(elevation)
                && (aircraft.surface
                    || weapon
                        .air_ceiling_deg()
                        .is_none_or(|ceiling| hull_elevation_deg(&place.basis, d) <= ceiling))
                && scene.line_of_sight(muzzle, aircraft.position)
        }
    };
    // A tank gun lays on an aircraft before its first round at it.
    let lay = match target {
        Some(target) if weapon.tank_gun() && !is_surface_id(target) => {
            (control::TANK_AIR_LAY_S * 120.).round() as u64
        }
        _ => 0,
    };
    // A flak team holds its fire until it engages the target's group.
    let hold = match (seen.team, target) {
        (Some(plan), Some(target)) => !plan.engaged.contains(&target),
        _ => false,
    };
    let (loaded, reserve) = match weapon.kind {
        Kind::Missile => (state.rails(arms, weapon), Reserve::Magazines(0)),
        _ => {
            let stock = state
                .unit(arms.unit)
                .map(|u| u.mounts[weapon.mounts[0].index]);
            match stock {
                Some(stock) => (
                    stock.loaded,
                    stock.reserve.map_or(Reserve::Unlimited, Reserve::Magazines),
                ),
                None => (0, Reserve::Magazines(0)),
            }
        }
    };
    let unit = state.unit_mut(arms.unit).expect("armed units have state");
    let engager = &mut unit.engagers[index];
    let before = (engager.controller.phase(), engager.controller.target());
    let outcome = engager.controller.advance(
        &weapon.profile,
        &Inputs {
            tick: scene.tick,
            hostile_in_range: in_range,
            eligible: &eligible,
            gates,
            stock: Stock {
                loaded,
                reserve,
                supply,
            },
            blind: false,
            slow: false,
            hold,
            lay,
        },
    );
    let after = (engager.controller.phase(), engager.controller.target());
    if outcome.swap {
        let unit = state.unit_mut(arms.unit).expect("armed units have state");
        let stock = &mut unit.mounts[weapon.mounts[0].index];
        // From the reserve, or from the truck in range when it is empty.
        if let Some(reserve) = &mut stock.reserve
            && *reserve > 0
        {
            *reserve -= 1;
        }
        stock.loaded = weapon.magazine;
        state.trace.push(Trace::Swap {
            unit: arms.unit,
            mount: weapon.mounts[0].index,
        });
    }
    if let Some(fire) = outcome.fire {
        match weapon.kind {
            Kind::Missile => {
                if let Some(rail) = rail {
                    let observation =
                        aircraft_by_id
                            .get(&fire.target)
                            .map(|a| seeker::Observation {
                                id: fire.target,
                                position: a.position,
                                velocity: a.velocity,
                                quality: 1.0,
                                off_axis: 0.0,
                                range: length(sub(a.position, muzzle)),
                            });
                    fire_missiles(state, live, arms, weapon, rail, fire, observation, scene);
                }
            }
            Kind::Gun { flak: true, .. } if fire.opening && seen.team.is_some() => {
                // A flak team's opening barrage: each shell at the next
                // member it can reach, in turn (the mount's round number
                // counts the gun's shells from its first, the opening's).
                let plan = seen.team.expect("checked");
                let members =
                    plan.barrage(arms.unit, fire.target, muzzle, aircraft_by_id, scene.ground);
                for _ in 0..fire.rounds {
                    let shell = state
                        .unit(arms.unit)
                        .and_then(|u| u.mounts.get(weapon.mounts[0].index))
                        .map_or(0, |m| m.ordinal);
                    let at = members[(shell % members.len() as u64) as usize];
                    if let Some(aircraft) = aircraft_by_id.get(&at) {
                        let one = FireRequest {
                            target: at,
                            rounds: 1,
                            ..fire
                        };
                        fire_gun(state, live, arms, index, weapon, one, aircraft, scene);
                    }
                }
            }
            Kind::Gun { .. } | Kind::Barrage { .. } => {
                if let Some(aircraft) = aircraft_by_id.get(&fire.target) {
                    fire_gun(state, live, arms, index, weapon, fire, aircraft, scene);
                }
            }
        }
    }
    // The phase change follows the tick's shots in the trace, so a burst's
    // last round comes before its Pause.
    if after != before {
        state.trace.push(Trace::Phase {
            unit: arms.unit,
            weapon: index,
            phase: after.0,
            target: after.1,
        });
    }
    // Lock feed and support for a self-contained missile unit: its own radar.
    if weapon.kind == Kind::Missile {
        let controller = &state.unit(arms.unit).expect("armed").engagers[index].controller;
        let lock = slot_lock(controller);
        if controller.phase().locked()
            && let Some(target) = controller.target()
            && !is_surface_id(target)
        {
            state.locks.push((target, weapon.record.seeker.signature));
            if radar_on {
                state.painting.push((target, arms.unit.0));
            }
        }
        let radar_guided = weapon.record.seeker.signature == 3;
        let targets: Vec<u32> = in_flight
            .range((arms.unit.0, 0)..=(arms.unit.0, u32::MAX))
            .map(|(_, target)| *target)
            .chain(lock)
            .collect();
        for target in targets {
            let Some(aircraft) = aircraft_by_id.get(&target) else {
                continue;
            };
            let sees = length(sub(aircraft.position, muzzle)) <= weapon.detection.max_range
                && scene.line_of_sight(muzzle, aircraft.position);
            supports.push(ActorSupport {
                owner: arms.unit.0,
                observation: Some(seeker::Observation {
                    id: target,
                    position: aircraft.position,
                    velocity: aircraft.velocity,
                    quality: 1.0,
                    off_axis: 0.0,
                    range: length(sub(aircraft.position, muzzle)),
                }),
                supported: sees && (radar_on || !radar_guided),
                radar_position: muzzle,
                radar_emitting: radar_on && radar_guided,
            });
        }
    } else if radar_on
        && let Some(target) = state.unit(arms.unit).expect("armed").engagers[index]
            .controller
            .target()
        && !is_surface_id(target)
        && state.unit(arms.unit).expect("armed").engagers[index]
            .controller
            .phase()
            .locked()
    {
        // A radar gun paints its target (no lock tone: guns have no
        // missile seeker class).
        state.painting.push((target, arms.unit.0));
    }
}

/// The skill's share of a missile's reach under the energy rules: its
/// experience-scaled launch range over the record's zone-1 range. The
/// energy gate demands reach to a point this much nearer than the target
/// is farther out ([`super::rotor_reach`] bounds the gate with it).
pub(crate) fn energy_share(weapon: &WeaponArms) -> f64 {
    (weapon.launch.max_range / f64::from(weapon.record.seeker.zones[1].maximum_range.max(1)))
        .clamp(0.05, 1.)
}

/// The question the energy gate asks for a round of `weapon` from `arc` at
/// `place` against an aircraft `id` at `position` moving at `velocity`: the
/// launcher on the rail, pitched as it launches, and the aircraft as seen
/// with its horizontal distance pushed out by the skill's share
/// ([`energy_share`]), at its own height and velocity. The energy gate and
/// the helicopter's threat rings ([`super::rotor_reach`]) ask the same.
pub(crate) fn gate_question(
    weapon: &WeaponArms,
    place: &Place,
    arc: &MountArc,
    id: u32,
    position: Vector,
    velocity: Vector,
    ground: &dyn Fn(f64, f64) -> f64,
) -> (live::Launcher, seeker::Observation) {
    let from = place.mount(arc, ground);
    let (bearing, elevation) = bearing_elevation(sub(position, from));
    let direction = direction_at(bearing, arc.launch_pitch(elevation));
    let launcher = live::Launcher {
        radar_power: true,
        position: from,
        basis: tore_sim::combat::guided::nose(direction),
        speed_fps: length(place.velocity),
        velocity: place.velocity,
        bay_ready: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    // The existing skill fraction is applied by demanding reach to the
    // correspondingly farther observed bearing, at the same target altitude.
    let fraction = energy_share(weapon);
    let mut point = position;
    for i in [0, 2] {
        point[i] = from[i] + (point[i] - from[i]) / fraction;
    }
    let observation = seeker::Observation {
        id,
        position: point,
        velocity,
        quality: 1.,
        range: length(sub(point, from)),
        off_axis: 0.,
    };
    (launcher, observation)
}

/// Under the energy rules, whether the energy model says a round from
/// `rail` reaches `target` (pushed out by the skill's share).
#[allow(clippy::too_many_arguments)]
fn energy_gate(
    live: &mut live::State,
    state: &SurfaceState,
    arms: &Arms,
    weapon: &WeaponArms,
    rail: usize,
    target: &Contact,
    scene: &Scene<'_>,
) -> bool {
    if live.weapon_rules != missiles::Rules::Energy {
        return true;
    }
    let place = state.place(arms);
    let arc = weapon
        .mounts
        .iter()
        .find(|m| m.index == rail)
        .unwrap_or(&weapon.mounts[0]);
    let (launcher, observation) = gate_question(
        weapon,
        &place,
        arc,
        target.id,
        target.position,
        target.velocity,
        scene.ground,
    );
    live.request_energy_solution(arms.unit.0, rail, &weapon.record, launcher, observation);
    live.energy_solution(arms.unit.0, rail, &weapon.record, launcher, observation)
        .is_some()
}

#[allow(clippy::too_many_arguments)]
fn fire_missiles(
    state: &mut SurfaceState,
    live: &mut live::State,
    arms: &Arms,
    weapon: &WeaponArms,
    first_rail: usize,
    fire: FireRequest,
    observation: Option<seeker::Observation>,
    scene: &Scene<'_>,
) {
    let place = state.place(arms);
    let Some(target) = observation else { return };
    for round in 0..fire.rounds {
        let rail = if round == 0 {
            Some(first_rail)
        } else {
            state.rail_for(arms, weapon, target.position)
        };
        let Some(rail) = rail else { break };
        let arc = weapon
            .mounts
            .iter()
            .find(|arc| arc.index == rail)
            .copied()
            .unwrap_or(weapon.mounts[0]);
        let muzzle = place.mount(&arc, scene.ground);
        let (bearing, elevation) = bearing_elevation(sub(target.position, muzzle));
        let direction = direction_at(bearing, arc.launch_pitch(elevation));
        let shot = SurfaceShot {
            owner: arms.unit.0,
            weapon: weapon.record.clone(),
            mount: rail,
            position: muzzle,
            direction,
            velocity: place.velocity,
            target: Some(target.id),
            observation: Some(target),
            ordinal: 0,
            end_tick: None,
        };
        let fired = live.fire_surface(shot);
        if fired.is_ok()
            && let Some(unit) = state.unit_mut(arms.unit)
            && let Some(stock) = unit.mounts.get_mut(rail)
        {
            stock.loaded = stock.loaded.saturating_sub(1);
        }
        state.trace.push(Trace::Shot {
            unit: arms.unit,
            mount: rail,
            record: weapon.record.source.clone(),
            target: target.id,
            rounds: u32::from(fired.is_ok()),
            refused: u32::from(fired.is_err()),
            opening: false,
            flak: false,
        });
    }
}

/// What a gun of `weapon` takes its aim from: a target seen at `position`
/// moving at `velocity`, `age` seconds ago. A radar or visual gun carries the
/// target on at its velocity for the age; a squad's small arms firing at an
/// aircraft ([`control::SMALL_ARMS_LEAD`]) carry it on and lead it only by
/// that share (riflemen aim by eye with no lead computer, so a fast jet is
/// shot at where it was; fitted, agent, 2026-10-12, John's rule that small
/// arms rarely hit an aircraft passing through). A networked client remakes a
/// burst's rounds with the same observation.
pub fn observation(
    weapon: &WeaponArms,
    target_is_surface: bool,
    position: Vector,
    velocity: Vector,
    age: f64,
) -> TargetObservation {
    let lead = if !target_is_surface && surface_guns::is_small_arms(&weapon.record.source) {
        control::SMALL_ARMS_LEAD
    } else {
        1.
    };
    TargetObservation {
        position: std::array::from_fn(|i| position[i] + velocity[i] * age * lead),
        velocity: std::array::from_fn(|i| velocity[i] * lead),
    }
}

/// The point a gun of `weapon` standing as `place` aims at from `muzzle` for
/// a target observed as `observed`: the lead point of the shared gunsight
/// solution (time of flight and drop), clamped into a barrage zone's fire
/// zone. The burst's aim error is applied on top by the caller. A networked
/// client follows the same lead when it remakes a burst's rounds from the
/// host's first one (protocol 22).
pub fn gun_aim_point(
    weapon: &WeaponArms,
    place: &Place,
    muzzle: Vector,
    observed: TargetObservation,
) -> Vector {
    let launcher = live::Launcher {
        radar_power: true,
        position: muzzle,
        basis: place.basis,
        speed_fps: length(place.velocity),
        velocity: place.velocity,
        bay_ready: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    let solution = gunsight::solve_observed(&weapon.record, &launcher, [0.; 3], Some(observed))
        .ok()
        .flatten();
    let mut aim = match solution {
        Some(solution) => std::array::from_fn(|i| {
            observed.position[i]
                + observed.velocity[i] * solution.seconds
                + if i == 1 { solution.drop_ft } else { 0. }
        }),
        None => observed.position,
    };
    if matches!(weapon.kind, Kind::Barrage { .. }) {
        // Clamp the lead point into the barrage's fire zone.
        let d = sub(aim, muzzle);
        let horizontal = d[0].hypot(d[2]);
        let max = weapon.launch.max_range;
        let scale = if horizontal > max {
            max / horizontal
        } else {
            1.
        };
        aim = [
            muzzle[0] + d[0] * scale,
            muzzle[1] + d[1].clamp(0., weapon.launch.max_altitude.max(0.)),
            muzzle[2] + d[2] * scale,
        ];
    }
    aim
}

/// Where a flak shell fired from `muzzle` bursts, and the ticks it takes to
/// get there: the shell's burst point is `egg` away from the target as the
/// target will be when the shell arrives, so the search is for the arrival
/// time at which the target, carried on at its observed velocity, stands
/// `egg` from the burst point the shell reaches in that time. The shell is
/// flown under its own speed law (it slows after the muzzle, which the
/// gunsight's lead does not know: without this a shell at 15,000 ft burst
/// about half a second short of a jet at 450 kt, which the old proximity fuze
/// hid) and its distance is to the burst point, not the egg's centre, so a
/// burst falls in the egg about the target at the moment it goes off.
/// `first` is the gunsight's lead point, the start of the search.
fn flak_burst_point(
    record: &Weapon,
    muzzle: Vector,
    observed: TargetObservation,
    egg: Vector,
    first: Vector,
) -> (Vector, Option<u64>) {
    let mut centre = first;
    let mut ticks = None;
    let mut burst: Vector = std::array::from_fn(|i| centre[i] + egg[i]);
    for _ in 0..8 {
        burst = std::array::from_fn(|i| centre[i] + egg[i]);
        ticks = live::ticks_to_range(record, length(sub(burst, muzzle)));
        let Some(ticks) = ticks else { break };
        let seconds = ticks as f64 / 120.;
        let next: Vector =
            std::array::from_fn(|i| observed.position[i] + observed.velocity[i] * seconds);
        let moved = length(sub(next, centre));
        centre = next;
        if moved < 1. {
            burst = std::array::from_fn(|i| centre[i] + egg[i]);
            break;
        }
    }
    (burst, ticks)
}

/// Fires a gun's rounds of `fire` at `aircraft` (an aircraft, or a ground
/// unit or ship): the lead point from the
/// shared gunsight solution, the burst's aim error, the round's end at the
/// target's range (or a flak shell's time fuze).
#[allow(clippy::too_many_arguments)]
fn fire_gun(
    state: &mut SurfaceState,
    live: &mut live::State,
    arms: &Arms,
    index: usize,
    weapon: &WeaponArms,
    fire: FireRequest,
    aircraft: &Contact,
    scene: &Scene<'_>,
) {
    let place = state.place(arms);
    // A gun turns toward its target, so its barrel's end is out along the
    // bearing to it.
    let bearing = bearing_elevation(sub(aircraft.position, place.origin)).0;
    let muzzle = place.muzzle(&weapon.mounts[0], bearing, scene.ground);
    let mount = weapon.mounts[0].index;
    let (radar, flak, barrage) = match weapon.kind {
        Kind::Gun { radar, flak } => (radar, flak, None),
        Kind::Barrage { chance, .. } => (false, false, Some(chance)),
        Kind::Missile => return,
    };
    // A visual gun refreshes its observation every 0.5 s and leads from that
    // stale velocity; a radar gun observes every tick.
    let observed = {
        let unit = state.unit_mut(arms.unit).expect("armed");
        let engager = &mut unit.engagers[index];
        let stale = engager.seen.filter(|seen| {
            !radar && seen.target == aircraft.id && scene.tick < seen.tick + VISUAL_REFRESH_TICKS
        });
        let seen = stale.unwrap_or(Seen {
            target: aircraft.id,
            tick: scene.tick,
            position: aircraft.position,
            velocity: aircraft.velocity,
        });
        engager.seen = Some(seen);
        let age = (scene.tick - seen.tick) as f64 / 120.;
        observation(weapon, aircraft.surface, seen.position, seen.velocity, age)
    };
    // Flak does not aim with an angle error: each shell is aimed at its own
    // burst point inside the egg around the lead point (below).
    if fire.burst_start && !flak {
        // The burst's aim error, by skill (a quarter of it at a ground unit
        // or ship); a jammer widens a radar gun's.
        let mut error = if aircraft.surface {
            control::surface_aim_error_deg(arms.skill, radar)
        } else if weapon.tank_gun() {
            // A tank gun's error at an aircraft grows with how fast the
            // gunner must track it.
            let rate = angular_rate_deg_s(
                sub(observed.position, muzzle),
                observed.velocity,
                place.velocity,
            );
            control::tank_air_aim_error_deg(arms.skill, rate)
        } else if surface_guns::is_small_arms(&weapon.record.source) {
            control::small_arms_aim_error_deg(arms.skill)
        } else {
            control::aim_error_deg(arms.skill, radar)
        }
        .to_radians();
        if radar && aircraft.jammer {
            error *= control::JAMMED_RADAR_GUN_ERROR_FACTOR;
        }
        let (radius, angle) = (
            error * unit_draw(&mut state.rng).sqrt(),
            unit_draw(&mut state.rng) * std::f64::consts::TAU,
        );
        let mut offset = [radius * angle.cos(), radius * angle.sin()];
        let mut fires = true;
        if let Some(chance) = barrage {
            // The barrage: a burst fires with its random-fire chance, with
            // the record's offset-fire scatter.
            fires = (draw(&mut state.rng) % 100) < u64::from(chance);
            let spread = |raw: i16| f64::from(raw.max(0)) / UNITS_PER_DEGREE;
            let h = spread(weapon.record.burst.offset_fire_heading).to_radians();
            let p = spread(weapon.record.burst.offset_fire_pitch).to_radians();
            offset[0] += (unit_draw(&mut state.rng) * 2. - 1.) * h;
            offset[1] += (unit_draw(&mut state.rng) * 2. - 1.) * p;
        }
        let engager = &mut state.unit_mut(arms.unit).expect("armed").engagers[index];
        engager.aim_error = offset;
        engager.holding = !fires;
    }
    let (offset, holding) = {
        let engager = &state.unit(arms.unit).expect("armed").engagers[index];
        (engager.aim_error, engager.holding)
    };
    let aim = gun_aim_point(weapon, &place, muzzle, observed);
    let distance = length(sub(aim, muzzle));
    let mut direction = deflect(sub(aim, muzzle), offset[0], offset[1]);
    // A tank gun cannot elevate past its ceiling: a round laid higher (the
    // lead on a climbing line, or the aim error above it) is not fired, and
    // stays in the magazine.
    let holding = holding
        || (!aircraft.surface
            && weapon
                .air_ceiling_deg()
                .is_some_and(|ceiling| hull_elevation_deg(&place.basis, direction) > ceiling));
    let mut end_tick = if flak {
        live::ticks_to_range(&weapon.record, distance)
    } else {
        live::ticks_to_range(&weapon.record, gun_end_range(distance))
    };
    // A jammer widens a radar gun's scatter, the egg's included.
    let widen = if radar && aircraft.jammer {
        control::JAMMED_RADAR_GUN_ERROR_FACTOR
    } else {
        1.
    };
    let (mut fired, mut refused) = (0, 0);
    if !holding {
        for _ in 0..fire.rounds {
            if flak {
                // The shell is set to burst at a point in the egg around the
                // lead point: its direction and its time fuze follow it.
                let draws = [
                    unit_draw(&mut state.rng),
                    unit_draw(&mut state.rng),
                    unit_draw(&mut state.rng),
                ];
                let egg = surface_guns::flak_egg_offset(draws, widen);
                let (burst, ticks) = flak_burst_point(&weapon.record, muzzle, observed, egg, aim);
                direction = attitude::unit(sub(burst, muzzle));
                end_tick = ticks;
            }
            let unit = state.unit_mut(arms.unit).expect("armed");
            let stock = &mut unit.mounts[mount];
            if stock.loaded == 0 {
                break;
            }
            let shot = SurfaceShot {
                owner: arms.unit.0,
                weapon: weapon.record.clone(),
                mount,
                position: muzzle,
                direction,
                velocity: place.velocity,
                target: Some(aircraft.id),
                observation: None,
                ordinal: stock.ordinal,
                end_tick,
            };
            match live.fire_surface(shot) {
                Ok(_) => {
                    let stock = &mut state.unit_mut(arms.unit).expect("armed").mounts[mount];
                    stock.loaded -= 1;
                    stock.ordinal += 1;
                    fired += 1;
                }
                // At capacity the round is not fired and the magazine keeps
                // it.
                Err(_) => refused += 1,
            }
        }
    } else {
        // A barrage burst that does not fire still spends its time; its
        // rounds stay in the magazine.
        refused = 0;
    }
    if fire.burst_start && fired > 0 && !flak {
        // The burst's schedule, for a networked host to tell its clients
        // (protocol 22): a burst of one round is already over.
        let (rounds, span) = state.unit(arms.unit).expect("armed").engagers[index]
            .controller
            .burst()
            .map_or((fired, 1), |(_, rounds, span, _)| (rounds, span));
        state.bursts.push(BurstNote {
            unit: arms.unit,
            mount,
            engager: index,
            target: aircraft.id,
            direction,
            rounds,
            span,
        });
    }
    if fired > 0 || refused > 0 {
        state.trace.push(Trace::Shot {
            unit: arms.unit,
            mount,
            record: weapon.record.source.clone(),
            target: aircraft.id,
            rounds: fired,
            refused,
            opening: fire.opening,
            flak,
        });
    }
}

#[path = "flak_team.rs"]
mod flak_team;
pub use flak_team::{
    FLAK_GROUP_IN_RANGE_MIN, FLAK_GROUP_IN_RANGE_PERCENT, FLAK_GROUP_RADIUS_FT,
    FLAK_HEADING_CONE_DEG, FLAK_HEADING_MIN_SPEED_FPS, FLAK_REACH_MARGIN_FT, FLAK_ROTATION_TICKS,
    FLAK_TEAM_DETECTION_FACTOR, FLAK_TEAM_RADIUS_FT, FlakGroup, FlakTeam, flak_teams,
};
use flak_team::{TeamPlan, plan_teams};

#[cfg(test)]
#[path = "fire_tests.rs"]
mod tests;
