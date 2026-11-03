//! Quick Mission launch geometry and placement: where the enemy group goes so
//! it stays on the map, where the player's wing parks for a ground start, and
//! the layout a restart rebuilds. The rules and numbers are in
//! `docs/spec/quick-mission-menu.md` ("Mission wings" and "Player ground
//! start"). The creator's screen is in `quick_mission`; nothing here reaches
//! the menu, the rocker, the loadout screen or any drawing type, so the mission
//! core can use it. The geometry functions are plain functions of numbers, so
//! every rule can be tested with synthetic bounds and runways.

pub mod availability;
pub mod distance_table;
pub mod seed_retry;

use crate::{mission::Situation, terrain::Terrain};
use tore_sim::{
    ai::airfield::AirfieldAnchors,
    airport::{ApproachEnd, Runway},
};

/// The manual (p.19) gives the separation in nautical miles.
pub const FEET_PER_NM: f64 = 6_076.12;

/// Separation choices in nautical miles. The first six are the retail list;
/// 100, 150, 200 and 300 are host additions requested by John on 2026-09-23,
/// and 75 on 2026-10-09.
pub const SEPARATION_NM: [f64; 11] = [1., 2., 5., 10., 20., 50., 75., 100., 150., 200., 300.];
/// Retail entries in the imported separation list.
pub const RETAIL_SEPARATIONS: usize = 6;

/// `fitted`, agent decision 2026-09-23: keep every enemy aircraft at least one
/// terrain cell (8192 ft) inside the map edge, so none starts over the
/// repeated edge terrain beyond it.
pub const MAP_MARGIN_CELLS: f64 = 1.;
/// `fitted`, agent decision: bearings are searched in one-degree steps.
pub const AIM_STEP_DEG: i32 = 1;

/// `fitted`, agent decision 2026-09-23: runway parking for the player's wing.
/// Each aircraft stands this far behind the one ahead of it, trying the
/// larger spacing first.
pub const GROUND_SPACING_FT: [f64; 4] = [250., 200., 150., 100.];
/// Wingmen stand this far right (odd) or left (even) of the centerline. With
/// the widest selectable wingspan, the F-14's 64 ft spread, the wingtips stay
/// within 72 ft of the centerline, and alternate sides keep neighbours 80 ft
/// apart laterally as well as a full spacing apart along the runway.
pub const GROUND_LATERAL_FT: f64 = 40.;
/// Requested taxiway queue; fitted spacing and hold distance, in feet.
pub const TAXI_QUEUE_SPACING_FT: f64 = 200.;
pub const TAXI_QUEUE_STEP_FT: f64 = 50.;
pub const TAXI_QUEUE_HOLD_FT: f64 = 250.;

/// The part of the map an aircraft may start over, feet. X runs east and Z
/// north, as in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapBounds {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl MapBounds {
    /// Terrain runs from 0 to `(cells - 1) * cell_ft` on each axis; the
    /// bounds are that rectangle shrunk by `margin_ft` on every side.
    pub fn from_cells(cols: usize, rows: usize, cell_ft: f64, margin_ft: f64) -> Self {
        let extent = |cells: usize| cells.saturating_sub(1) as f64 * cell_ft;
        Self {
            min: [margin_ft, margin_ft],
            max: [extent(cols) - margin_ft, extent(rows) - margin_ft],
        }
    }

    #[cfg(test)]
    pub fn contains(&self, x: f64, z: f64) -> bool {
        (self.min[0]..=self.max[0]).contains(&x) && (self.min[1]..=self.max[1]).contains(&z)
    }
}

/// Where the enemy group is placed relative to the friendly reference point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnemyAim {
    /// Clockwise rotation from the reference heading, radians.
    pub turn: f64,
    /// Distance actually used, feet.
    pub distance_ft: f64,
    /// Distance the player chose, feet.
    pub requested_ft: f64,
}

impl EnemyAim {
    pub fn straight(distance_ft: f64) -> Self {
        Self {
            turn: 0.,
            distance_ft,
            requested_ft: distance_ft,
        }
    }

    /// True when the chosen distance did not fit and a shorter one is used.
    pub fn shortened(&self) -> bool {
        self.distance_ft + 1. < self.requested_ft
    }
}

/// The farthest distance, up to `limit`, at which every point of the group
/// stays inside `bounds` along `bearing`. None if no distance in `0..=limit`
/// works. `group` holds each aircraft's [right, forward] offset from the
/// group's placement point, in a frame whose forward axis is the bearing.
fn reach(
    reference: [f64; 2],
    bearing: f64,
    group: &[[f64; 2]],
    bounds: MapBounds,
    limit: f64,
) -> Option<f64> {
    let (sin, cos) = bearing.sin_cos();
    let forward = [sin, cos];
    let right = [cos, -sin];
    let (mut low, mut high) = (0f64, limit);
    for [r, f] in group {
        for axis in 0..2 {
            // Position along this axis is `start + slope * distance`.
            let start = reference[axis] + right[axis] * r + forward[axis] * f;
            let slope = forward[axis];
            let (min, max) = (bounds.min[axis], bounds.max[axis]);
            if slope.abs() < 1e-12 {
                if start < min || start > max {
                    return None;
                }
                continue;
            }
            let (a, b) = ((min - start) / slope, (max - start) / slope);
            low = low.max(a.min(b));
            high = high.min(a.max(b));
        }
    }
    (low <= high).then_some(high)
}

/// Aim the enemy group into the map (John's request, 2026-09-23).
///
/// Straight ahead is kept when the whole group fits there at the chosen
/// distance. Otherwise the bearing closest to `heading` that fits is used,
/// searching one degree at a time, clockwise before counter-clockwise at equal
/// angles. If no bearing fits at the full distance, the bearing that allows the
/// longest distance is used at that distance (closest to `heading` on a tie).
/// `fitted`: the search order and the tie rules are agent choices.
pub fn aim_into_map(
    reference: [f64; 2],
    heading: f64,
    distance_ft: f64,
    group: &[[f64; 2]],
    bounds: MapBounds,
) -> EnemyAim {
    if group.is_empty() || !distance_ft.is_finite() || distance_ft <= 0. {
        return EnemyAim::straight(distance_ft);
    }
    let half_turn = 180 / AIM_STEP_DEG;
    let turns = (0..=half_turn).flat_map(|step| {
        let degrees = step * AIM_STEP_DEG;
        let both = step != 0 && step != half_turn;
        std::iter::once(degrees).chain(both.then_some(-degrees))
    });
    let mut best: Option<(f64, f64)> = None;
    for degrees in turns {
        let turn = f64::from(degrees).to_radians();
        let Some(reach) = reach(reference, heading + turn, group, bounds, distance_ft) else {
            continue;
        };
        if reach >= distance_ft - 1e-6 {
            return EnemyAim {
                turn,
                distance_ft,
                requested_ft: distance_ft,
            };
        }
        if best.is_none_or(|(_, farthest)| reach > farthest + 1e-6) {
            best = Some((turn, reach));
        }
    }
    // A reference point outside the usable map has nothing to aim at; keep
    // the chosen geometry rather than invent one.
    best.map_or(EnemyAim::straight(distance_ft), |(turn, reach)| EnemyAim {
        turn,
        distance_ft: reach,
        requested_ft: distance_ft,
    })
}

/// Each parked aircraft's [right, forward] offset from the leader's slot,
/// leader first: wingman 1 right, 2 left, 3 right, 4 left, each one `spacing`
/// farther back than the previous.
pub fn ground_slot_offsets(count: usize, spacing_ft: f64) -> Vec<[f64; 2]> {
    (0..count)
        .map(|order| {
            let side = match order {
                0 => 0.,
                n if n % 2 == 1 => 1.,
                _ => -1.,
            };
            [side * GROUND_LATERAL_FT, -(order as f64) * spacing_ft]
        })
        .collect()
}

/// Parking points on `runway`, leader first, at runway elevation. The last
/// aircraft stands at the existing single-aircraft start point (5% of the
/// runway length in from the near threshold, capped at 100 ft) so the leader
/// is `(count - 1) * spacing` farther down the runway.
pub fn runway_slots(runway: &Runway, count: usize, spacing_ft: f64) -> Vec<[f64; 3]> {
    let (last, heading) = runway.departure_pose();
    let forward = [heading.sin(), heading.cos()];
    let right = [heading.cos(), -heading.sin()];
    let lead = count.saturating_sub(1) as f64 * spacing_ft;
    ground_slot_offsets(count, spacing_ft)
        .into_iter()
        .map(|[r, f]| {
            let along = lead + f;
            [
                last[0] + forward[0] * along + right[0] * r,
                last[1],
                last[2] + forward[1] * along + right[1] * r,
            ]
        })
        .collect()
}

/// Choose the widest spacing at which every slot passes `check`, which
/// returns the slot resting on the surface or the reason it cannot be used.
/// Returns the spacing and the checked slots, or the first reason the
/// tightest spacing still failed.
pub fn fit_runway_slots(
    runway: &Runway,
    count: usize,
    mut check: impl FnMut([f64; 3]) -> Result<[f64; 3], String>,
) -> Result<(f64, Vec<[f64; 3]>), String> {
    let spacings: &[f64] = if count <= 1 {
        &GROUND_SPACING_FT[..1]
    } else {
        &GROUND_SPACING_FT
    };
    let mut reason = String::new();
    for &spacing in spacings {
        match runway_slots(runway, count, spacing)
            .into_iter()
            .map(&mut check)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(slots) => return Ok((spacing, slots)),
            Err(why) => reason = why,
        }
    }
    Err(reason)
}

/// The heading the wing departs on.
pub fn departure_heading(runway: &Runway) -> f64 {
    runway.approach_heading(ApproachEnd::Near)
}

/// Player on the reviewed takeoff spot; wingmen queued on the taxi-out path
/// (John, 2026-09-23). Positions are measured backwards from the last taxi
/// anchor, facing along that segment toward the runway. Blocked slots are
/// skipped and a queue that cannot fit falls back to the runway layout.
pub fn anchored_slots(
    anchors: &AirfieldAnchors,
    count: usize,
    mut check: impl FnMut([f64; 3]) -> Result<[f64; 3], String>,
) -> Option<Vec<([f64; 3], f64)>> {
    let mut slots = vec![(check(anchors.takeoff_spot).ok()?, anchors.takeoff_heading)];
    let segments: Vec<_> = (1..4)
        .rev()
        .map(|leg| {
            let end = anchors.taxi_out[leg];
            let start = anchors.taxi_out[leg - 1];
            let dx = end[0] - start[0];
            let dz = end[2] - start[2];
            (end, dx, dz, dx.hypot(dz))
        })
        .collect();
    let length: f64 = segments.iter().map(|s| s.3).sum();
    let mut offset = 0.;
    while slots.len() < count {
        let mut found = None;
        while offset <= length {
            let mut remaining = offset;
            for &(end, dx, dz, len) in &segments {
                if len > 0. && remaining <= len {
                    let p = [
                        end[0] - dx * remaining / len,
                        end[1],
                        end[2] - dz * remaining / len,
                    ];
                    let from_player =
                        (p[0] - anchors.takeoff_spot[0]).hypot(p[2] - anchors.takeoff_spot[2]);
                    if from_player >= TAXI_QUEUE_HOLD_FT
                        && slots.iter().all(|(other, _)| {
                            (other[0] - p[0]).hypot(other[2] - p[2]) >= TAXI_QUEUE_SPACING_FT
                        })
                        && let Ok(p) = check(p)
                    {
                        found = Some((p, dx.atan2(dz)));
                    }
                    break;
                }
                remaining -= len;
            }
            if found.is_some() {
                break;
            }
            offset += TAXI_QUEUE_STEP_FT;
        }
        slots.push(found?);
        offset += TAXI_QUEUE_SPACING_FT;
    }
    Some(slots)
}

/// Every slot as a [right, forward] offset from the leader's slot, in the
/// leader's frame.
pub fn relative_offsets(slots: &[[f64; 3]], heading: f64) -> Vec<[f64; 2]> {
    let Some(lead) = slots.first() else {
        return Vec::new();
    };
    let (sin, cos) = heading.sin_cos();
    slots
        .iter()
        .map(|p| {
            let d = [p[0] - lead[0], p[2] - lead[2]];
            [d[0] * cos - d[1] * sin, d[0] * sin + d[1] * cos]
        })
        .collect()
}

pub fn runway_pose(world: &Terrain, object: u32) -> crate::WorldResult<([f64; 3], f64)> {
    let runway = world
        .airport_scene
        .runway(object)
        .ok_or("Selected runway is unavailable; choose another airport.")?;
    let (position, heading) = runway.departure_pose();
    if !runway.surface.contains_horizontal(position[0], position[2]) {
        return Err("Selected runway has no supported departure point".into());
    }
    Ok((position, heading))
}

/// `fitted`, agent decision 2026-09-23: buildings are checked at this height
/// above the runway at every parking point, about a fighter's wheel-to-centre
/// clearance, before any aircraft is placed there.
const SLOT_PROBE_FT: f64 = 6.;

/// Where the player's wing parks for a ground start.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundLayout {
    pub object: u32,
    pub airport: u32,
    pub runway: tore_sim::ai::airfield::RunwayView,
    /// Departure heading down the runway, the leader's heading.
    pub heading: f64,
    /// True when the airport's own takeoff spot and taxiway queue are used;
    /// false for the fitted staggered runway fallback.
    pub anchored: bool,
    /// Staggered fallback only: distance between parked aircraft.
    pub spacing_ft: Option<f64>,
    /// Surface points, leader (the player) first.
    pub slots: Vec<[f64; 3]>,
    /// Heading of each slot.
    pub headings: Vec<f64>,
}

impl GroundLayout {
    /// The same slots as [right, forward] offsets from the player's slot.
    pub fn offsets(&self) -> Vec<[f64; 2]> {
        relative_offsets(&self.slots, self.heading)
    }

    /// The departure handed to the AI wingmen.
    pub fn departure(&self) -> crate::ai_wings::Departure {
        crate::ai_wings::Departure {
            runway: self.runway,
            headings: self.headings.clone(),
            slots: self.slots.clone(),
        }
    }
}

/// Place `count` aircraft (the player's whole wing) for a ground start. With
/// the airport's own points the player takes the takeoff spot and wingmen the
/// taxiway queue; otherwise the wing parks staggered on the runway. Every slot
/// must be on the airport's paving, on a landable surface and clear of
/// buildings. If the staggered layout cannot fit, its spacing tightens, and
/// if none works the start is rejected with a message for the creator.
/// The runway a `start ground auto` mission parks Blue's wing on: with a
/// ground target, the Blue-side airfield nearest the target at least 15 nm
/// from it (docs/spec/surface-defenses.md, "Start placement"; default,
/// pending John) that is no short strip or vertical pad and holds a ground
/// layout for `count` aircraft. Without a target, or when no such airfield
/// exists, the earlier rule (`fitted`, agent decision 2026-10-10, slice Q1):
/// the first runway, by object id, of an airport of the side that qualifies,
/// else of a neutral one. Never an enemy field (slice AL1): with none of the
/// side's own or neutral the start is refused, and the creator offers
/// Airborne only.
pub fn auto_runway(world: &Terrain, count: usize) -> crate::WorldResult<u32> {
    auto_runway_for(world, count, false)
}

/// [`auto_runway`] for either side: `red` ranks the Red-side airfields, for
/// a Redfor human's ground start in PvP.
pub fn auto_runway_for(world: &Terrain, count: usize, red: bool) -> crate::WorldResult<u32> {
    use tore_sim::airport::Allegiance;
    let scene = &world.airport_scene;
    // The airport of each runway, as the side sees it.
    let allegiance = |id: u32| {
        scene
            .runway(id)
            .and_then(|r| scene.airports.iter().find(|a| a.id == r.airport))
            .filter(|a| a.serves(red))
            .map(|a| a.allegiance_for(red))
    };
    let qualifies = |id: u32| {
        allegiance(id).is_some()
            && !scene.vertical_pad(id)
            && !scene.short_strip(id)
            && ground_layout(world, id, count).is_ok()
    };
    if let Some(starts) = &world.surface.starts {
        let ranked = if red {
            &starts.red_airfields
        } else {
            &starts.blue_airfields
        };
        if let Some(id) = ranked.iter().copied().find(|id| qualifies(*id)) {
            return Ok(id);
        }
    }
    let usable = |own_only: bool| {
        let mut ids: Vec<u32> = scene
            .runways
            .iter()
            .map(|r| r.object)
            .filter(|id| !own_only || allegiance(*id) == Some(Allegiance::Friendly))
            .collect();
        ids.sort_unstable();
        ids.into_iter().find(|id| qualifies(*id))
    };
    usable(true).or_else(|| usable(false)).ok_or_else(|| {
        "No runway of your side in this theater holds your wing for a ground start. Choose Airborne."
            .into()
    })
}

/// The runway Enemy Wing 1 parks on for an enemy ground start (slice CL-R,
/// the Redfor mirror of the player's start): `None` when it starts in the
/// air. `enemy-start ground auto` ranks the Red-side airfields as
/// [`auto_runway_for`] does for Blue (with a ground target the Red-side field
/// nearest the target at least 15 nm from it; else the first Red-side
/// runway, by object id, that holds the wing). A chosen runway must be a
/// Redfor or neutral field's (slice AL1: a Blue field is the other side's,
/// under its guns) and no vertical pad; a STOL strip is allowed, as it is
/// for the player (John, 2026-10-11). Whether the wing fits is [`ground_layout`]'s check.
pub fn enemy_runway(
    world: &Terrain,
    start: crate::mission::EnemyStart,
    count: usize,
) -> crate::WorldResult<Option<u32>> {
    use crate::mission::EnemyStart;
    let scene = &world.airport_scene;
    match start {
        EnemyStart::Airborne => Ok(None),
        EnemyStart::GroundAuto => auto_runway_for(world, count, true).map(Some).map_err(|_| {
            "No Redfor runway in this theater holds enemy wing 1 for a ground start. Start the enemy Airborne."
                .into()
        }),
        EnemyStart::Ground { runway } => {
            let airport = scene
                .runway(runway)
                .and_then(|r| scene.airports.iter().find(|a| a.id == r.airport))
                .ok_or("The enemy's runway is unavailable; choose another airport.")?;
            let name = &airport.name;
            if !airport.serves(true) {
                return Err(format!(
                    "{name} is a Blue airfield: no enemy ground start there. Choose a Redfor airfield or start the enemy Airborne."
                )
                .into());
            }
            if scene.vertical_pad(runway) {
                return Err(format!(
                    "{name} is a vertical pad: no enemy ground start there. Choose a runway or start the enemy Airborne."
                )
                .into());
            }
            Ok(Some(runway))
        }
    }
}

pub fn ground_layout(
    world: &Terrain,
    object: u32,
    count: usize,
) -> crate::WorldResult<GroundLayout> {
    let object = world.airport_scene.canonical_id(object);
    let runway = world
        .airport_scene
        .runway(object)
        .ok_or("Selected runway is unavailable; choose another airport.")?;
    let view = world
        .runway_view(object)
        .ok_or("Selected runway is unavailable; choose another airport.")?;
    let buildings: Vec<u32> = world.airport_scene.objects.iter().map(|o| o.id).collect();
    let check = |p: [f64; 3]| -> Result<[f64; 3], String> {
        if !runway.surface.contains_horizontal(p[0], p[2]) {
            return Err(if count > 1 {
                "The selected runway is too short for your whole wing.".into()
            } else {
                "Selected runway has no supported departure point".into()
            });
        }
        let surface = world.surface(p[0], p[2]);
        if !surface.landable {
            return Err("Selected runway does not provide a ground surface".into());
        }
        let probe = [p[0], surface.height + SLOT_PROBE_FT, p[2]];
        if world
            .solid_contact(probe, probe, buildings.iter().copied())
            .is_some()
        {
            return Err("The runway start is obstructed. Choose another airport.".into());
        }
        Ok([p[0], surface.height, p[2]])
    };
    let heading = departure_heading(runway);
    let mut check = check;
    if let Some(slots) = view
        .anchors
        .as_ref()
        .and_then(|anchors| anchored_slots(anchors, count, &mut check))
    {
        let (slots, headings) = slots.into_iter().unzip();
        return Ok(GroundLayout {
            object,
            airport: runway.airport,
            runway: view,
            heading,
            anchored: true,
            spacing_ft: None,
            slots,
            headings,
        });
    }
    let (spacing_ft, slots) = fit_runway_slots(runway, count, check).map_err(|why| {
        if count > 1 {
            format!("{why} Your wing of {count} could not be parked; choose another airport or fewer wingmen.")
        } else {
            why
        }
    })?;
    Ok(GroundLayout {
        object,
        airport: runway.airport,
        runway: view,
        heading,
        anchored: false,
        spacing_ft: Some(spacing_ft),
        headings: vec![heading; slots.len()],
        slots,
    })
}

/// Stand an aircraft in its parking slot: stationary, engine idling, gear and
/// flaps down, brakes set. Rejects a slot where the aircraft itself would sit
/// inside a building.
pub fn place_on_runway(
    world: &Terrain,
    flight: &mut tore_sim::flight::State,
    layout: &GroundLayout,
    order: usize,
) -> crate::WorldResult<()> {
    let position = *layout
        .slots
        .get(order)
        .ok_or("The ground start has no slot for this aircraft")?;
    let heading = layout
        .headings
        .get(order)
        .copied()
        .unwrap_or(layout.heading);
    let mut candidate = flight.clone();
    candidate.start_on_runway(position, heading)?;
    if world
        .solid_contact(
            candidate.position,
            candidate.position,
            world.airport_scene.objects.iter().map(|o| o.id),
        )
        .is_some()
    {
        return Err("The runway start is obstructed. Choose another airport.".into());
    }
    *flight = candidate;
    Ok(())
}

/// The player alone on the runway, as the `--ground-start` developer option
/// and the straight-flight fixtures use it.
pub fn apply_ground_start(
    world: &Terrain,
    flight: &mut tore_sim::flight::State,
    object: u32,
) -> crate::WorldResult<u32> {
    let layout = ground_layout(world, object, 1)?;
    place_on_runway(world, flight, &layout, 0)?;
    Ok(layout.airport)
}

/// Everything the creator decided about where aircraft start, kept so a
/// restart rebuilds exactly the same scene.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionLayout {
    /// The player's wing parked on this runway, for a ground start.
    pub ground: Option<GroundLayout>,
    /// Enemy Wing 1 parked on this runway, for an enemy ground start
    /// ([`crate::mission::EnemyStart`]). The rest of the enemy group starts
    /// where [`Self::enemy`] aims it.
    pub enemy_ground: Option<GroundLayout>,
    /// Airborne start only: the turn added to the player's starting heading,
    /// radians clockwise, so that the enemy ahead stays on the map. The whole
    /// airborne scene turns with the player.
    pub player_turn: f64,
    /// Where the enemy group sits relative to the player.
    pub enemy: EnemyAim,
    /// Airborne with a ground target: the player's heading points at the
    /// target and is kept, and the enemy group alone turns toward Red's
    /// start, as on a ground start (agent decision, 2026-10-10).
    pub held_heading: bool,
    /// The creator's situation as it applies here: the mission's for an
    /// airborne start whose enemy the separation places, neutral when the
    /// player starts on the ground or a ground target placed the enemy.
    pub situation: Situation,
}

/// The situation a start honours: an airborne start with no ground target
/// placing the enemy. Elsewhere it means nothing and the start is neutral
/// (agent decision, 2026-10-28: a parked wing cannot be on anyone's tail,
/// and a target's template already decides where the enemy waits).
pub fn effective_situation(situation: Situation, ground_start: bool, placed: bool) -> Situation {
    if ground_start || placed {
        Situation::Neutral
    } else {
        situation
    }
}

impl MissionLayout {
    /// Plan a mission around `start`, the player's pose before any ground
    /// placement (a ground start takes the leader's runway slot instead).
    /// `wings` are the mission's wings; the enemy ones are kept on the map
    /// ([`crate::ai_wings::enemy_group_offsets`]) where `situation` puts
    /// them ([`effective_situation`]).
    pub fn plan(
        world: &Terrain,
        start: &tore_sim::flight::State,
        ground: Option<GroundLayout>,
        wings: &[tore_sim::ai::launch::WingLaunch],
        separation_ft: f64,
        situation: Situation,
    ) -> Self {
        let situation =
            effective_situation(situation, ground.is_some(), world.surface.starts.is_some());
        let (reference, heading) = match &ground {
            Some(g) => ([g.slots[0][0], g.slots[0][2]], g.heading),
            None => ([start.position[0], start.position[2]], start.yaw),
        };
        // With a ground target Red starts where the surface layout put it,
        // by the target (John, 2026-10-10): the group is aimed at that point
        // from wherever the player starts, in the air or on a runway.
        let enemy = match &world.surface.starts {
            Some(starts) => {
                let red = starts.red.map(f64::from);
                let to = [red[0] - reference[0], red[1] - reference[1]];
                let distance_ft = to[0].hypot(to[1]);
                EnemyAim {
                    turn: (to[0].atan2(to[1]) - heading).rem_euclid(std::f64::consts::TAU),
                    distance_ft,
                    requested_ft: distance_ft,
                }
            }
            None => aim_into_map(
                reference,
                heading + situation_bearing(situation),
                separation_ft,
                &crate::ai_wings::enemy_group_offsets(wings, situation),
                map_bounds(world),
            ),
        };
        let held_heading = ground.is_none() && world.surface.starts.is_some();
        Self {
            player_turn: if ground.is_some() || held_heading {
                0.
            } else {
                enemy.turn
            },
            ground,
            enemy_ground: None,
            enemy,
            held_heading,
            situation,
        }
    }

    /// The layout with Enemy Wing 1 parked by `enemy_ground`.
    pub fn with_enemy_ground(mut self, enemy_ground: Option<GroundLayout>) -> Self {
        self.enemy_ground = enemy_ground;
        self
    }

    pub fn spawn_plan(&self) -> crate::ai_wings::SpawnPlan {
        crate::ai_wings::SpawnPlan {
            separation_ft: self.enemy.distance_ft,
            // An airborne scene turns with the player; parked aircraft cannot,
            // so only the enemy bearing changes.
            enemy_turn: if self.ground.is_some() || self.held_heading {
                self.enemy.turn
            } else {
                0.
            },
            runway_slots: self.ground.as_ref().map(GroundLayout::offsets),
            situation: self.situation,
        }
    }

    /// A line for the player when the chosen separation did not fit.
    pub fn notice(&self) -> Option<String> {
        self.enemy.shortened().then(|| {
            format!(
                "Enemy forces start {:.0} miles away: {:.0} miles does not fit this theater.",
                (self.enemy.distance_ft / FEET_PER_NM).floor(),
                self.enemy.requested_ft / FEET_PER_NM
            )
        })
    }
}

/// The bearing of the enemy group's placement point from the player's
/// heading, radians clockwise: ahead, or behind in a disadvantage.
pub fn situation_bearing(situation: Situation) -> f64 {
    if situation == Situation::Disadvantage {
        std::f64::consts::PI
    } else {
        0.
    }
}

/// The usable map for starting aircraft: the terrain less one cell each side.
pub fn map_bounds(world: &Terrain) -> MapBounds {
    let cell = f64::from(tore_formats::theater::CELL_FEET);
    MapBounds::from_cells(
        world.theater.cols,
        world.theater.rows,
        cell,
        MAP_MARGIN_CELLS * cell,
    )
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::terrain::Terrain;
    const RUNWAY: u32 = 0x4000_0001;

    /// The synthetic terrain with one 6000 ft north-facing runway, its near
    /// threshold 1096 ft into the map.
    fn airfield(length_ft: f64) -> Terrain {
        use tore_sim::airport::{Airport, Allegiance, OrientedBox, SourceKey, StaticObject};
        let mut world = crate::test_support::terrain();
        let surface = OrientedBox {
            center: [4096., 20., 4096.],
            half: [1500., 2., length_ft / 2.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
        };
        let scene = &mut world.airport_scene;
        scene.objects.push(StaticObject {
            id: RUNWAY,
            source: SourceKey {
                layout: "T.MM".into(),
                ordinal: 1,
            },
            name: "Strip".into(),
            object_type: "RNWY1.OT".into(),
            bounds: surface,
            hit_points: 100,
            category: 0,
            radar_signature: 1.,
            infrared_signature: 0.,
            type_flags: 0,
            max_visible: 0,
            runway: true,
        });
        scene.runways.push(tore_sim::airport::Runway {
            object: RUNWAY,
            airport: 9,
            name: "Strip".into(),
            surface,
            approach_center: [4096., 20., 4096.],
            elevation_ft: 20.,
            heading: 0.,
            length_ft,
        });
        scene.airports.push(Airport {
            id: 9,
            name: "Strip".into(),
            runway_objects: vec![RUNWAY],
            allegiance: Allegiance::Neutral,
            neutral_permission: true,
        });
        world
    }

    fn hangar(world: &mut Terrain, id: u32, along_ft: f64) {
        let near = 4096. - world.airport_scene.runways[0].length_ft / 2.;
        building(world, id, 4096., near + along_ft);
    }

    fn building(world: &mut Terrain, id: u32, x: f64, z: f64) {
        world
            .airport_scene
            .objects
            .push(tore_sim::airport::StaticObject {
                id,
                source: tore_sim::airport::SourceKey {
                    layout: "T.MM".into(),
                    ordinal: id,
                },
                name: "Hangar".into(),
                object_type: "HANGR.OT".into(),
                bounds: tore_sim::airport::OrientedBox {
                    center: [x, 30., z],
                    half: [100., 30., 40.],
                    heading: 0.,
                    pitch: 0.,
                    bank: 0.,
                },
                hit_points: 100,
                category: 0x2000,
                radar_signature: 1.,
                infrared_signature: 0.,
                type_flags: 0,
                max_visible: 0,
                runway: false,
            });
    }

    #[test]
    fn a_ground_start_parks_the_whole_wing_with_the_player_in_front() {
        let world = airfield(6000.);
        let layout = ground_layout(&world, RUNWAY, 5).unwrap();
        assert_eq!((layout.airport, layout.spacing_ft), (9, Some(250.)));
        assert!(!layout.anchored);
        assert_eq!(layout.headings, [0.; 5]);
        let near = 4096. - 3000.;
        let expected = [
            (0., 1100.),
            (40., 850.),
            (-40., 600.),
            (40., 350.),
            (-40., 100.),
        ];
        for (slot, (right, along)) in layout.slots.iter().zip(expected) {
            assert!((slot[0] - 4096. - right).abs() < 1e-6);
            assert!((slot[2] - near - along).abs() < 1e-6);
            assert_eq!(slot[1], 20.);
        }
        assert_eq!(layout.offsets()[3], [40., -750.]);
        let departure = layout.departure();
        assert_eq!(departure.slots, layout.slots);
        assert_eq!(departure.runway.object, RUNWAY);
        // The player stands in the front slot on the researched model.
        let mut player =
            tore_sim::flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        assert!(place_on_runway(&world, &mut player, &layout, 0).is_err());
        player.enable_research(1).unwrap();
        place_on_runway(&world, &mut player, &layout, 0).unwrap();
        assert_eq!(
            [player.position[0], player.position[2]],
            [4096., near + 1100.]
        );
        assert!(player.brake_out && player.gear_down && player.speed == 0.);
        // Alone, the player keeps the original single-aircraft start point.
        let alone = ground_layout(&world, RUNWAY, 1).unwrap();
        assert_eq!(alone.slots[0][2], near + 100.);
    }

    #[test]
    fn airport_points_put_the_player_on_the_runway_and_wingmen_on_the_taxiway() {
        use std::f64::consts::FRAC_PI_2;
        let mut world = airfield(6000.);
        let at = |x: f64, z: f64| [x, 20., z];
        world.airfield_anchors.insert(
            RUNWAY,
            tore_sim::ai::airfield::AirfieldAnchors {
                taxi_out: [
                    at(5000., 1300.),
                    at(4600., 1300.),
                    at(4300., 1300.),
                    at(4096., 1300.),
                ],
                takeoff_spot: at(4096., 1500.),
                takeoff_heading: 0.,
                landing_point: at(4096., 1340.),
                landing_heading: 0.,
                taxi_in: [
                    at(4096., 6000.),
                    at(4400., 6000.),
                    at(4400., 1400.),
                    at(5000., 1400.),
                ],
                parking: std::array::from_fn(|k| at(5000., 1500. + 200. * k as f64)),
                parking_heading: FRAC_PI_2,
            },
        );
        let layout = ground_layout(&world, RUNWAY, 3).unwrap();
        assert!(layout.anchored && layout.spacing_ft.is_none());
        assert_eq!(
            layout.slots,
            [at(4096., 1500.), at(4246., 1300.), at(4446., 1300.)]
        );
        assert_eq!(layout.headings, [0., -FRAC_PI_2, -FRAC_PI_2]);
        assert!(layout.runway.anchors.is_some());
        assert_eq!(layout.departure().headings, layout.headings);
        assert_eq!(layout.offsets()[1], [150., -200.]);
        let mut wingman =
            tore_sim::flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        wingman.enable_research(2).unwrap();
        place_on_runway(&world, &mut wingman, &layout, 1).unwrap();
        assert_eq!(
            wingman.yaw.rem_euclid(std::f64::consts::TAU),
            3. * FRAC_PI_2
        );
        // An obstructed queue slot moves the queue farther along the taxiway.
        building(&mut world, 300, 4246., 1300.);
        let layout = ground_layout(&world, RUNWAY, 3).unwrap();
        assert_ne!(layout.slots[1], at(4246., 1300.));
        // A building on the takeoff spot falls back to the staggered runway
        // layout, which starts clear of it.
        building(&mut world, 301, 4096., 1500.);
        let layout = ground_layout(&world, RUNWAY, 3).unwrap();
        assert!(!layout.anchored);
        assert_eq!(layout.spacing_ft, Some(250.));
    }

    #[test]
    fn blocked_or_short_runways_tighten_the_wing_then_refuse_it() {
        // A hangar over the 250 ft leader slot moves everyone to 200 ft.
        let mut world = airfield(6000.);
        hangar(&mut world, 200, 1100.);
        let layout = ground_layout(&world, RUNWAY, 5).unwrap();
        assert_eq!(layout.spacing_ft, Some(200.));
        assert!((layout.slots[0][2] - (4096. - 3000.) - 900.).abs() < 1e-6);
        // A hangar on the last aircraft's slot leaves nothing to fall back to.
        hangar(&mut world, 201, 100.);
        let error = ground_layout(&world, RUNWAY, 5).unwrap_err().to_string();
        assert!(
            error.contains("obstructed") && error.contains("wing of 5"),
            "{error}"
        );
        // A 600 ft strip only fits five aircraft at 100 ft spacing.
        let short = airfield(600.);
        assert_eq!(
            ground_layout(&short, RUNWAY, 5).unwrap().spacing_ft,
            Some(100.)
        );
        let error = ground_layout(&airfield(300.), RUNWAY, 5)
            .unwrap_err()
            .to_string();
        assert!(error.contains("too short"), "{error}");
        assert!(ground_layout(&airfield(300.), RUNWAY, 1).is_ok());
    }

    #[test]
    fn an_enemy_ground_start_takes_a_redfor_or_neutral_field_and_never_a_blue_one() {
        use crate::mission::EnemyStart;
        use tore_sim::airport::Allegiance;
        let mut world = airfield(6000.);
        let ground = EnemyStart::Ground { runway: RUNWAY };
        assert_eq!(enemy_runway(&world, EnemyStart::Airborne, 2).unwrap(), None);
        // Neutral serves both sides.
        assert_eq!(enemy_runway(&world, ground, 2).unwrap(), Some(RUNWAY));
        world.airport_scene.airports[0].allegiance = Allegiance::Hostile;
        assert_eq!(enemy_runway(&world, ground, 2).unwrap(), Some(RUNWAY));
        assert_eq!(
            enemy_runway(&world, EnemyStart::GroundAuto, 2).unwrap(),
            Some(RUNWAY)
        );
        world.airport_scene.airports[0].allegiance = Allegiance::Friendly;
        let error = enemy_runway(&world, ground, 2).unwrap_err().to_string();
        assert!(error.contains("Strip is a Blue airfield"), "{error}");
        let error = enemy_runway(&world, EnemyStart::GroundAuto, 2)
            .unwrap_err()
            .to_string();
        assert!(error.contains("No Redfor runway"), "{error}");
        let error = enemy_runway(&world, EnemyStart::Ground { runway: RUNWAY + 9 }, 2)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unavailable"), "{error}");
    }

    #[test]
    fn the_layout_turns_the_scene_in_the_air_and_only_the_enemy_on_the_ground() {
        let world = airfield(6000.);
        let ground = ground_layout(&world, RUNWAY, 3).unwrap();
        let aim = EnemyAim {
            turn: 0.5,
            distance_ft: 100.5 * FEET_PER_NM,
            requested_ft: 300. * FEET_PER_NM,
        };
        let airborne = MissionLayout {
            ground: None,
            enemy_ground: None,
            player_turn: aim.turn,
            enemy: aim,
            held_heading: false,
            situation: Situation::Neutral,
        };
        let plan = airborne.spawn_plan();
        assert_eq!(plan.enemy_turn, 0.);
        assert_eq!(plan.separation_ft, 100.5 * FEET_PER_NM);
        assert!(plan.runway_slots.is_none());
        assert_eq!(
            airborne.notice().unwrap(),
            "Enemy forces start 100 miles away: 300 miles does not fit this theater."
        );
        let parked = MissionLayout {
            ground: Some(ground.clone()),
            enemy_ground: None,
            player_turn: 0.,
            enemy: EnemyAim::straight(5. * FEET_PER_NM),
            held_heading: false,
            situation: Situation::Neutral,
        };
        assert!(parked.notice().is_none());
        let parked = MissionLayout {
            enemy: aim,
            ..parked
        };
        let plan = parked.spawn_plan();
        assert_eq!(plan.enemy_turn, 0.5);
        assert_eq!(plan.runway_slots.unwrap(), ground.offsets());
        // Planning from the runway uses the leader's slot and runway heading.
        let start = tore_sim::flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        let planned =
            MissionLayout::plan(&world, &start, Some(ground), &[], 1000., Situation::Neutral);
        assert_eq!(planned.player_turn, 0.);
        assert_eq!(planned.enemy, EnemyAim::straight(1000.));
    }

    /// Each enemy aircraft's start for `situation`, seen from a player
    /// starting near the north edge of a 60 by 60 cell map facing north:
    /// the player's turned heading, and every enemy's position and heading.
    fn situation_starts(situation: Situation) -> (f64, Vec<([f64; 3], f64)>, MapBounds) {
        use crate::ai_wings::mission_spawns;
        use tore_sim::attitude::Basis;
        let mut world = crate::test_support::terrain();
        world.theater.cols = 60;
        world.theater.rows = 60;
        let bounds = map_bounds(&world);
        let mut start =
            tore_sim::flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        start.position = [30. * 8192., 9000., bounds.max[1] - 5. * FEET_PER_NM];
        start.yaw = 0.;
        let mut spec =
            crate::mission::MissionSpec::new("UKR", tore_formats::aircraft::AircraftId::F18);
        spec.wings[3].count = 3;
        spec.wings[4].count = 2;
        spec.wings[5].count = 5;
        let wings = spec.wing_launches().unwrap();
        let layout =
            MissionLayout::plan(&world, &start, None, &wings, 20. * FEET_PER_NM, situation);
        assert_eq!(layout.situation, situation);
        assert!(!layout.enemy.shortened());
        let heading = start.yaw + layout.player_turn;
        let basis = Basis::new(heading, 0., 0.);
        let enemies = mission_spawns(&wings, &layout.spawn_plan())
            .into_iter()
            .filter(|spawn| spawn.opposing)
            .map(|spawn| {
                let (position, basis) = spawn.pose(start.position, basis);
                (position, basis.forward[0].atan2(basis.forward[2]))
            })
            .collect();
        (heading, enemies, bounds)
    }

    /// Degrees of `angle` folded into -180 to 180.
    fn degrees(angle: f64) -> f64 {
        (angle.to_degrees() + 540.).rem_euclid(360.) - 180.
    }

    #[test]
    fn the_situation_puts_the_enemy_ahead_head_on_ahead_going_away_or_behind() {
        // (situation, the enemy lead's bearing from the player's nose, the
        // enemy's heading relative to the player's).
        for (situation, bearing, heading) in [
            (Situation::Neutral, 0., 180.),
            (Situation::Advantage, 0., 0.),
            (Situation::Disadvantage, 180., 0.),
        ] {
            let (player, enemies, bounds) = situation_starts(situation);
            let start = [30. * 8192., bounds.max[1] - 5. * FEET_PER_NM];
            let (lead, _) = enemies[0];
            let to = [lead[0] - start[0], lead[2] - start[1]];
            assert!((to[0].hypot(to[1]) - 20. * FEET_PER_NM).abs() < 1e-6);
            assert!(
                degrees(to[0].atan2(to[1]) - player - f64::to_radians(bearing)).abs() < 1e-6,
                "{situation:?}: the lead bears {} from the nose",
                degrees(to[0].atan2(to[1]) - player)
            );
            for (position, own) in &enemies {
                assert!(
                    degrees(own - player - f64::to_radians(heading)).abs() < 1e-6,
                    "{situation:?}: heading {}",
                    degrees(own - player)
                );
                assert!(
                    bounds.contains(position[0], position[2]),
                    "{situation:?} off the map"
                );
            }
            // Five miles from the north edge, 20 miles ahead or behind does
            // not fit facing north: the whole scene turned.
            if situation != Situation::Disadvantage {
                assert!(
                    degrees(player).abs() > 30.,
                    "{situation:?} kept heading north"
                );
            }
        }
    }

    #[test]
    fn a_ground_start_or_a_placed_enemy_is_neutral_whatever_the_situation() {
        for situation in Situation::ALL {
            assert_eq!(
                effective_situation(situation, true, false),
                Situation::Neutral
            );
            assert_eq!(
                effective_situation(situation, false, true),
                Situation::Neutral
            );
            assert_eq!(effective_situation(situation, false, false), situation);
        }
        let world = airfield(6000.);
        let ground = ground_layout(&world, RUNWAY, 1).unwrap();
        let start = tore_sim::flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        let planned = MissionLayout::plan(
            &world,
            &start,
            Some(ground),
            &[],
            1000.,
            Situation::Disadvantage,
        );
        assert_eq!(planned.spawn_plan().situation, Situation::Neutral);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tore_sim::airport::OrientedBox;

    const CELL: f64 = 8192.;

    fn bounds() -> MapBounds {
        // A 208 by 200 cell theater with the one-cell margin.
        MapBounds::from_cells(208, 200, CELL, CELL)
    }

    fn runway(heading: f64, length_ft: f64) -> Runway {
        let center = [100_000., 50., 200_000.];
        Runway {
            object: 7,
            airport: 3,
            name: "Test".into(),
            surface: OrientedBox {
                center,
                half: [75., 5., length_ft / 2.],
                heading,
                pitch: 0.,
                bank: 0.,
            },
            approach_center: center,
            elevation_ft: 50.,
            heading,
            length_ft,
        }
    }

    #[test]
    fn separation_table_is_nautical_and_includes_host_choices() {
        assert_eq!(
            SEPARATION_NM,
            [1., 2., 5., 10., 20., 50., 75., 100., 150., 200., 300.]
        );
        assert_eq!(FEET_PER_NM, 6_076.12);
        assert_eq!(RETAIL_SEPARATIONS, 6);
    }

    #[test]
    fn bounds_follow_the_terrain_extent_and_margin() {
        let b = bounds();
        assert_eq!(b.min, [CELL, CELL]);
        assert_eq!(b.max, [206. * CELL, 198. * CELL]);
        assert!(b.contains(CELL, 198. * CELL));
        assert!(!b.contains(CELL - 1., CELL));
    }

    #[test]
    fn a_group_that_fits_keeps_the_forward_bearing() {
        let center = [104. * CELL, 100. * CELL];
        let group = [[0., 0.], [-4096., 0.], [4096., 0.], [512., 512.]];
        let aim = aim_into_map(center, 0.3, 50. * FEET_PER_NM, &group, bounds());
        assert_eq!(aim, EnemyAim::straight(50. * FEET_PER_NM));
        assert!(!aim.shortened());
    }

    #[test]
    fn a_group_off_the_map_turns_to_the_nearest_bearing_that_fits() {
        // 20 nm from the north edge, facing north, with 50 nm chosen.
        let b = bounds();
        let reference = [104. * CELL, b.max[1] - 20. * FEET_PER_NM];
        let group = [[0., 0.], [-4096., 0.], [4096., 0.]];
        let distance = 50. * FEET_PER_NM;
        let aim = aim_into_map(reference, 0., distance, &group, b);
        assert_eq!(aim.distance_ft, distance);
        assert!(!aim.shortened());
        let turn = aim.turn.to_degrees();
        // The clockwise side is preferred at equal angles.
        assert!(turn > 60. && turn < 90., "turned {turn} degrees");
        // Every aircraft is on the map at the full distance...
        let bearing = aim.turn;
        let (sin, cos) = bearing.sin_cos();
        for [r, f] in group {
            let x = reference[0] + cos * r + sin * (distance + f);
            let z = reference[1] - sin * r + cos * (distance + f);
            assert!(b.contains(x, z), "{x} {z}");
        }
        // ...and one degree less would put some of it off the map.
        assert!(
            reach(reference, bearing - 1f64.to_radians(), &group, b, distance).unwrap() < distance
        );
        // Mirror image: facing south next to the south edge, the same turn
        // but counter-clockwise from south would be equally close; the
        // clockwise one is chosen.
        let mirrored = [104. * CELL, b.min[1] + 20. * FEET_PER_NM];
        let aim = aim_into_map(mirrored, std::f64::consts::PI, distance, &group, b);
        assert!(aim.turn > 0.);
    }

    #[test]
    fn a_distance_longer_than_the_map_is_shortened_along_the_longest_line() {
        let b = bounds();
        let center = [104. * CELL, 100. * CELL];
        let group = [[0., 0.]];
        let aim = aim_into_map(center, 0., 300. * FEET_PER_NM, &group, b);
        assert!(aim.shortened());
        assert_eq!(aim.requested_ft, 300. * FEET_PER_NM);
        // The centre sits nearer the north-east corner, so the longest line
        // runs south-west, toward bearing 226 degrees (a turn of -134).
        let corner = (center[0] - b.min[0]).hypot(center[1] - b.min[1]);
        assert!(aim.distance_ft <= corner && aim.distance_ft > corner * 0.995);
        assert_eq!(aim.turn.to_degrees().round(), -134.);
        // No bearing, however long the search, reaches the chosen distance.
        for step in -180..=180 {
            let r = reach(
                center,
                f64::from(step).to_radians(),
                &group,
                b,
                300. * FEET_PER_NM,
            );
            assert!(r.unwrap() <= aim.distance_ft + 1e-6);
        }
    }

    #[test]
    fn a_reference_outside_the_usable_map_keeps_the_chosen_geometry() {
        let far = [-10. * CELL, -10. * CELL];
        let aim = aim_into_map(far, 0., 5. * FEET_PER_NM, &[[0., 0.]], bounds());
        assert_eq!(aim, EnemyAim::straight(5. * FEET_PER_NM));
        assert_eq!(
            aim_into_map(far, 0., 5., &[], bounds()),
            EnemyAim::straight(5.)
        );
    }

    #[test]
    fn ground_slots_stagger_behind_the_leader_and_keep_the_threshold_inset() {
        assert_eq!(
            ground_slot_offsets(5, 250.),
            [
                [0., 0.],
                [40., -250.],
                [-40., -500.],
                [40., -750.],
                [-40., -1000.]
            ]
        );
        for heading in [0., 1.1, 4.0] {
            let r = runway(heading, 8000.);
            let slots = runway_slots(&r, 5, 250.);
            let near = r.threshold(ApproachEnd::Near);
            let forward = [heading.sin(), heading.cos()];
            let right = [heading.cos(), -heading.sin()];
            let local = |p: [f64; 3]| {
                let d = [p[0] - near[0], p[2] - near[2]];
                (
                    d[0] * right[0] + d[1] * right[1],
                    d[0] * forward[0] + d[1] * forward[1],
                )
            };
            // The last aircraft stands at the existing 100 ft inset; the
            // leader, the player, is 1000 ft farther down the runway.
            let expected = [
                (0., 1100.),
                (40., 850.),
                (-40., 600.),
                (40., 350.),
                (-40., 100.),
            ];
            for (slot, (want_right, want_along)) in slots.iter().zip(expected) {
                let (got_right, got_along) = local(*slot);
                assert!((got_right - want_right).abs() < 1e-6);
                assert!((got_along - want_along).abs() < 1e-6);
                assert_eq!(slot[1], 50.);
            }
        }
        // A single aircraft starts exactly where the player always has.
        let r = runway(0.5, 8000.);
        assert_eq!(runway_slots(&r, 1, 250.)[0], r.departure_pose().0);
        // Short runways use the 5% inset.
        let short = runway(0., 1000.);
        let slots = runway_slots(&short, 2, 250.);
        let near = short.threshold(ApproachEnd::Near);
        assert!((slots[1][2] - near[2] - 50.).abs() < 1e-9);
    }

    fn anchors() -> AirfieldAnchors {
        let at = |x: f64, z: f64| [x, 50., z];
        AirfieldAnchors {
            taxi_out: [at(900., 0.), at(900., 100.), at(0., 100.), at(0., -200.)],
            takeoff_spot: at(0., 0.),
            takeoff_heading: 0.,
            landing_point: at(0., -160.),
            landing_heading: 0.,
            taxi_in: [
                at(0., 3000.),
                at(300., 3000.),
                at(300., 100.),
                at(900., 100.),
            ],
            parking: std::array::from_fn(|k| at(1000., 200. * k as f64)),
            parking_heading: std::f64::consts::FRAC_PI_2,
        }
    }

    #[test]
    fn anchored_queue_faces_the_runway_and_skips_obstacles() {
        let a = anchors();
        let open = |p: [f64; 3]| Ok([p[0], 49., p[2]]);
        let slots = anchored_slots(&a, 5, open).unwrap();
        assert_eq!(slots[0], ([0., 49., 0.], 0.));
        assert_eq!(slots[1].0, [250., 49., 100.]);
        for pair in slots[1..].windows(2) {
            assert!(
                (pair[0].0[0] - pair[1].0[0]).hypot(pair[0].0[2] - pair[1].0[2])
                    >= TAXI_QUEUE_SPACING_FT
            );
        }
        assert_eq!(slots[1].1, -std::f64::consts::FRAC_PI_2);
        let blocked = |p: [f64; 3]| {
            if (200. ..=300.).contains(&p[0]) {
                Err("blocked".into())
            } else {
                Ok(p)
            }
        };
        assert_eq!(
            anchored_slots(&a, 3, blocked).unwrap()[1].0,
            [350., 50., 100.]
        );
        assert!(anchored_slots(&a, 2, |_| Err("blocked".into())).is_none());
        assert!(anchored_slots(&a, 20, open).is_none());
        assert_eq!(
            relative_offsets(&slots.iter().map(|s| s.0).collect::<Vec<_>>(), 0.)[1],
            [250., 100.]
        );
    }

    #[test]
    fn blocked_slots_tighten_the_spacing_then_reject_the_start() {
        let r = runway(0., 8000.);
        let near = r.threshold(ApproachEnd::Near)[2];
        // An obstruction 1000 to 1200 ft past the threshold blocks the
        // 250 ft layout's leader (1100 ft) but not the 200 ft layout (900 ft).
        let blocked = |p: [f64; 3]| {
            if (1000.0..=1200.).contains(&(p[2] - near)) {
                Err("The runway start is obstructed.".to_string())
            } else {
                Ok([p[0], 49., p[2]])
            }
        };
        let (spacing, slots) = fit_runway_slots(&r, 5, blocked).unwrap();
        assert_eq!(spacing, 200.);
        assert_eq!(slots.len(), 5);
        assert!(slots.iter().all(|s| s[1] == 49.));
        assert!((slots[0][2] - near - 900.).abs() < 1e-6);
        // Blocking the inset point itself leaves no spacing to fall back to.
        let always = |p: [f64; 3]| {
            if p[2] - near < 150. {
                Err("The runway start is obstructed.".to_string())
            } else {
                Ok(p)
            }
        };
        assert_eq!(
            fit_runway_slots(&r, 5, always).unwrap_err(),
            "The runway start is obstructed."
        );
        // One aircraft is never retried at another spacing.
        let mut calls = 0;
        let _ = fit_runway_slots(&r, 1, |_| {
            calls += 1;
            Err("no".to_string())
        });
        assert_eq!(calls, 1);
    }
}
