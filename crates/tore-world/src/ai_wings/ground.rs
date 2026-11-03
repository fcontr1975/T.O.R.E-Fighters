//! The bridge half of the strikers' ground picture (docs/spec/ai-strike.md,
//! "Who strikes" and "What the AI knows"). The picture itself, its lists, its
//! memory and its threat inputs live with each actor in
//! [`tore_sim::ai::awareness::ground`]; this module only:
//!
//! - briefs each Strike actor once on the mission's ground objective: the
//!   Quick Mission destroy targets (flagged 0x80) of another side than its
//!   own, their centre, and for a SEAD striker (an anti-radiation store
//!   aboard) the objective's air defences;
//! - describes the ground objects to the mission when a picture is due: what
//!   the ground lists read of each combat row, its side and, for an armed
//!   unit or a radar, its reach and whether it can guide; whether it is a
//!   hidden shoulder-fired SAM team, and which of those have a round in the
//!   air at an aircraft (a launch gives a team away).
//!
//! It holds no state. With no striker it costs one scan of the actors and
//! changes nothing, so air-to-air missions are untouched.
use super::AiWings;
use crate::surface::rotor_reach::RotorReach;
use crate::surface::{Surface, UnitId, fire};
use crate::terrain::Terrain;
use tore_sim::ai::awareness::ground::{DefenseFacts, GroundScene, GroundSubject};
use tore_sim::ai::engagement::{SEAD_BRIEFING_RADIUS_FT, StrikeObjective, StrikeRole};
use tore_sim::ai::targeting::Side;
use tore_sim::combat::live;
use tore_sim::combat::missiles::{Rules, TargetRole};

/// The anti-radiation seeker signature (docs/formats/radar.md, "HARM page").
const ANTI_RADIATION: u8 = 4;

/// One step's ground picture work, before the mission steps: brief new
/// strikers, then refresh the pictures that are due.
pub(super) fn step(
    wings: &mut AiWings,
    state: &live::State,
    world: &Terrain,
    ground: &(dyn Fn(f64, f64) -> f64 + Sync),
) {
    brief(wings, state, &world.surface);
    if !wings.mission.ground_due() {
        return;
    }
    let subjects = subjects(state, &world.surface, &mut wings.rotor_reach);
    let painted = revealed(&wings.painting, &world.surface.arsenal);
    let launched = launched(state);
    wings.mission.observe_ground(
        GroundScene {
            subjects: &subjects,
            weather: state.sensor_weather(),
            painted: &painted,
            launched: &launched,
        },
        ground,
    );
}

/// The hidden shoulder-fired teams' rounds in the air, as (aircraft aimed
/// at, team) pairs, ascending (T.O.R.E opinionated, John 2026-10-29: a
/// launch gives the team away). A round lives for seconds, so the
/// quarter-second refresh never misses one.
fn launched(state: &live::State) -> Vec<(u32, u32)> {
    let mut pairs: Vec<(u32, u32)> = state
        .projectiles
        .iter()
        .filter(|p| p.gun_round.is_none())
        .filter_map(|p| Some((p.target?, p.owner)))
        .filter(|(_, owner)| state.ground_listing(*owner).is_some_and(|l| l.team))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// What a radar painting an aircraft gives away (slice H4): the radar, and
/// for a battery's radar the launchers it guides for, as (aircraft, unit)
/// pairs.
fn revealed(painting: &[(u32, u32)], arsenal: &fire::Arsenal) -> Vec<(u32, u32)> {
    let mut pairs = Vec::new();
    for &(plane, emitter) in painting {
        pairs.push((plane, emitter));
        for battery in arsenal.batteries.iter().filter(|b| b.radar.0 == emitter) {
            pairs.extend(battery.launchers.iter().map(|unit| (plane, unit.0)));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// Briefs every Strike actor not yet briefed.
fn brief(wings: &mut AiWings, state: &live::State, surface: &Surface) {
    let waiting: Vec<(u32, Side, bool)> = wings
        .mission
        .actors()
        .iter()
        .filter(|actor| actor.needs_briefing())
        .map(|actor| {
            let sead = actor.stations().iter().any(|spec| {
                !spec.is_empty()
                    && wings
                        .weapons
                        .get(&(actor.id(), spec.station.0))
                        .is_some_and(|w| w.seeker.signature == ANTI_RADIATION)
            });
            (actor.id(), actor.identity().side, sead)
        })
        .collect();
    for (id, side, sead) in waiting {
        let objective = objective(state, surface, side, sead);
        if let Some(actor) = wings.mission.actor_mut(id) {
            actor.set_strike_objective(objective);
        }
    }
}

/// The objective a striker of `side` is briefed on: the mission's destroy
/// targets that belong to another side (none for Redfor in Quick Mission,
/// whose targets are its own), their centre, and for a SEAD striker the
/// hostile armed units and radars within 5 nm of that centre, the hidden
/// shoulder-fired teams aside.
pub(super) fn objective(
    state: &live::State,
    surface: &Surface,
    side: Side,
    sead: bool,
) -> StrikeObjective {
    let row = |id: u32| state.targets.iter().find(|t| t.id == id);
    let hostile = |id: u32| row(id).is_some_and(|t| t.side != side && t.side != live::NO_SIDE);
    let targets: Vec<u32> = super::outcome::ground_targets(surface, state)
        .into_iter()
        .filter(|id| hostile(*id))
        .collect();
    let point = (!targets.is_empty()).then(|| {
        let mut sum = [0.; 3];
        for id in &targets {
            let position = row(*id).map_or([0.; 3], |t| t.position);
            for (total, value) in sum.iter_mut().zip(position) {
                *total += value;
            }
        }
        sum.map(|total| total / targets.len() as f64)
    });
    let defenses = match point {
        Some(point) if sead => surface
            .arsenal
            .units
            .iter()
            .filter(|arms| !arms.weapons.is_empty() || arms.radar.is_some())
            // No briefing knows where a shoulder-fired team hides (T.O.R.E
            // opinionated, John 2026-10-29).
            .filter(|arms| !arms.team)
            .map(|arms| arms.unit.0)
            .filter(|id| !targets.contains(id) && hostile(*id))
            .filter(|id| {
                row(*id).is_some_and(|t| {
                    (t.position[0] - point[0]).hypot(t.position[2] - point[2])
                        <= SEAD_BRIEFING_RADIUS_FT
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    StrikeObjective {
        targets,
        defenses,
        point,
    }
}

/// Every object a ground list may hold, as the mission reads it: the
/// surface rows and the aircraft on the ground or in an airfield sequence,
/// in row order. Flying aircraft are the air picture's.
pub(super) fn subjects(
    state: &live::State,
    surface: &Surface,
    rotor: &mut RotorReach,
) -> Vec<GroundSubject> {
    state
        .targets
        .iter()
        .filter(|t| t.role == TargetRole::Surface || !t.airborne || state.airfield_attached(t.id))
        .map(|t| {
            let arms = surface.arsenal.arms(UnitId(t.id));
            GroundSubject {
                object: state.ground_object(t),
                side: t.side,
                defense: arms
                    .filter(|arms| !arms.weapons.is_empty() || arms.radar.is_some())
                    .map(|arms| defense(arms, t, state, &surface.arsenal, rotor)),
                // The host marks the teams that are no mission target.
                hidden: state.ground_listing(t.id).is_some_and(|l| l.team),
            }
        })
        .collect()
}

/// An armed unit's or radar's defence facts: the farthest launch zone of its
/// missiles and of its guns, how far its missiles may be launched at a
/// helicopter ([`rotor_reach`]), whether it can guide (its battery's radar
/// stands, or it guides by itself), and its heading.
fn defense(
    arms: &fire::Arms,
    row: &live::Target,
    state: &live::State,
    arsenal: &fire::Arsenal,
    rotor: &mut RotorReach,
) -> DefenseFacts {
    let reach = |missile: bool| {
        arms.weapons
            .iter()
            .filter(|w| matches!(w.kind, fire::Kind::Missile) == missile)
            .map(|w| w.launch.max_range)
            .fold(0., f64::max)
    };
    let alive = |id: u32| state.targets.iter().any(|t| t.id == id && t.hp > 0);
    let working = match arms.battery {
        Some(index) => arsenal
            .batteries
            .iter()
            .find(|battery| battery.index == index)
            .is_none_or(|battery| alive(battery.radar.0)),
        None => true,
    };
    let forward = row.basis.forward;
    DefenseFacts {
        missile_reach_ft: reach(true),
        rotor_reach_ft: rotor_reach(arms, state, arsenal, rotor),
        gun_reach_ft: reach(false),
        radar: arms.radar.is_some(),
        working,
        ship: arms.ship,
        heading_deg: forward[0].atan2(forward[2]).to_degrees().rem_euclid(360.),
    }
}

/// How far an armed unit's missiles may be launched at a helicopter, feet
/// (a helicopter's threat ring, docs/spec/ai-strike.md, "Helicopter threat
/// awareness"; John, 2026-10-29): under the energy rules the bound of
/// [`RotorReach`] on each missile it fires, its own and, for a battery's
/// launcher, the battery's (at the battery's best skill), else its launch
/// zone; under the other rules the launch zone, which is where those rules
/// stop it. 0 with no missiles.
fn rotor_reach(
    arms: &fire::Arms,
    state: &live::State,
    arsenal: &fire::Arsenal,
    rotor: &mut RotorReach,
) -> f64 {
    let battery = arms
        .battery
        .and_then(|index| arsenal.batteries.iter().find(|b| b.index == index))
        .filter(|battery| battery.launchers.contains(&arms.unit))
        .map(|battery| &battery.weapon);
    arms.weapons
        .iter()
        .filter(|w| matches!(w.kind, fire::Kind::Missile))
        .chain(battery)
        .map(|w| {
            let energy = (state.weapon_rules == Rules::Energy)
                .then(|| rotor.reach_ft(w, arms.position[1], state.devices.wind))
                .flatten();
            energy.unwrap_or(w.launch.max_range)
        })
        .fold(0., f64::max)
}

impl AiWings {
    /// Gives AI aircraft `id` an air-to-ground role. Strike briefs it on the
    /// mission's ground objective at the next step and starts its ground
    /// picture; CAP (every aircraft's default) drops it. The Quick Mission
    /// role setting (ST-O) and the tests call this. Returns false for an id
    /// that is not an AI aircraft.
    pub fn set_strike_role(&mut self, id: u32, role: StrikeRole) -> bool {
        match self.mission.actor_mut(id) {
            Some(actor) => {
                actor.set_strike_role(role);
                true
            }
            None => false,
        }
    }

    /// Orders AI aircraft `id` onto ground object `target`, or clears the
    /// order (wing orders and takeover, ST-O and ST-K). Returns false for an
    /// id that is not an AI aircraft.
    pub fn set_strike_order(&mut self, id: u32, target: Option<u32>) -> bool {
        match self.mission.actor_mut(id) {
            Some(actor) => {
                actor.set_strike_order(target);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "ground_tests.rs"]
mod tests;
