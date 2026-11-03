//! Filling the wire's plain data from `tore-world`'s: the entities from the
//! picture combat takes each tick, and the events from a tick's output.
//!
//! These are the host's conversions; which events go to which connection
//! (the seat's own cues, and the mission-wide ones to everyone) is the host
//! session's choice.

use super::WireResult;
use super::bits::{steps, turn16};
#[cfg(test)]
use super::entity::EntityKind;
use super::entity::{
    AircraftState, DamageState, DebrisState, Devices, EngineState, Entity, EntityState, Motion,
    MoverState, POSITION_STEP, PilotState, ProjectileState, RATE_STEP, ROTOR_SPEED_MAX,
    ROTOR_SPEED_STEP, ROTOR_TILT_STEP, RotorState, SPEED_STEP, Status,
};
use super::events::{LinkEvent, MountView, Rumble, SurfaceUnitView, WireEvent};
use super::names::NameTable;
use super::priority::Relevance;
use tore_formats::aircraft::AircraftId;
use tore_sim::acoustics::Emission;
use tore_sim::combat::live::{DeviceRelease, EffectKind};
use tore_world::comms;
use tore_world::datalink::{DataLink, Entry};
use tore_world::seats::{PlaneId, SeatId};
use tore_world::snapshot::{
    AircraftPose, DebrisPose, Draw, EffectPose, Engine, MarkPose, PilotPose, ProjectilePose,
    RenderSnapshot,
};
use tore_world::surface::SurfacePose;
use tore_world::world::{Cue, OrderReply, Release, World};

fn level(value: f64) -> u8 {
    if value.is_finite() {
        (value.clamp(0., 1.) * 255.).round() as u8
    } else {
        0
    }
}

fn surface(value: f64) -> i8 {
    if value.is_finite() {
        (value.clamp(-1., 1.) * 127.).round() as i8
    } else {
        0
    }
}

fn gun_angle(value: f64) -> i16 {
    if value.is_finite() {
        (value.clamp(-1., 1.) * super::entity::GUN_AIM_STEPS).round() as i16
    } else {
        0
    }
}

fn attitude(angles: [f64; 3]) -> [u16; 3] {
    angles.map(turn16)
}

fn position(p: [f64; 3]) -> [i64; 3] {
    p.map(|v| steps(v, POSITION_STEP))
}

/// An aircraft as drawn, quantized.
pub fn aircraft_state(pose: &AircraftPose) -> AircraftState {
    let devices = pose.devices.map(|d| Devices {
        levels: [d[0], d[1], d[2], d[3], d[4], d[5]].map(level),
        surfaces: [d[6], d[7], d[8]].map(surface),
        speed: steps(d[9], SPEED_STEP).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        throttle: level(d[10]),
        lift_levels: [d[11], d[13], d[14]].map(level),
        vector_yaw: surface(d[12]),
        gun_aim: std::array::from_fn(|i| gun_angle(d[15 + i])),
        gun_group: d[21].clamp(0., 7.) as u8,
    });
    let rate = |v: f64| steps(v, RATE_STEP).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    AircraftState {
        aircraft: pose.aircraft,
        motion: Motion::of(pose.position, pose.velocity),
        attitude: attitude(pose.attitude),
        devices,
        engine: EngineState {
            lit: pose.engine.lit,
            afterburner: pose.engine.afterburner,
            flame: pose.engine.flame,
            rates: pose.engine.rates.map(rate),
        },
        damage: DamageState {
            hp: pose.damage.hp,
            initial_hp: pose.damage.initial_hp,
            sections: pose.damage.sections,
            structural: pose.damage.structural,
        },
        status: Status {
            airborne: pose.airborne,
            crashed: pose.crashed,
            wreck: pose.wreck,
        },
        rotor: rotor_state(&pose.engine),
    }
}

/// A rotorcraft's rotors as drawn, quantized; none where the pose has no
/// rotor speed (every other aircraft).
pub fn rotor_state(engine: &Engine) -> Option<RotorState> {
    (engine.rotor > 0.).then(|| RotorState {
        speed: steps(engine.rotor, ROTOR_SPEED_STEP).clamp(0, i64::from(ROTOR_SPEED_MAX)) as u16,
        tilt: engine
            .rotor_tilt
            .map(|tilt| tilt.map(|v| steps(v, ROTOR_TILT_STEP).clamp(-127, 127) as i8)),
    })
}

/// A projectile as drawn, quantized; `player` is the connection's plane, for
/// whether it is aimed at the player.
pub fn projectile_state(
    pose: &ProjectilePose,
    player: u32,
    names: &mut NameTable,
) -> WireResult<ProjectileState> {
    let velocity: [f64; 3] = std::array::from_fn(|i| {
        (pose.position[i] - pose.previous[i]) * tore_sim::flight::DT.recip()
    });
    let [x, y, z] = pose.direction;
    Ok(ProjectileState {
        owner: pose.owner,
        weapon: names.intern(&pose.weapon)?,
        shape: pose.shape.as_deref().map(|s| names.intern(s)).transpose()?,
        target: pose.target,
        aimed_at_player: pose.target == Some(player),
        motion: Motion::of(pose.position, velocity),
        direction: [turn16(x.atan2(z)), turn16(y.atan2(x.hypot(z)))],
    })
}

fn velocity_between(now: [f64; 3], then: Option<[f64; 3]>, ticks: u64) -> [f64; 3] {
    match then {
        Some(then) if ticks > 0 => {
            let seconds = ticks as f64 * tore_sim::flight::DT;
            std::array::from_fn(|i| (now[i] - then[i]) / seconds)
        }
        _ => [0.; 3],
    }
}

/// The picture the seat flying `plane` draws, built from `world` after a
/// step: its plane is the picture's player and every other plane, plane 0
/// and the other human-flown ones included, is a target
/// ([`tore_world::combat::Combat::snapshot`]). `None` when no cockpit flies
/// `plane`.
pub fn seat_picture(world: &World, plane: PlaneId) -> Option<RenderSnapshot> {
    let cockpit = world
        .cockpits
        .iter()
        .find(|cockpit| cockpit.plane == plane)?;
    world.combat.state.ownship(plane.0)?;
    Some(
        world
            .combat
            .snapshot(plane.0, &cockpit.flight, world.ai_wings.as_ref()),
    )
}

/// The plane an observer's entities are taken for (stage F phase 2): no
/// plane, so every aircraft and every ejected pilot is sent and no missile is
/// aimed at the observer.
pub const NO_PLANE: u32 = u32::MAX;

/// The picture an observer's stream is made from (stage F phase 2,
/// docs/ARCHITECTURE.md "The observer view"): every aircraft in the mission,
/// in real plane ids. While a human flies, it is the first such seat's
/// picture ([`seat_picture`]), whose player pose is an aircraft like any
/// other; with no human-flown plane it is built from combat's targets as
/// [`tore_world::combat::Combat::snapshot`] draws them, its player pose empty
/// (no aircraft). Only what [`entities`] reads is filled in: the aircraft,
/// projectiles, debris and pilots.
pub fn observer_picture(world: &World) -> RenderSnapshot {
    if let Some(picture) = world
        .cockpits
        .iter()
        .find_map(|cockpit| seat_picture(world, cockpit.plane))
    {
        return picture;
    }
    targets_picture(world)
}

/// [`observer_picture`] with no human-flown plane: combat's targets, the
/// projectiles, the debris and the AI's ejected pilots, as the combat
/// snapshot of a mission with drawn models gives them.
fn targets_picture(world: &World) -> RenderSnapshot {
    use tore_world::snapshot::{AircraftPose, Damage};
    let combat = &world.combat;
    let state = &combat.state;
    let actors = || {
        world
            .ai_wings
            .iter()
            .flat_map(|wings| wings.mission().actors())
            .filter(|actor| actor.alive())
    };
    let flying: std::collections::BTreeMap<u32, [f64; tore_world::snapshot::DEVICES]> = actors()
        .map(|actor| (actor.id(), tore_world::snapshot::devices(actor.flight())))
        .collect();
    let burning: std::collections::BTreeSet<u32> = actors()
        .filter(|actor| actor.flight().afterburner_active())
        .map(|actor| actor.id())
        .collect();
    // The AI rotorcraft's rotors, as the combat snapshot draws them.
    let rotors: std::collections::BTreeMap<u32, (f64, f64, [[f64; 2]; 2])> = actors()
        .filter_map(|actor| {
            let flight = actor.flight();
            let (turns, tilt) = tore_world::snapshot::rotor_pose(flight);
            flight
                .rotor_speed_percent()
                .map(|percent| (actor.id(), (percent / 100., turns, tilt)))
        })
        .collect();
    RenderSnapshot {
        tick: state.tick(),
        player: AircraftPose::default(),
        targets: state
            .targets
            .iter()
            .map(|t| AircraftPose {
                id: t.id,
                aircraft: t.aircraft,
                draw: t.aircraft.map_or(Draw::Hidden, Draw::Model),
                position: t.position,
                attitude: tore_world::combat::target_pose(t, combat.ai_poses),
                velocity: t.velocity,
                devices: flying
                    .get(&t.id)
                    .copied()
                    .or_else(|| combat.current_target(t.id).and_then(|pose| pose.devices)),
                engine: {
                    let (rotor, rotor_turns, rotor_tilt) =
                        rotors.get(&t.id).copied().unwrap_or_default();
                    Engine {
                        lit: true,
                        afterburner: false,
                        rates: [0.; 3],
                        rotor,
                        rotor_turns,
                        rotor_tilt,
                        flame: t.airborne && t.hp > 0 && burning.contains(&t.id),
                    }
                },
                damage: Damage {
                    hp: t.hp,
                    initial_hp: t.initial_hp,
                    sections: t.localized_damage.amounts,
                    structural: t.localized_damage.structural_section,
                },
                airborne: t.airborne,
                wreck: t.wreck.as_ref().map(|wreck| wreck.phase),
                crashed: t.hp <= 0,
                stores: Vec::new(),
            })
            .collect(),
        projectiles: state
            .projectiles
            .iter()
            .map(|p| {
                let weapon = state.weapon(p);
                ProjectilePose {
                    id: p.id,
                    owner: p.owner,
                    weapon: weapon.source.clone(),
                    shape: weapon.shape.clone(),
                    gun: tore_sim::combat::live::is_gun(weapon),
                    tracer: p.tracer,
                    position: p.position,
                    previous: p.previous,
                    direction: p.direction,
                    // As the seat picture: an inert body keeps no target cue.
                    target: p.target.filter(|_| p.threatening()),
                    incoming: p.incoming.is_some() && p.threatening(),
                    speed_f8: p.speed_f8,
                    powered: state.motor_powered(p),
                }
            })
            .collect(),
        debris: state
            .debris
            .iter()
            .map(|piece| DebrisPose {
                owner: piece.owner,
                draw: Draw::Hidden,
                position: piece.position,
                attitude: piece.basis.angles(),
                variant: state
                    .targets
                    .iter()
                    .find(|target| target.id == piece.owner)
                    .and_then(|target| target.localized_damage.structural_section)
                    .map(|section| section as usize),
            })
            .collect(),
        pilots: world
            .ai_wings
            .iter()
            .flat_map(|wings| {
                [
                    wings
                        .escapees()
                        .map(|(owner, e)| (owner, false, e))
                        .collect::<Vec<_>>(),
                    wings
                        .crew_escapees()
                        .map(|(owner, e)| (owner, true, e))
                        .collect(),
                ]
                .concat()
            })
            .map(|(owner, crew, escape)| PilotPose {
                owner,
                position: escape.position,
                heading: escape.heading,
                phase: escape.phase,
                crew,
            })
            .collect(),
        surface: combat.surface_poses(),
        ..RenderSnapshot::default()
    }
}

/// What the next entities of a seat or an observer need of its last
/// picture: where each debris piece's owner, each ejected pilot and each
/// surface unit was, the first of each in the picture's order, and the
/// picture's tick. Sorted, so a lookup is a binary search rather than a walk
/// of the last picture for every entity (optimization slice S11).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PictureMemory {
    tick: u64,
    debris: std::sync::Arc<Vec<(u32, [f64; 3])>>,
    pilots: Vec<((u32, bool), [f64; 3])>,
    surface: std::sync::Arc<Vec<(u32, [f64; 3])>>,
    /// The keys of the entities sent with it, for the tests' bookkeeping.
    #[cfg(test)]
    pub(crate) keys: Vec<super::entity::EntityKey>,
    /// The whole picture, for the tests that make entities as they were
    /// made before slice S11.
    #[cfg(test)]
    pub(crate) whole: Option<Box<RenderSnapshot>>,
}

/// `rows` sorted by key, keeping the first row of each key.
fn first_of_each<K: Ord + Copy>(mut rows: Vec<(K, [f64; 3])>) -> Vec<(K, [f64; 3])> {
    // A stable sort keeps rows of one key in their order.
    rows.sort_by_key(|(key, _)| *key);
    rows.dedup_by_key(|(key, _)| *key);
    rows
}

fn find<K: Ord + Copy>(rows: &[(K, [f64; 3])], key: K) -> Option<[f64; 3]> {
    rows.binary_search_by_key(&key, |(k, _)| *k)
        .ok()
        .map(|index| rows[index].1)
}

impl PictureMemory {
    /// What `picture` leaves for the next entities.
    pub fn of(picture: &RenderSnapshot) -> Self {
        Self {
            tick: picture.tick,
            debris: std::sync::Arc::new(first_of_each(
                picture
                    .debris
                    .iter()
                    .map(|d| (d.owner, d.position))
                    .collect(),
            )),
            pilots: first_of_each(
                picture
                    .pilots
                    .iter()
                    .map(|p| ((p.owner, p.crew), p.position))
                    .collect(),
            ),
            surface: std::sync::Arc::new(first_of_each(
                picture
                    .surface
                    .iter()
                    .map(|s| (s.id.0, s.position))
                    .collect(),
            )),
            #[cfg(test)]
            keys: Vec::new(),
            #[cfg(test)]
            whole: None,
        }
    }

    fn debris(&self, owner: u32) -> Option<[f64; 3]> {
        find(&self.debris, owner)
    }

    fn pilot(&self, owner: u32, crew: bool) -> Option<[f64; 3]> {
        find(&self.pilots, (owner, crew))
    }

    fn surface(&self, id: u32) -> Option<[f64; 3]> {
        find(&self.surface, id)
    }

    /// The keys of the entities sent with this picture (tests).
    #[cfg(test)]
    pub(crate) fn keys(&self) -> &[super::entity::EntityKey] {
        &self.keys
    }
}

/// Every entity of `current` a client flying `player` draws: every aircraft
/// but its own (plane 0 included when another seat flies it), every missile,
/// bomb and rocket (gun rounds are burst events), every debris piece, every
/// ejected pilot but its own (its escape is part of its plane's exact
/// state), and every surface unit that follows a route (protocol 22). `current` is the seat's picture ([`seat_picture`]); a picture
/// built for another plane serves too, since its player pose is taken as an
/// aircraft like any other. Debris, pilots and surface units take their
/// velocity from `previous`, the seat's picture a tick or more before. The entities come
/// in key order.
pub fn entities(
    current: &RenderSnapshot,
    previous: Option<&RenderSnapshot>,
    player: u32,
    names: &mut NameTable,
) -> WireResult<Vec<Entity>> {
    let memory = previous.map(PictureMemory::of);
    entities_after(current, memory.as_ref(), player, names)
}

/// [`entities`] with the last picture's [`PictureMemory`].
///
/// Where two entities share a key the first of them is sent: the list is
/// put in key order by a stable sort and the later ones dropped.
pub fn entities_after(
    current: &RenderSnapshot,
    previous: Option<&PictureMemory>,
    player: u32,
    names: &mut NameTable,
) -> WireResult<Vec<Entity>> {
    let mut out = Vec::with_capacity(
        1 + current.targets.len()
            + current.projectiles.len()
            + current.debris.len()
            + current.pilots.len()
            + current.surface.len(),
    );
    for pose in std::iter::once(&current.player).chain(&current.targets) {
        if pose.aircraft.is_none() || pose.id == player {
            continue;
        }
        out.push(Entity {
            id: pose.id,
            state: EntityState::Aircraft(aircraft_state(pose)),
        });
    }
    for pose in current.projectiles.iter().filter(|p| !p.gun) {
        out.push(Entity {
            id: pose.id,
            state: EntityState::Projectile(projectile_state(pose, player, names)?),
        });
    }
    let ticks = previous.map_or(0, |p| current.tick.saturating_sub(p.tick));
    // The aircraft a piece of debris is drawn as: the first pose of its
    // owner's id, found by id.
    let poses_by_id = (!current.debris.is_empty()).then(|| {
        first_of_index(
            std::iter::once(&current.player)
                .chain(&current.targets)
                .map(|pose| pose.id),
        )
    });
    let aircraft_of = |id: u32| -> Option<AircraftId> {
        let index = find_index(poses_by_id.as_deref()?, id)?;
        std::iter::once(&current.player)
            .chain(&current.targets)
            .nth(index)
            .and_then(|pose| pose.aircraft)
    };
    for pose in &current.debris {
        let then = previous.and_then(|p| p.debris(pose.owner));
        let model = match pose.draw {
            Draw::Model(id) => Some(id),
            Draw::Ownship | Draw::Hidden => aircraft_of(pose.owner),
        };
        out.push(Entity {
            id: pose.owner,
            state: EntityState::Debris(debris_state(
                pose,
                model,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    // The own pilot's escape is part of its plane's exact state, but the
    // second crew member of its plane is not: he is sent like any other.
    for pose in current
        .pilots
        .iter()
        .filter(|p| p.owner != player || p.crew)
    {
        let then = previous.and_then(|p| p.pilot(pose.owner, pose.crew));
        out.push(Entity {
            id: super::entity::pilot_id(pose.owner, pose.crew),
            state: EntityState::Pilot(pilot_state(
                pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    for pose in &current.surface {
        let then = previous.and_then(|p| p.surface(pose.id.0));
        out.push(Entity {
            id: pose.id.0,
            state: EntityState::Surface(mover_state(
                pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    out.sort_by_key(Entity::key);
    out.dedup_by_key(|e| e.key());
    Ok(out)
}

/// Each id of `ids` with the index of its first appearance, sorted by id.
fn first_of_index(ids: impl Iterator<Item = u32>) -> Vec<(u32, usize)> {
    let mut rows: Vec<(u32, usize)> = ids.enumerate().map(|(i, id)| (id, i)).collect();
    rows.sort_unstable();
    rows.dedup_by_key(|(id, _)| *id);
    rows
}

fn find_index(rows: &[(u32, usize)], id: u32) -> Option<usize> {
    rows.binary_search_by_key(&id, |(row, _)| *row)
        .ok()
        .map(|at| rows[at].1)
}

/// A round of [`SharedPicture`], quantized but for its names.
#[derive(Clone, Debug)]
struct SharedRound {
    id: u32,
    owner: u32,
    weapon: String,
    shape: Option<String>,
    target: Option<u32>,
    motion: Motion,
    direction: [u16; 2],
}

/// A debris piece of [`SharedPicture`], as a seat flying none of the
/// human-flown planes is sent it, its velocity left to each seat.
#[derive(Clone, Copy, Debug)]
struct SharedDebris {
    owner: u32,
    model: Option<AircraftId>,
    variant: Option<usize>,
    position: [f64; 3],
    attitude: [u16; 3],
}

/// The network's share of one tick's pictures, quantized once for every due
/// seat (optimization slice S11): the aircraft and rounds as they go on the
/// wire, and what each seat's debris, pilots and surface units need. A
/// seat's entities come from it with [`seat_entities`], equal to
/// [`entities`] of the seat's own picture ([`seat_picture`]).
#[derive(Debug)]
pub struct SharedPicture {
    tick: u64,
    /// The aircraft, the first of each id, in id order.
    aircraft: Vec<Entity>,
    rounds: Vec<SharedRound>,
    debris: Vec<SharedDebris>,
    pilots: Vec<tore_world::combat::WirePilot>,
    surface: Vec<SurfacePose>,
    /// The debris and surface parts of every seat's [`PictureMemory`].
    debris_memory: std::sync::Arc<Vec<(u32, [f64; 3])>>,
    surface_memory: std::sync::Arc<Vec<(u32, [f64; 3])>>,
}

impl SharedPicture {
    /// The shared part of `world`'s pictures now; `None` where each seat's
    /// differs throughout ([`tore_world::combat::Combat::wire_picture`]),
    /// when every seat takes its own [`seat_picture`].
    pub fn of(world: &World) -> Option<Self> {
        let picture = world.combat.wire_picture(world.ai_wings.as_ref())?;
        let ids = first_of_index(picture.aircraft.iter().map(|pose| pose.id));
        let aircraft_of =
            |id: u32| find_index(&ids, id).and_then(|index| picture.aircraft[index].aircraft);
        let mut aircraft: Vec<Entity> = picture
            .aircraft
            .iter()
            .filter(|pose| pose.aircraft.is_some())
            .map(|pose| Entity {
                id: pose.id,
                state: EntityState::Aircraft(aircraft_state(pose)),
            })
            .collect();
        aircraft.sort_by_key(|e| e.id);
        aircraft.dedup_by_key(|e| e.id);
        let rounds = picture
            .rounds
            .into_iter()
            .map(|pose| {
                let velocity: [f64; 3] = std::array::from_fn(|i| {
                    (pose.position[i] - pose.previous[i]) * tore_sim::flight::DT.recip()
                });
                let [x, y, z] = pose.direction;
                SharedRound {
                    id: pose.id,
                    owner: pose.owner,
                    weapon: pose.weapon,
                    shape: pose.shape,
                    target: pose.target,
                    motion: Motion::of(pose.position, velocity),
                    direction: [turn16(x.atan2(z)), turn16(y.atan2(x.hypot(z)))],
                }
            })
            .collect();
        let debris: Vec<SharedDebris> = picture
            .debris
            .iter()
            .map(|pose| SharedDebris {
                owner: pose.owner,
                model: match pose.draw {
                    Draw::Model(id) => Some(id),
                    Draw::Ownship | Draw::Hidden => aircraft_of(pose.owner),
                },
                variant: pose.variant,
                position: pose.position,
                attitude: attitude(pose.attitude),
            })
            .collect();
        let debris_memory = std::sync::Arc::new(first_of_each(
            debris.iter().map(|d| (d.owner, d.position)).collect(),
        ));
        let surface_memory = std::sync::Arc::new(first_of_each(
            picture
                .surface
                .iter()
                .map(|s| (s.id.0, s.position))
                .collect(),
        ));
        Some(Self {
            tick: picture.tick,
            aircraft,
            rounds,
            debris,
            pilots: picture.pilots,
            surface: picture.surface,
            debris_memory,
            surface_memory,
        })
    }
}

/// What a seat's own picture adds to [`SharedPicture`]: its own plane, as
/// its cockpit flies it.
#[derive(Clone, Copy, Debug)]
pub struct SeatOwn {
    plane: u32,
    aircraft: Option<AircraftId>,
    damage_variant: Option<usize>,
    /// Its pilot and second crew member, once they eject.
    escape: Option<PilotPose>,
    crew_escape: Option<PilotPose>,
}

impl SeatOwn {
    /// The seat flying `plane` in `world`; `None` when no cockpit flies it
    /// or it has no ownship, as for [`seat_picture`].
    pub fn of(world: &World, plane: PlaneId) -> Option<Self> {
        let cockpit = world
            .cockpits
            .iter()
            .find(|cockpit| cockpit.plane == plane)?;
        let own = world.combat.state.ownship(plane.0)?;
        let pilot = |crew: bool, escape: &tore_sim::ejection::Escape| PilotPose {
            owner: plane.0,
            position: escape.position,
            heading: escape.heading,
            phase: escape.phase,
            crew,
        };
        Some(Self {
            plane: plane.0,
            aircraft: Some(own.configuration().aircraft),
            damage_variant: cockpit.flight.damage_variant,
            escape: cockpit.flight.escape.as_ref().map(|e| pilot(false, e)),
            crew_escape: cockpit.flight.crew_escape.as_ref().map(|e| pilot(true, e)),
        })
    }
}

/// [`entities_after`] of the seat's own picture, from the tick's shared
/// part and the seat's own plane, and what the next one needs of it.
pub fn seat_entities(
    shared: &SharedPicture,
    own: &SeatOwn,
    previous: Option<&PictureMemory>,
    names: &mut NameTable,
) -> WireResult<(Vec<Entity>, PictureMemory)> {
    let player = own.plane;
    let mut out = Vec::with_capacity(
        shared.aircraft.len()
            + shared.rounds.len()
            + shared.debris.len()
            + shared.pilots.len()
            + shared.surface.len()
            + 2,
    );
    out.extend(shared.aircraft.iter().filter(|e| e.id != player).copied());
    for round in &shared.rounds {
        out.push(Entity {
            id: round.id,
            state: EntityState::Projectile(ProjectileState {
                owner: round.owner,
                weapon: names.intern(&round.weapon)?,
                shape: round
                    .shape
                    .as_deref()
                    .map(|s| names.intern(s))
                    .transpose()?,
                target: round.target,
                aimed_at_player: round.target == Some(player),
                motion: round.motion,
                direction: round.direction,
            }),
        });
    }
    let ticks = previous.map_or(0, |p| shared.tick.saturating_sub(p.tick));
    for piece in &shared.debris {
        // The seat's own plane's debris is drawn as its own aircraft, in the
        // damage variant its cockpit shows.
        let (model, variant) = if piece.owner == player {
            (own.aircraft, own.damage_variant)
        } else {
            (piece.model, piece.variant)
        };
        let then = previous.and_then(|p| p.debris(piece.owner));
        out.push(Entity {
            id: piece.owner,
            state: EntityState::Debris(DebrisState {
                owner: piece.owner,
                model,
                variant: variant.map(|v| v.min(7) as u8),
                motion: Motion::of(
                    piece.position,
                    velocity_between(piece.position, then, ticks),
                ),
                attitude: piece.attitude,
            }),
        });
    }
    // The seat's picture's pilots in its order: its own pilot from its
    // cockpit first, the others' and the AI's, then the second crew members
    // the same way. Every one is remembered; the own pilot is not sent.
    let mut pilots = Vec::with_capacity(shared.pilots.len() + 2);
    let others = |crew: bool| {
        shared
            .pilots
            .iter()
            .filter(move |p| p.pose.crew == crew && !(p.human && p.pose.owner == player))
            .map(|p| p.pose)
    };
    let list = own
        .escape
        .into_iter()
        .chain(others(false))
        .chain(own.crew_escape)
        .chain(others(true));
    for pose in list {
        pilots.push(((pose.owner, pose.crew), pose.position));
        if pose.owner == player && !pose.crew {
            continue;
        }
        let then = previous.and_then(|p| p.pilot(pose.owner, pose.crew));
        out.push(Entity {
            id: super::entity::pilot_id(pose.owner, pose.crew),
            state: EntityState::Pilot(pilot_state(
                &pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    for pose in &shared.surface {
        let then = previous.and_then(|p| p.surface(pose.id.0));
        out.push(Entity {
            id: pose.id.0,
            state: EntityState::Surface(mover_state(
                pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    out.sort_by_key(Entity::key);
    out.dedup_by_key(|e| e.key());
    let memory = PictureMemory {
        tick: shared.tick,
        debris: std::sync::Arc::clone(&shared.debris_memory),
        pilots: first_of_each(pilots),
        surface: std::sync::Arc::clone(&shared.surface_memory),
        #[cfg(test)]
        keys: out.iter().map(Entity::key).collect(),
        #[cfg(test)]
        whole: None,
    };
    Ok((out, memory))
}

/// [`entities`] as it was before slice S11, walking the last picture for
/// every entity: kept for the tests that compare the two.
#[cfg(test)]
pub(crate) fn entities_scan(
    current: &RenderSnapshot,
    previous: Option<&RenderSnapshot>,
    player: u32,
    names: &mut NameTable,
) -> WireResult<Vec<Entity>> {
    let mut out = Vec::new();
    let aircraft_of = |id: u32| -> Option<AircraftId> {
        std::iter::once(&current.player)
            .chain(&current.targets)
            .find(|pose| pose.id == id)
            .and_then(|pose| pose.aircraft)
    };
    for pose in std::iter::once(&current.player).chain(&current.targets) {
        if pose.aircraft.is_none() || pose.id == player {
            continue;
        }
        if out
            .iter()
            .any(|e: &Entity| e.id == pose.id && e.state.kind() == EntityKind::Aircraft)
        {
            continue;
        }
        out.push(Entity {
            id: pose.id,
            state: EntityState::Aircraft(aircraft_state(pose)),
        });
    }
    for pose in current.projectiles.iter().filter(|p| !p.gun) {
        out.push(Entity {
            id: pose.id,
            state: EntityState::Projectile(projectile_state(pose, player, names)?),
        });
    }
    let ticks = previous.map_or(0, |p| current.tick.saturating_sub(p.tick));
    for pose in &current.debris {
        let then = previous
            .and_then(|p| p.debris.iter().find(|d| d.owner == pose.owner))
            .map(|d| d.position);
        let model = match pose.draw {
            Draw::Model(id) => Some(id),
            Draw::Ownship | Draw::Hidden => aircraft_of(pose.owner),
        };
        if out
            .iter()
            .any(|e| e.id == pose.owner && e.state.kind() == EntityKind::Debris)
        {
            continue;
        }
        out.push(Entity {
            id: pose.owner,
            state: EntityState::Debris(debris_state(
                pose,
                model,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    // The own pilot's escape is part of its plane's exact state, but the
    // second crew member of its plane is not: he is sent like any other.
    for pose in current
        .pilots
        .iter()
        .filter(|p| p.owner != player || p.crew)
    {
        let id = super::entity::pilot_id(pose.owner, pose.crew);
        let then = previous
            .and_then(|p| {
                p.pilots
                    .iter()
                    .find(|q| (q.owner, q.crew) == (pose.owner, pose.crew))
            })
            .map(|q| q.position);
        if out
            .iter()
            .any(|e| e.id == id && e.state.kind() == EntityKind::Pilot)
        {
            continue;
        }
        out.push(Entity {
            id,
            state: EntityState::Pilot(pilot_state(
                pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    for pose in &current.surface {
        let then = previous
            .and_then(|p| p.surface.iter().find(|q| q.id == pose.id))
            .map(|q| q.position);
        out.push(Entity {
            id: pose.id.0,
            state: EntityState::Surface(mover_state(
                pose,
                velocity_between(pose.position, then, ticks),
            )),
        });
    }
    out.sort_by_key(Entity::key);
    out.dedup_by_key(|e| e.key());
    Ok(out)
}

/// A debris piece, quantized.
pub fn debris_state(
    pose: &DebrisPose,
    model: Option<AircraftId>,
    velocity: [f64; 3],
) -> DebrisState {
    DebrisState {
        owner: pose.owner,
        model,
        variant: pose.variant.map(|v| v.min(7) as u8),
        motion: Motion::of(pose.position, velocity),
        attitude: attitude(pose.attitude),
    }
}

/// An ejected pilot, quantized.
pub fn pilot_state(pose: &PilotPose, velocity: [f64; 3]) -> PilotState {
    PilotState {
        owner: super::entity::pilot_id(pose.owner, pose.crew),
        motion: Motion::of(pose.position, velocity),
        heading: turn16(pose.heading),
        phase: pose.phase,
    }
}

/// A moving surface unit, quantized (protocol 22).
pub fn mover_state(pose: &SurfacePose, velocity: [f64; 3]) -> MoverState {
    MoverState {
        motion: Motion::of(pose.position, velocity),
        attitude: attitude(pose.attitude),
        wrecked: pose.wrecked,
    }
}

/// A unit's hardpoints as a client draws and shows them: a missile rail's
/// loaded rounds and a gun's spare magazines, each from the unit's state, or
/// from its full loads while the state has none yet (`stock` is `None` for a
/// unit as the mission built it).
fn mount_views(
    arms: &tore_world::surface::fire::Arms,
    stock: Option<&[tore_world::surface::MountStock]>,
) -> Vec<MountView> {
    use tore_world::surface::fire::Kind;
    let missile = |index: usize| {
        arms.weapons
            .iter()
            .any(|w| w.kind == Kind::Missile && w.mounts.iter().any(|m| m.index == index))
    };
    let gun = |index: usize| {
        arms.weapons
            .iter()
            .any(|w| w.kind != Kind::Missile && w.mounts.iter().any(|m| m.index == index))
    };
    let small = |n: u32| n.min(u32::from(u16::MAX)) as u16;
    arms.loads
        .iter()
        .enumerate()
        .map(|(index, load)| {
            let now = stock.and_then(|s| s.get(index)).unwrap_or(load);
            MountView {
                rails: missile(index).then(|| small(now.loaded)),
                spares: if gun(index) {
                    now.reserve.map(small)
                } else {
                    None
                },
            }
        })
        .collect()
}

/// Every surface unit's and placed parked aircraft's state as a client
/// draws and shows it (protocol 22): units with a combat row, ascending id.
pub fn surface_views(world: &World) -> Vec<SurfaceUnitView> {
    let surface = &world.terrain.surface;
    let combat = &world.combat;
    let rows: std::collections::BTreeMap<u32, (i32, bool)> = combat
        .state
        .targets
        .iter()
        .filter(|t| t.role == tore_sim::combat::missiles::TargetRole::Surface)
        .map(|t| (t.id, (t.hp, t.radar_emitting)))
        .collect();
    let units = surface.units.iter().map(|u| u.id.0);
    let parked = surface.parked.iter().map(|p| p.id.0);
    let mut ids: Vec<u32> = units
        .chain(parked)
        .filter(|id| rows.contains_key(id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|id| {
            let (hp, radar) = rows[&id];
            let unit = tore_world::surface::UnitId(id);
            let mounts = surface.arsenal.arms(unit).map_or_else(Vec::new, |arms| {
                let stock = combat
                    .surface
                    .unit(unit)
                    .map(|s| s.mounts.as_slice())
                    .filter(|m| !m.is_empty());
                mount_views(arms, stock)
            });
            SurfaceUnitView {
                unit: id,
                hp,
                radar,
                mounts,
            }
        })
        .collect()
}

/// A surface unit's state as the mission built it, which a client assumes
/// for every unit the host has not told it of: full hit points
/// (`initial_hp`), radar off, full rails and spares.
pub fn built_view(
    surface: &tore_world::surface::Surface,
    unit: u32,
    initial_hp: i32,
) -> SurfaceUnitView {
    SurfaceUnitView {
        unit,
        hp: initial_hp,
        radar: false,
        mounts: surface
            .arsenal
            .arms(tore_world::surface::UnitId(unit))
            .map_or_else(Vec::new, |arms| mount_views(arms, None)),
    }
}

/// The relevance a host can work out from positions alone: the distance
/// from the player's plane at `own` and whether a missile is aimed at
/// `player`. The host adds its own flight, the friendly sensors' tracks and
/// the view's subject.
pub fn distance_relevance(entity: &Entity, own: [f64; 3], player: u32) -> Relevance {
    let at = entity.state.motion().position_ft();
    let distance_ft = (0..3).map(|i| (at[i] - own[i]).powi(2)).sum::<f64>().sqrt();
    Relevance {
        distance_ft,
        aimed_at_player: matches!(entity.state, EntityState::Projectile(p) if p.target == Some(player)),
        ..Relevance::NEAR
    }
}

/// The event a cue makes for `seat`: the seat's own cues and the
/// mission-wide wing ejections; `None` for another seat's cues and for the
/// tick's markers, which only a local presenter uses.
pub fn cue_event(cue: &Cue, seat: SeatId, names: &mut NameTable) -> WireResult<Option<WireEvent>> {
    let mine = |s: &SeatId| *s == seat;
    Ok(match cue {
        Cue::Message { seat: s, text } if mine(s) => {
            Some(WireEvent::Message { text: text.clone() })
        }
        Cue::Feedback { seat: s, event } if mine(s) => Some(WireEvent::Feedback {
            rumble: Rumble::of(*event),
        }),
        Cue::Tower { seat: s, stem } if mine(s) => Some(WireEvent::Tower {
            stem: stem.map(|stem| names.intern(stem)).transpose()?,
        }),
        Cue::WeaponCycled { seat: s } if mine(s) => Some(WireEvent::WeaponCycled),
        Cue::WingEjection {
            id,
            message,
            friendly,
        } => Some(WireEvent::WingEjection {
            aircraft: *id,
            message: message.clone(),
            friendly: *friendly,
        }),
        Cue::Radio { seat: s, call } if mine(s) => Some(WireEvent::Radio {
            route: call.route,
            important: call.kind == comms::Kind::Important,
            net: call.net,
            label: call.label.clone(),
            text: call.text.clone(),
            stems: call
                .stems
                .iter()
                .take(super::limits::STEMS)
                .map(|stem| names.intern(stem))
                .collect::<WireResult<_>>()?,
        }),
        Cue::OrderVoice { seat: s, stems } if mine(s) => Some(WireEvent::OrderVoice {
            stems: stems
                .iter()
                .take(super::limits::STEMS)
                .map(|stem| names.intern(stem))
                .collect::<WireResult<_>>()?,
        }),
        _ => None,
    })
}

/// The data link's journal entries a seat flying `plane` is sent (slice G7):
/// those about a member of its own flight, in the journal's order. A plane
/// the picture does not know is sent none.
pub fn link_events(link: &DataLink, plane: u32, entries: &[Entry]) -> Vec<WireEvent> {
    let Some(flight) = link.member(plane).map(|m| m.flight) else {
        return Vec::new();
    };
    entries
        .iter()
        .map(LinkEvent::of)
        .filter(|event| {
            link.member(event.plane())
                .is_some_and(|member| member.flight == flight)
        })
        .map(WireEvent::Link)
        .collect()
}

/// A weapon release sound for its seat.
pub fn release_event(release: &Release, names: &mut NameTable) -> WireResult<WireEvent> {
    Ok(WireEvent::Release {
        sound: names.intern(&release.sound)?,
        station: release.station.min(255) as u8,
    })
}

/// What became of a wing order, for the seat that gave it.
pub fn order_event(reply: &OrderReply) -> WireEvent {
    WireEvent::OrderReply {
        order: reply.order,
        outcome: reply.outcome.clone(),
    }
}

/// A sound emission.
pub fn sound_event(emission: &Emission, from: Option<u32>) -> WireEvent {
    WireEvent::Sound {
        kind: emission.kind,
        position: position(emission.position),
        arrived: emission.arrived,
        from,
    }
}

/// A flash, hit or explosion as combat draws it.
pub fn effect_event(effect: &EffectPose) -> WireEvent {
    WireEvent::Effect {
        kind: effect.kind,
        position: position(effect.position),
        ticks: effect.ticks,
        blast: effect.blast,
        scale: effect.scale,
    }
}

/// A crater or crash-site fire.
pub fn mark_event(mark: &MarkPose) -> WireEvent {
    WireEvent::Mark {
        kind: mark.kind,
        position: position(mark.position),
        scale: mark.scale,
    }
}

/// Chaff or a flare leaving an aircraft.
pub fn countermeasure_event(release: &DeviceRelease) -> WireEvent {
    WireEvent::Countermeasure {
        aircraft: release.owner,
        flare: release.kind == EffectKind::Flare,
        position: position(release.release.position),
        velocity: release
            .release
            .velocity
            .map(|v| steps(v, super::entity::VELOCITY_STEP)),
        attitude: attitude(release.release.basis.angles()),
        number: release.number,
        left: release.left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_world::mission::{MissionSpec, Skill, Start};
    use tore_world::seats::SeatInput;
    use tore_world::test_support::resources::{THEATER, resources};
    use tore_world::world::{MissionCommand, Seating, TickOutput};

    #[test]
    fn a_rotorcrafts_rotors_are_quantized_for_the_wire_and_come_back() {
        let engine = Engine {
            rotor: 0.8437,
            rotor_turns: 1_234.5,
            rotor_tilt: [[0.1, -0.0123], [0.6, -0.6]],
            ..Engine::default()
        };
        let state = rotor_state(&engine).expect("a rotorcraft");
        assert_eq!(state.speed, 844);
        assert_eq!(state.tilt, [[26, -3], [127, -127]]);
        let (speed, tilt) = crate::client::interpolation::rotor_of(&AircraftState {
            rotor: Some(state),
            ..AircraftState::default()
        });
        assert!((speed - engine.rotor).abs() <= ROTOR_SPEED_STEP / 2.);
        assert!((tilt[0][0] - 0.1).abs() <= ROTOR_TILT_STEP / 2.);
        assert!((tilt[0][1] + 0.0123).abs() <= ROTOR_TILT_STEP / 2.);
        // Past the wire's reach a tilt holds at its end; the blade angle is
        // the client's own, so it is not sent.
        assert!((tilt[1][0] - 127. * ROTOR_TILT_STEP).abs() < 1e-12);
        // A fixed-wing aircraft (no rotor speed) sends no rotor group.
        assert_eq!(rotor_state(&Engine::default()), None);
        let overspeed = Engine {
            rotor: 9.,
            ..engine
        };
        assert_eq!(rotor_state(&overspeed).unwrap().speed, ROTOR_SPEED_MAX);
    }

    fn aircraft_ids(entities: &[Entity]) -> Vec<u32> {
        entities
            .iter()
            .filter(|e| e.state.kind() == EntityKind::Aircraft)
            .map(|e| e.id)
            .collect()
    }

    fn spec() -> MissionSpec {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 2;
        spec.wings[3].count = 2;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 2;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        spec
    }

    /// Steps `world` 30 ticks, `takes` handing planes to seats on the first.
    fn fly(world: &mut World, takes: &[(u8, u32)], seats: &[u8]) {
        let mut out = TickOutput::default();
        let take: Vec<MissionCommand> = takes
            .iter()
            .map(|&(seat, plane)| MissionCommand::Take {
                seat: SeatId(seat),
                plane: PlaneId(plane),
            })
            .collect();
        for tick in 0..30 {
            let inputs: Vec<SeatInput> = seats
                .iter()
                .map(|&seat| SeatInput {
                    seat: SeatId(seat),
                    tick: world.tick(),
                    ..SeatInput::default()
                })
                .collect();
            let mission: &[MissionCommand] = if tick == 0 { &take } else { &[] };
            world
                .step_with(mission, &inputs, &mut out, |_, _| Ok(()))
                .unwrap();
        }
    }

    /// The aircraft the seat flying `plane` is sent.
    fn sent_to(world: &World, plane: u32) -> Vec<Entity> {
        let picture = seat_picture(world, PlaneId(plane)).unwrap();
        entities(&picture, None, plane, &mut NameTable::new()).unwrap()
    }

    fn check_plane_0_reaches(world: &World, plane: u32) {
        let lead = sent_to(world, plane)
            .into_iter()
            .find(|e| e.id == 0 && e.state.kind() == EntityKind::Aircraft)
            .expect("plane 0 is sent to the other seat");
        let cockpit = world
            .cockpits
            .iter()
            .find(|c| c.plane == PlaneId(0))
            .unwrap();
        assert_eq!(
            *lead.state.motion(),
            Motion::of(cockpit.flight.position, cockpit.flight.velocity)
        );
    }

    /// John, 2026-10-06: a two-seater's second crew member has a chute of his
    /// own. Every client gets it, the flying seat's own plane's too (the own
    /// pilot's chute is in the plane's exact state, his is not), under an id
    /// of its own, and a client reads the plane and the seat back.
    #[test]
    fn a_two_seaters_second_chute_has_an_id_of_its_own_and_reaches_every_client() {
        use tore_sim::ejection::Phase;
        let pose = |owner: u32, crew: bool, x: f64| PilotPose {
            owner,
            position: [x, 3000., 0.],
            heading: 0.5,
            phase: Phase::Seat,
            crew,
        };
        let current = RenderSnapshot {
            pilots: vec![
                pose(7, false, 0.),
                pose(9, false, 10.),
                pose(7, true, 20.),
                pose(9, true, 30.),
            ],
            ..RenderSnapshot::default()
        };
        let sent = entities(&current, None, 7, &mut NameTable::new()).unwrap();
        let pilots: Vec<(u32, u32)> = sent
            .iter()
            .filter_map(|e| match e.state {
                EntityState::Pilot(p) => Some((e.id, p.owner)),
                _ => None,
            })
            .collect();
        let crew = |owner| crate::wire::entity::pilot_id(owner, true);
        assert_eq!(
            pilots,
            [(9, 9), (crew(7), crew(7)), (crew(9), crew(9))],
            "the own pilot is in the exact state; the three others are sent, in id order"
        );
        assert!(crew(7) != 7 && crew(9) != 9);
        for (id, owner, seat) in [(9, 9, false), (crew(7), 7, true), (crew(9), 9, true)] {
            assert_eq!(crate::wire::entity::pilot_owner(id), (owner, seat));
        }
        // Other clients get the own plane's two chutes.
        let other = entities(&current, None, 3, &mut NameTable::new()).unwrap();
        assert_eq!(
            other
                .iter()
                .filter(|e| e.state.kind() == EntityKind::Pilot)
                .count(),
            4
        );
        // Through the wire coder both survive.
        let again: Vec<Entity> = other
            .iter()
            .filter(|e| e.state.kind() == EntityKind::Pilot)
            .copied()
            .collect();
        assert_eq!(again.len(), 4);
        assert!(again.windows(2).all(|w| w[0].key() < w[1].key()));
    }

    #[test]
    fn each_seat_gets_every_plane_but_its_own() {
        // Single player's mission with a second seat in plane 1.
        let map = resources();
        let mut world = World::new(&spec(), &map, Seating::SinglePlayer).unwrap();
        fly(&mut world, &[(1, 1)], &[0, 1]);
        assert!(
            seat_picture(&world, PlaneId(2)).is_none(),
            "the AI flies plane 2"
        );
        assert_eq!(aircraft_ids(&sent_to(&world, 0)), [1, 2, 3]);
        assert_eq!(aircraft_ids(&sent_to(&world, 1)), [0, 2, 3]);
        check_plane_0_reaches(&world, 1);

        // An open mission, as a server runs one: nobody at first, then two
        // seats in planes 0 and 1.
        let mut world = World::new(&spec(), &map, Seating::Open).unwrap();
        fly(&mut world, &[], &[]);
        assert!(seat_picture(&world, PlaneId(0)).is_none(), "no human yet");
        fly(&mut world, &[(1, 0), (2, 1)], &[1, 2]);
        assert_eq!(aircraft_ids(&sent_to(&world, 0)), [1, 2, 3]);
        assert_eq!(aircraft_ids(&sent_to(&world, 1)), [0, 2, 3]);
        check_plane_0_reaches(&world, 1);
    }

    /// Slice S11: a fight with three seats (one ejects, two crew), the AI
    /// shooting missiles and losing aircraft. Every seat's entities from
    /// the shared picture, and the merge-walk [`entities`], equal what the
    /// per-seat picture and the walk of the last picture gave, tick by
    /// tick, names and velocities included.
    #[test]
    fn the_shared_picture_sends_every_seat_what_its_own_picture_sent() {
        use std::collections::BTreeMap;
        let map = resources();
        let mut spec = spec();
        spec.wings[0].count = 4;
        spec.wings[3].count = 4;
        spec.wings[3].skill = Skill::Ace;
        spec.separation_nm = 5;
        let mut world = World::new(&spec, &map, Seating::Open).unwrap();
        let seats: [(u8, u32); 3] = [(1, 0), (2, 1), (3, 2)];
        let mut out = TickOutput::default();
        let mut old_previous: BTreeMap<u32, RenderSnapshot> = BTreeMap::new();
        let mut merged_previous: BTreeMap<u32, PictureMemory> = BTreeMap::new();
        let mut new_previous: BTreeMap<u32, PictureMemory> = BTreeMap::new();
        let mut names: BTreeMap<(u32, u8), NameTable> = BTreeMap::new();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut missiles = 0;
        for tick in 0..4800u64 {
            let take: Vec<MissionCommand> = if tick == 0 {
                seats
                    .iter()
                    .map(|&(seat, plane)| MissionCommand::Take {
                        seat: SeatId(seat),
                        plane: PlaneId(plane),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let inputs: Vec<SeatInput> = seats
                .iter()
                .map(|&(seat, _)| {
                    // Seat 2 fires its gun, seat 3 its missiles at whatever
                    // its radar gives it.
                    let mut input = SeatInput {
                        seat: SeatId(seat),
                        tick: world.tick(),
                        trigger: (seat == 2 && tick % 240 < 40) || (seat == 3 && tick % 300 < 20),
                        ..SeatInput::default()
                    };
                    if seat == 3 && tick == 10 {
                        input
                            .commands
                            .push(tore_world::seats::SeatCommand::CycleWeapon { forward: true });
                    }
                    if seat == 3 && tick % 300 == 290 {
                        input.commands.push(tore_world::seats::SeatCommand::Combat(
                            tore_sim::combat::live::Command::Designate,
                        ));
                    }
                    input
                })
                .collect();
            world
                .step_with(&take, &inputs, &mut out, |_, _| Ok(()))
                .unwrap();
            // Seat 1's pilot and second crew member eject.
            if tick == 700 {
                let cockpit = world
                    .cockpits
                    .iter_mut()
                    .find(|c| c.plane == PlaneId(0))
                    .unwrap();
                let flight = &mut cockpit.flight;
                let basis = tore_sim::attitude::Basis::new(flight.yaw, flight.pitch, flight.bank);
                flight.escape = Some(tore_sim::ejection::Escape::new(
                    flight.position,
                    flight.velocity,
                    basis,
                ));
                let mut crew = flight.position;
                crew[0] += 12.;
                flight.crew_escape = Some(tore_sim::ejection::Escape::new(
                    crew,
                    flight.velocity,
                    basis,
                ));
            }
            // Debris of a seat's own plane, of another seat's and of the
            // AI's, two pieces of one owner among them.
            if [300, 302, 900, 1500].contains(&tick) {
                let owner = match tick {
                    300 | 302 => 0,
                    900 => 1,
                    _ => 5,
                };
                let at = world
                    .combat
                    .state
                    .targets
                    .iter()
                    .find(|t| t.id == 5)
                    .map_or([0., 9000., 0.], |t| t.position);
                world
                    .combat
                    .state
                    .debris
                    .push(tore_sim::combat::debris::Piece {
                        owner,
                        variant: (tick % 3) as usize,
                        position: [at[0] + tick as f64, at[1], at[2]],
                        velocity: [100., 0., 50.],
                        basis: tore_sim::attitude::Basis::new(0.3, 0.1, -0.2),
                    });
            }
            missiles += world.combat.state.projectiles.len();
            if tick % 2 != 1 {
                continue;
            }
            let shared = SharedPicture::of(&world).expect("an open mission's pictures share");
            for &(_, plane) in &seats {
                let Some(picture) = seat_picture(&world, PlaneId(plane)) else {
                    assert!(SeatOwn::of(&world, PlaneId(plane)).is_none());
                    continue;
                };
                let mut table = |n: u8| names.remove(&(plane, n)).unwrap_or_default();
                let (mut old_names, mut merged_names, mut new_names) =
                    (table(0), table(1), table(2));
                let old = entities_scan(&picture, old_previous.get(&plane), plane, &mut old_names)
                    .unwrap();
                let merged = entities_after(
                    &picture,
                    merged_previous.get(&plane),
                    plane,
                    &mut merged_names,
                )
                .unwrap();
                let own = SeatOwn::of(&world, PlaneId(plane)).unwrap();
                let (new, memory) =
                    seat_entities(&shared, &own, new_previous.get(&plane), &mut new_names).unwrap();
                assert_eq!(merged, old, "plane {plane} at tick {tick}: merge walk");
                assert_eq!(new, old, "plane {plane} at tick {tick}: shared picture");
                let text = |n: &NameTable| format!("{:?} {:?}", n.names(), n.clone().take_new());
                assert_eq!(text(&merged_names), text(&old_names));
                assert_eq!(text(&new_names), text(&old_names));
                for e in &old {
                    let kind = match e.state {
                        EntityState::Aircraft(_) => "aircraft",
                        EntityState::Projectile(_) => "rounds",
                        EntityState::Debris(d) if d.owner == plane => "own debris",
                        EntityState::Debris(_) => "debris",
                        EntityState::Pilot(p)
                            if p.owner == crate::wire::entity::pilot_id(plane, true) =>
                        {
                            "own crew"
                        }
                        EntityState::Pilot(_) => "pilots",
                        EntityState::Surface(_) => "surface",
                    };
                    *counts.entry(kind).or_default() += 1;
                }
                merged_previous.insert(plane, PictureMemory::of(&picture));
                old_previous.insert(plane, picture);
                new_previous.insert(plane, memory);
                names.insert((plane, 0), old_names);
                names.insert((plane, 1), merged_names);
                names.insert((plane, 2), new_names);
            }
        }
        eprintln!("{counts:?}, {missiles} rounds in flight-ticks");
        for kind in ["aircraft", "rounds", "debris", "pilots", "own crew"] {
            assert!(
                counts.get(kind).is_some_and(|n| *n > 0),
                "no {kind}: {counts:?}"
            );
        }
    }

    /// What an observer is sent of `world`.
    fn observed(world: &World) -> Vec<Entity> {
        entities(
            &observer_picture(world),
            None,
            NO_PLANE,
            &mut NameTable::new(),
        )
        .unwrap()
    }

    #[test]
    fn an_observer_gets_every_plane_with_or_without_a_human() {
        // Nobody flies: the AI's aircraft from combat's targets.
        let map = resources();
        let mut world = World::new(&spec(), &map, Seating::Open).unwrap();
        fly(&mut world, &[], &[]);
        assert!(world.combat.state.ownships().is_empty());
        let alone = observed(&world);
        assert_eq!(aircraft_ids(&alone), [0, 1, 2, 3]);

        // Two humans, in planes 0 and 2: every plane, theirs included, at
        // their cockpits' positions, and the AI's as a seat sees them.
        fly(&mut world, &[(1, 0), (2, 2)], &[1, 2]);
        let all = observed(&world);
        assert_eq!(aircraft_ids(&all), [0, 1, 2, 3]);
        for plane in [0, 2] {
            let cockpit = world
                .cockpits
                .iter()
                .find(|c| c.plane == PlaneId(plane))
                .unwrap();
            let entity = all
                .iter()
                .find(|e| e.id == plane && e.state.kind() == EntityKind::Aircraft)
                .unwrap();
            assert_eq!(
                *entity.state.motion(),
                Motion::of(cockpit.flight.position, cockpit.flight.velocity)
            );
        }
        let seat = sent_to(&world, 0);
        for id in [1, 3] {
            let ai = |list: &[Entity]| {
                list.iter()
                    .find(|e| e.id == id && e.state.kind() == EntityKind::Aircraft)
                    .cloned()
            };
            assert_eq!(ai(&all), ai(&seat), "AI plane {id}");
            // Built from combat's targets alone, as with no human.
            let fallback = entities(
                &targets_picture(&world),
                None,
                NO_PLANE,
                &mut NameTable::new(),
            )
            .unwrap();
            assert_eq!(ai(&fallback), ai(&seat), "AI plane {id} from the targets");
        }
    }
}
