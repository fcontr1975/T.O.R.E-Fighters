//! A guided store's shots at a ground target, as the host's exact-point
//! answer gives them (docs/spec/ai-strike.md, "Stand-off reach"; T.O.R.E
//! opinionated change, John 2026-10-29: "If an attack aircraft can hit
//! outside of range safely of known threats AI should do that").
//!
//! A striker that plans a release from farther out than its usual range
//! asks the host, range by range, whether a shot from there would hit
//! ([`ShotTable::shot`]). The host answers with the same question the AI's
//! release gate asks (under the energy rules the energy model's exact-point
//! answer; under the other rules the store's zone or its glide reach), for a
//! static target on flat ground from a level launch, and caches each answer
//! by store, launch band and range step. The planner takes a range only when
//! the shots one step nearer and one step farther hit too
//! ([`reliable`]): a band where the round strikes the ground (the AT-2's
//! mid-range band from a low hover, docs/baselines/missile-energy.md) or the
//! last step of its reach is never planned.
use crate::ai::weapon_service::StationId;
use std::sync::Arc;

/// The planner's ranges lie on this grid, feet of ground range (fitted,
/// agent): the host caches one answer per step.
pub const SHOT_STEP_FT: f64 = 500.;

/// The launch a shot is asked for: level, toward the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotLaunch {
    /// The target's height, feet above sea level.
    pub target_ft: f64,
    /// The launch's height over the target, feet.
    pub height_ft: f64,
    /// The launcher's speed, ft/s.
    pub speed_fps: f64,
}

/// The host's answers (see the module notes).
pub trait ShotTable: Send + Sync {
    /// A shot of `station`'s store from `launch` at a static target
    /// `range_ft` of ground range away: its time of flight, seconds, when it
    /// hits; `None` when it does not (it strikes the ground, falls short, or
    /// the range lies outside the store's zone), or the host does not know
    /// the store.
    fn shot(&self, station: StationId, launch: ShotLaunch, range_ft: f64) -> Option<f64>;
}

/// The host's [`ShotTable`] for this step, if it gave one. Per-step input,
/// never held: two are equal when they are the same table.
#[derive(Clone, Default)]
pub struct Shots(pub Option<Arc<dyn ShotTable>>);

impl Shots {
    /// See [`ShotTable::shot`]; `None` with no table.
    pub fn shot(&self, station: StationId, launch: ShotLaunch, range_ft: f64) -> Option<f64> {
        self.0
            .as_ref()
            .and_then(|table| table.shot(station, launch, range_ft))
    }
    /// Whether the host gave a table.
    pub fn known(&self) -> bool {
        self.0.is_some()
    }
}

impl std::fmt::Debug for Shots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Shots(table)"
        } else {
            "Shots(none)"
        })
    }
}

impl PartialEq for Shots {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// `range_ft` on the planner's grid (the nearest step, at least one).
pub fn on_grid(range_ft: f64) -> f64 {
    (range_ft / SHOT_STEP_FT).round().max(1.) * SHOT_STEP_FT
}

/// A shot from `range_ft` (on the grid) hits reliably: it hits, and so do
/// the shots one step nearer and one step farther. Its time of flight when
/// it does. The farther shot is asked last, so a range whose own shot
/// misses costs one answer.
pub fn reliable(
    shots: &Shots,
    station: StationId,
    launch: ShotLaunch,
    range_ft: f64,
) -> Option<f64> {
    let range = on_grid(range_ft);
    let seconds = shots.shot(station, launch, range)?;
    shots.shot(station, launch, range - SHOT_STEP_FT)?;
    shots.shot(station, launch, range + SHOT_STEP_FT)?;
    Some(seconds)
}

/// The grid ranges past `beyond_ft`, nearest first, out to `cap_ft` less one
/// step (the farthest a reliable shot can be).
pub fn ranges_beyond(beyond_ft: f64, cap_ft: f64) -> impl Iterator<Item = f64> {
    let first = (beyond_ft / SHOT_STEP_FT).floor() as i64 + 1;
    let last = ((cap_ft - SHOT_STEP_FT) / SHOT_STEP_FT).floor() as i64;
    (first.max(1)..=last).map(|k| k as f64 * SHOT_STEP_FT)
}

/// The farthest ground range from `height_ft` over the target that still
/// lies inside a slant reach of `slant_ft`.
pub fn ground_reach(slant_ft: f64, height_ft: f64) -> f64 {
    (slant_ft * slant_ft - height_ft * height_ft).max(0.).sqrt()
}
