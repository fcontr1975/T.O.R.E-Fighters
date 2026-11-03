//! How far a surface missile may be launched at a helicopter, for the
//! helicopter's threat rings (docs/spec/ai-strike.md, "Helicopter threat
//! awareness"; T.O.R.E opinionated change requested by John, 2026-10-29).
//!
//! Under the energy rules a site's launch gate ([`super::fire`]'s energy
//! gate) has no fixed range: it launches whenever the energy model says the
//! round reaches the aircraft, flown to a point pushed out by the site's
//! skill share. Against a slow, low helicopter that is beyond the record's
//! zone-1 range (an SA-14 launched at 16,600 ft with a 13,500 ft zone). This
//! module bounds that gate from above by asking it the same question
//! ([`fire::gate_question`], flown by [`live::State::energy_trial`]): the
//! helicopter closing head on at [`CLOSING_FPS`], at each of [`HEIGHTS_FT`]
//! above the launcher, the wind blowing from the site toward it, no ground
//! in the way, the launcher at the top of its altitude band. The farthest
//! horizontal distance that still hits is the reach: a known site cannot
//! launch at the helicopter from farther out (the ring adds its margin on
//! top).
//!
//! The answer is a fixed function of the record, the skill share, the
//! mount's launch pitches, the altitude band and the wind, so it is cached
//! per those (fitted, agent).
use super::fire::{self, MountArc, Place, WeaponArms};
use std::collections::BTreeMap;
use tore_sim::attitude::{Basis, Vector};
use tore_sim::combat::live;

/// The fastest a helicopter is taken to close on a site, ft/s: 160 kt,
/// above the AI helicopters' maximum (about 150 kt on the Mi-24; fitted,
/// agent).
pub const CLOSING_FPS: f64 = 160. * tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT;
/// The helicopter's heights above the launcher tried, feet (fitted, agent:
/// level with a site on a hill, its masked run and pop-up over a site
/// below it).
pub const HEIGHTS_FT: [f64; 3] = [0., 1_000., 2_500.];
/// The launcher's altitude is taken at the top of its band of this many
/// feet (thinner air carries the round farther).
pub const ALTITUDE_BAND_FT: f64 = 250.;
/// The wind is taken at the next multiple of this, ft/s, above its speed.
pub const WIND_STEP_FPS: f64 = 5.;
/// The search: steps out from close in, then halves the last step down to
/// [`RESOLUTION_FT`], keeping the far side.
const STEP_FT: f64 = 1_000.;
const RESOLUTION_FT: f64 = 100.;
/// Misses in a row past the farthest hit that end the search.
const MISSES_TO_STOP: u32 = 2;
/// With no hit at all by this many times the record's zone-1 range (and
/// at least [`NO_HIT_FLOOR_FT`]) the search gives up: the record's range
/// stands.
const NO_HIT_SHARE: f64 = 1.5;
const NO_HIT_FLOOR_FT: f64 = 10_000.;
/// The search never looks farther out than this.
const LIMIT_FT: f64 = 300_000.;
/// The ground under the trial, far below anything it flies.
const NO_GROUND_FT: f64 = -1.0e6;

/// What the reach depends on besides the record's numbers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    record: String,
    /// The skill share, thousandths.
    share: i64,
    /// The mount's lowest and highest launch pitch, tenths of a degree.
    pitch: [i64; 2],
    /// The altitude band's index.
    band: i64,
    /// The wind, multiples of [`WIND_STEP_FPS`].
    wind: i64,
}

/// The cache of reaches. One per mission.
#[derive(Clone, Debug, Default)]
pub struct RotorReach {
    reach: BTreeMap<Key, Option<f64>>,
}

impl RotorReach {
    /// The farthest horizontal distance, feet, from which `weapon` on a
    /// unit standing at `altitude_ft` may be launched at a helicopter under
    /// the energy rules with `wind` (world feet per second): the farthest
    /// the energy gate passes over its mounts and [`HEIGHTS_FT`]. `None`
    /// when no question hits (a store with no motor, which the energy rules
    /// never launch, or one that reaches nothing): the caller keeps the
    /// record's range.
    pub fn reach_ft(&mut self, weapon: &WeaponArms, altitude_ft: f64, wind: Vector) -> Option<f64> {
        let band = (altitude_ft.max(0.) / ALTITUDE_BAND_FT).floor() as i64;
        let wind = (wind[0].hypot(wind[2]) / WIND_STEP_FPS).ceil() as i64;
        let share = (fire::energy_share(weapon) * 1000.).round() as i64;
        let mut arcs: Vec<([i64; 2], &MountArc)> = weapon
            .mounts
            .iter()
            .map(|arc| (arc.launch_pitches().map(|p| (p * 10.).round() as i64), arc))
            .collect();
        arcs.sort_by_key(|(pitch, _)| *pitch);
        arcs.dedup_by_key(|(pitch, _)| *pitch);
        let mut best: Option<f64> = None;
        for (pitch, arc) in arcs {
            let key = Key {
                record: weapon.record.source.clone(),
                share,
                pitch,
                band,
                wind,
            };
            let reach = *self
                .reach
                .entry(key)
                .or_insert_with(|| gate_reach(weapon, arc, band, wind));
            if let Some(reach) = reach {
                best = Some(best.map_or(reach, |b: f64| b.max(reach)));
            }
        }
        best
    }
}

/// Whether the energy gate passes for a helicopter `distance` feet out and
/// `height` feet above `weapon` on `arc`, closing at [`CLOSING_FPS`], with
/// the launcher's ground at `ground_ft` and a `wind_fps` blowing along the
/// line toward it (negative: from it), over no terrain.
fn gate_hits(
    weapon: &WeaponArms,
    arc: &MountArc,
    ground_ft: f64,
    wind_fps: f64,
    distance: f64,
    height: f64,
) -> bool {
    gate_hits_moving(
        weapon,
        arc,
        ground_ft,
        [0., 0., wind_fps],
        [0., height, distance],
        [0., 0., -CLOSING_FPS],
    )
}

/// Whether the energy gate passes for an aircraft at `at` (relative to the
/// unit's ground point, feet) moving at `velocity`, with the unit's ground
/// at `ground_ft`, a `wind` and no terrain: the question the gate asks,
/// flown at once.
fn gate_hits_moving(
    weapon: &WeaponArms,
    arc: &MountArc,
    ground_ft: f64,
    wind: Vector,
    at: Vector,
    velocity: Vector,
) -> bool {
    let origin = [0., ground_ft, 0.];
    let place = Place {
        origin,
        eye: origin,
        basis: Basis::new(0., 0., 0.),
        heading: 0.,
        velocity: [0.; 3],
    };
    let flat = |_: f64, _: f64| ground_ft;
    let mount = place.mount(arc, &flat);
    let position = [at[0], mount[1] + at[1], at[2]];
    let (launcher, observed) =
        fire::gate_question(weapon, &place, arc, 0, position, velocity, &flat);
    live::State::energy_trial(&weapon.record, launcher, observed, wind, &|_, _| {
        NO_GROUND_FT
    })
    .is_some_and(|answer| answer.hit)
}

/// The farthest distance the gate passes for one mount, over
/// [`HEIGHTS_FT`], with the wind along the line both ways.
fn gate_reach(weapon: &WeaponArms, arc: &MountArc, band: i64, wind: i64) -> Option<f64> {
    let ground = (band + 1) as f64 * ALTITUDE_BAND_FT;
    let wind = wind as f64 * WIND_STEP_FPS;
    let winds: &[f64] = if wind > 0. { &[wind, -wind] } else { &[0.] };
    let no_hit = (NO_HIT_SHARE * f64::from(weapon.record.seeker.zones[1].maximum_range.max(0)))
        .max(NO_HIT_FLOOR_FT);
    winds
        .iter()
        .flat_map(|wind| HEIGHTS_FT.iter().map(move |height| (*wind, *height)))
        .filter_map(|(wind, height)| {
            farthest(
                |distance| gate_hits(weapon, arc, ground, wind, distance, height),
                no_hit,
            )
        })
        .reduce(f64::max)
}

/// The farthest distance `hits` holds for: stepping out by [`STEP_FT`] until
/// [`MISSES_TO_STOP`] misses in a row past a hit, then halving the step past
/// the farthest hit down to [`RESOLUTION_FT`]; the far side of the last
/// interval (never short). `None` with no hit by `no_hit`.
fn farthest(hits: impl Fn(f64) -> bool, no_hit: f64) -> Option<f64> {
    let mut last = None;
    let mut misses = 0;
    let mut distance = STEP_FT;
    while distance <= LIMIT_FT {
        if hits(distance) {
            last = Some(distance);
            misses = 0;
        } else if last.is_some() {
            misses += 1;
            if misses >= MISSES_TO_STOP {
                break;
            }
        } else if distance >= no_hit {
            return None;
        }
        distance += STEP_FT;
    }
    let (mut hit, mut miss) = (last?, last? + STEP_FT);
    while miss - hit > RESOLUTION_FT {
        let middle = 0.5 * (hit + miss);
        if hits(middle) {
            hit = middle;
        } else {
            miss = middle;
        }
    }
    Some(miss)
}

#[cfg(test)]
#[path = "rotor_reach_tests.rs"]
mod tests;
