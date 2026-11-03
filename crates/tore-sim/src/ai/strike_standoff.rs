//! A fixed-wing striker's stand-off release outside known rings
//! (docs/spec/ai-strike.md, "Stand-off reach"; T.O.R.E opinionated change,
//! John 2026-10-29: "If an attack aircraft can hit outside of range safely
//! of known threats AI should do that").
//!
//! With a stand-off store selected (a seeker missile, or a guided store
//! released on its sight's bar: a guided bomb, a laser store on a spot
//! that is not its own) and a known defence whose reach covers the target,
//! the striker checks its usual release point: the seeker's fire range, or
//! the sight's bar. When that point and the turn away after it stand outside
//! every known ring ([`threat::jet_rings`]), the plan is that point. Else
//! it looks farther out along its line to the target, on the
//! [`reach::SHOT_STEP_FT`] grid, to the store's reliable reach
//! ([`reach::reliable`], the host's exact-point answers): the nearest range
//! whose release point and turn stand outside every ring is its plan
//! ([`ReleasePlan`]). It releases there once the store reaches
//! ([`super::Solution::reaches`]), turns away from the target and opens to
//! the plan's range plus its turn plus [`STANDOFF_EXIT_FT`] before it turns
//! back. With no such range, or with no defence covering the target, the
//! pass is flown as before. The pass types, the line flown and the
//! committed-to-release window are unchanged; the window times the planned
//! release.
use super::reach::{self, ShotLaunch};
use super::{Context, Solution, StoreKind, Strike, bearing_deg, horizontal, turn_radius_ft};
use crate::ai::helicopter_threat as threat;
use crate::attitude::Vector;

/// A pass that has closed this far inside its planned range without
/// releasing gives the plan up and is flown as before; or, while its last
/// stand-off round still flies, ends there (fitted, agent).
pub const PLAN_SLACK_FT: f64 = 2_000.;
/// After a stand-off release the exit opens this far past the plan's range
/// and turn before it turns back (fitted, agent, as the helicopter's
/// stand-off exit).
pub const STANDOFF_EXIT_FT: f64 = 6_000.;
/// A stand-off round is taken to fly its time of flight plus this, seconds
/// (fitted, agent).
pub const ROUND_MARGIN_S: f64 = 5.;
/// The turn away after the release carries the striker on toward the target
/// by this many turn radii at its speed (fitted, agent: a level turn's
/// half circle reaches one radius ahead; the input-only adapter turns more
/// gently than the limit).
pub const ADVANCE_TURNS: f64 = 2.;
/// A round's time of flight from a usual release the host gives no answer
/// for is its range at this speed, ft/s (fitted, agent: only the striker's
/// wait for its round reads it).
pub const ROUND_FALLBACK_FPS: f64 = 1_000.;

/// A stand-off release planned outside known rings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReleasePlan {
    /// The release's ground range from the target, feet.
    pub range_ft: f64,
    /// The round's time of flight from there, seconds.
    pub seconds: f64,
    /// How far the turn away carries it on toward the target, feet.
    pub advance_ft: f64,
}

/// The point `range_ft` from `target` toward `own`, on the ground.
fn toward(target: Vector, own: Vector, range_ft: f64) -> Vector {
    let b = bearing_deg(target, own).to_radians();
    [
        target[0] + b.sin() * range_ft,
        own[1],
        target[2] + b.cos() * range_ft,
    ]
}

impl Strike {
    /// The selected store is one a stand-off release is planned for.
    fn stand_off_store(&self, cx: &Context<'_>) -> bool {
        match self.kind {
            Some(kind) if kind.seeker() => true,
            Some(StoreKind::Coordinate) => true,
            // A laser store on its own spot holds the spot after the
            // release (slice ST-L): it stays at the target.
            Some(StoreKind::Laser) => cx.inputs.lase.sensor.is_none(),
            _ => false,
        }
    }

    /// The stand-off release for the pass in hand at a target at `target`,
    /// if one is planned (see the module notes).
    pub(super) fn plan_release(
        &self,
        cx: &Context<'_>,
        target: Vector,
        solution: Option<Solution>,
    ) -> Option<ReleasePlan> {
        if !self.stand_off_store(cx) || self.takeover.is_some() {
            return None;
        }
        let store = self.store(cx)?;
        let rings = threat::jet_rings(cx.picture, cx.side);
        // Only a known defence whose own reach covers the target changes the
        // pass.
        let covered = rings
            .iter()
            .any(|r| horizontal(r.centre, target) < r.radius_ft - threat::RING_MARGIN_FT);
        if !covered {
            return None;
        }
        let own = cx.own;
        let height = own.position[1] - target[1];
        let usual = if store.kind.seeker() {
            reach::ground_reach(store.fire_range_ft, height)
        } else {
            solution?.bar_ft?
        };
        let advance = ADVANCE_TURNS * turn_radius_ft(own.speed.0, own.g_limit);
        let safe = |range: f64| {
            let point = toward(target, own.position, range);
            let end = toward(target, own.position, (range - advance).max(0.));
            threat::inside(&rings, point).is_none() && threat::clear(&rings, point, end)
        };
        let launch = ShotLaunch {
            target_ft: target[1],
            height_ft: height,
            speed_fps: own.speed.0,
        };
        if safe(usual) {
            // Its usual release already stands outside: it stays where it
            // was, and the plan keeps the turn away after it.
            let seconds = cx
                .inputs
                .shots
                .shot(store.station, launch, reach::on_grid(usual))
                .unwrap_or(usual / ROUND_FALLBACK_FPS);
            return Some(ReleasePlan {
                range_ft: usual,
                seconds,
                advance_ft: advance,
            });
        }
        let cap = reach::ground_reach(store.maximum_range_ft, height);
        reach::ranges_beyond(usual, cap)
            .filter(|range| safe(*range))
            .find_map(|range| {
                reach::reliable(&cx.inputs.shots, store.station, launch, range).map(|seconds| {
                    ReleasePlan {
                        range_ft: range,
                        seconds,
                        advance_ft: advance,
                    }
                })
            })
    }

    /// Plans the pass's stand-off release again this step, before it
    /// releases: a ring learned since, or a new height, may move it. A pass
    /// that has closed [`PLAN_SLACK_FT`] inside its plan without releasing
    /// gives it up (and is flown as before), unless its last stand-off
    /// round still flies: then it ends there, and the answer is true.
    pub(super) fn update_release_plan(
        &mut self,
        cx: &Context<'_>,
        target: Vector,
        solution: Option<Solution>,
    ) -> bool {
        if self.released > 0 {
            return false;
        }
        if self.plan_dropped {
            self.release_plan = None;
            return false;
        }
        self.release_plan = self.plan_release(cx, target, solution);
        let Some(plan) = self.release_plan else {
            return false;
        };
        if horizontal(cx.own.position, target) < plan.range_ft - PLAN_SLACK_FT {
            if self.round_flying(cx.tick) {
                return true;
            }
            self.plan_dropped = true;
            self.release_plan = None;
        }
        false
    }

    /// The planned release is due: the store reaches from here and the
    /// striker is at or inside the plan's range.
    pub(super) fn planned_due(&self, cx: &Context<'_>, target: Vector, s: &Solution) -> bool {
        self.release_plan
            .is_some_and(|plan| s.reaches && horizontal(cx.own.position, target) <= plan.range_ft)
    }

    /// The pass released on a plan: its round flies for the plan's time.
    pub(super) fn released_on_plan(&mut self, tick: u64) {
        if let Some(plan) = self.release_plan {
            let ticks = ((plan.seconds + ROUND_MARGIN_S) * 120.).ceil() as u64;
            self.standoff_until = Some(tick + ticks);
        }
    }

    /// Its last stand-off round still flies.
    fn round_flying(&self, tick: u64) -> bool {
        self.standoff_until.is_some_and(|until| tick < until)
    }

    /// The pass in hand is a stand-off: it released on its plan, or ended
    /// at the plan while its last round flies. The exit then turns away from
    /// the target.
    pub(super) fn standing_off(&self, tick: u64) -> bool {
        self.release_plan.is_some() && (self.released > 0 || self.round_flying(tick))
    }

    /// The exit distance after a stand-off: the plan's range, its turn and
    /// [`STANDOFF_EXIT_FT`].
    pub(super) fn standoff_exit_ft(&self, tick: u64) -> Option<f64> {
        self.release_plan
            .filter(|_| self.standing_off(tick))
            .map(|plan| plan.range_ft + plan.advance_ft + STANDOFF_EXIT_FT)
    }
}

#[cfg(test)]
#[path = "strike_standoff_tests.rs"]
mod tests;
