//! The move-ahead ambush: a helicopter's stand-off shot at a moving target
//! from a point ahead of its path (docs/spec/ai-strike.md, "Move-ahead
//! ambush"; T.O.R.E opinionated change, John 2026-10-29: "whatever is
//! realistic here, seems like a move ahead ambush makes sense", under his
//! rule that an attacker able to hit from outside every known threat does).
//!
//! A stand-off planned on a moving target's place now stops holding within
//! seconds: the column drives on among guns that stand still, and each
//! firing point it planned no longer lies at the right range from the
//! target, or its advance now runs into a ring. The move-ahead plan looks
//! where the target will be instead:
//!
//! - **Prediction.** The target keeps its perceived ground velocity (the AI
//!   is not told a column's route; its lists give it each contact's place
//!   and velocity). A known defence the lists hold moving (a gun that drives
//!   with the column) keeps its own velocity, and its ring moves with it;
//!   every other ring stands where the site is known.
//! - **The search.** For each fire time ahead, in [`TIME_STEP_S`] steps out
//!   to [`HORIZON_S`], each firing range whose shot the host says hits
//!   reliably (nearest first), and each bearing from the target's place
//!   then (the helicopter's own side first, as the stand-off's), a firing
//!   point at that range and bearing from the predicted place is taken when
//!   the target's path comes to that range of it ([`window`]) and the
//!   helicopter can be there (nose on, by the lead-in point unless it is
//!   already heading in, [`route`]) while it is, waiting no longer than
//!   [`MAX_WAIT_S`]; the shot's time is the later of the two. The point
//!   must keep outside every ring, each where it will be at that time: on
//!   the way in, over the wait (a [`WAIT_RADIUS_FT`] circle about the
//!   lead-in point behind the firing point, when there is time to spare)
//!   and over the advance after the shot, the store's flight to where it
//!   meets the target ([`intercept`]) flown at the guiding speed, the nose
//!   on the target or held off it inside the launcher's cone
//!   ([`GUIDE_OFFSETS_DEG`]); and the shot must hit reliably both at the
//!   target's place at the shot and where the store meets it. The search
//!   keeps [`PLAN_MARGIN_FT`] more room than a plan in hand needs. The
//!   first such plan is flown: the earliest fire time step, then the
//!   nearest range, then the bearing nearest the helicopter's.
//! - **Flying it.** It flies to the wait circle and circles there masked,
//!   leaves it in time to reach the firing point as the target comes to the
//!   planned range ([`go`]), unmasks there with its nose on the target, and
//!   fires once the target is within a range step of the planned range and
//!   the shot from where it is, now, still hits and leaves its advance
//!   clear ([`shot_clear`], the last check); it guides with the nose offset
//!   that check found. Not away by [`ADVANCE_MARGIN_FT`] past the firing
//!   point ([`past_ft`]), it gives the pass up.
//! - **Re-planning** ([`holds`]): the plan is a firing point and a range.
//!   It is planned again when the target stops, or turns so that its path
//!   no longer brings it within a range step of that range of the point
//!   ([`window`]), when
//!   the helicopter can no longer be there in time, when the shot from there
//!   would no longer hit, or when a ring (a new one, or a moving one) comes
//!   over the wait or the advance. A target that turns but still drives
//!   into range is waited for. A plan stands at least [`REPLAN_TICKS`].
//!
//! A static target, or a moving one with no move-ahead point, is attacked as
//! before. Everything is a fixed function of the picture: no dice. The
//! numbers are fitted (agent).
use crate::ai::helicopter_threat::{
    ADVANCE_MARGIN_FT, FIRE_RANGE_SHARE, HOLD_RADIUS_FT, LEAD_IN_FT, Ring, STANDOFF_SHARE,
    StandOff, along, approach_bearings, bearing, horizontal,
};
use crate::ai::strike::reach;
use crate::attitude::Vector;

/// A target slower than this, ft/s, is attacked as a static one (fitted,
/// agent: a column halted to fight creeps, a driving one makes 16 to 50).
pub const MOVING_FPS: f64 = 5.;
/// How far ahead the target's path is predicted, seconds (fitted, agent:
/// the hold-off's three minutes; a column's legs last six minutes or more,
/// so a straight line holds that long).
pub const HORIZON_S: f64 = 180.;
/// The fire times tried, seconds apart (fitted, agent: 250 ft of a
/// column's travel, half the shot table's range step).
pub const TIME_STEP_S: f64 = 5.;
/// It is planned to unmask this long before the shot (fitted, agent: the
/// nose comes onto the target as it reaches the firing point).
pub const LEAD_S: f64 = 2.;
/// Heading in within this of the firing radial, degrees, it flies
/// straight to the firing point; else by the lead-in point (fitted,
/// agent). The unmask waits for the nose within this of the target too.
pub const INBOUND_DEG: f64 = 30.;
/// With more time than this to spare, seconds, it waits on the circle
/// (fitted, agent); with less it flies straight on in.
pub const WAIT_SLACK_S: f64 = 5.;
/// The longest it waits for the target at the ambush, seconds (fitted,
/// agent, the hover time limit: a longer wait trusts the prediction too
/// far).
pub const MAX_WAIT_S: f64 = 120.;
/// The circle it waits on: the hold-off's (a helicopter cannot hover on
/// the AI's adapter), about the lead-in point [`LEAD_IN_FT`] behind the
/// firing point, so the circle runs through the firing point.
pub const WAIT_RADIUS_FT: f64 = HOLD_RADIUS_FT;
/// The search keeps this much farther from every ring than a plan in hand
/// must (fitted, agent): the target's perceived course wanders a little
/// and the advance's end with it, and a plan made on the edge would be
/// dropped at once.
pub const PLAN_MARGIN_FT: f64 = 500.;
/// While it guides, the nose may be held this far off the target, tried in
/// this order, degrees (fitted, agent): inside the launcher's 45 degree
/// zone-1 cone, which command guidance needs every tick, with at least 5
/// to spare (the adapter holds a commanded heading within 2 in the
/// heavy-AAA run). Off the nose the advance closes on the target more
/// slowly and slides to one side, away from a ring the straight line would
/// run into.
pub const GUIDE_OFFSETS_DEG: [f64; 5] = [0., 30., -30., 40., -40.];
/// The advance is followed in steps of this, seconds (fitted, agent).
pub const GUIDE_STEP_S: f64 = 1.;
/// A plan is kept at least this long before it is checked again (fitted,
/// agent: one second).
pub const REPLAN_TICKS: u64 = 120;
/// A move-ahead is looked for at most this often, ticks (fitted, agent):
/// the full search is the costly part ([`search_due`]).
pub const SEARCH_TICKS: u64 = 120;
/// Whether a move-ahead may be searched for at `tick` ([`SEARCH_TICKS`]).
pub fn search_due(tick: u64) -> bool {
    tick.is_multiple_of(SEARCH_TICKS)
}
/// Ticks per second.
const TICKS: f64 = 120.;

/// A ground object as the AI perceives it: where it is and its ground
/// velocity (the vertical part dropped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub position: Vector,
    pub velocity: Vector,
}

impl Track {
    pub fn new(position: Vector, velocity: Vector) -> Self {
        Self {
            position,
            velocity: [velocity[0], 0., velocity[2]],
        }
    }
    pub fn speed_fps(&self) -> f64 {
        self.velocity[0].hypot(self.velocity[2])
    }
    /// Moving fast enough to be led ([`MOVING_FPS`]).
    pub fn moving(&self) -> bool {
        self.speed_fps() >= MOVING_FPS
    }
    /// Where it will be in `seconds`.
    pub fn at(&self, seconds: f64) -> Vector {
        [
            self.position[0] + self.velocity[0] * seconds,
            self.position[1],
            self.position[2] + self.velocity[2] * seconds,
        ]
    }
}

/// A ring and the ground velocity of its site (zero for one that stands).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovingRing {
    pub ring: Ring,
    pub velocity: Vector,
}

impl MovingRing {
    pub fn standing(ring: Ring) -> Self {
        Self {
            ring,
            velocity: [0.; 3],
        }
    }
    /// Its centre in `seconds`.
    pub fn at(&self, seconds: f64) -> Vector {
        Track::new(self.ring.centre, self.velocity).at(seconds)
    }
}

/// A move-ahead plan (checkpointed in the strike state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ahead {
    /// The firing point.
    pub point: Vector,
    /// The wait circle's centre, the lead-in point behind the firing point.
    pub orbit: Vector,
    /// Where the target was predicted to be at the shot.
    pub aim: Vector,
    /// The firing range from there, feet.
    pub range_ft: f64,
    /// While it guides, the nose is held this far off the target, degrees
    /// (positive to the right; [`GUIDE_OFFSETS_DEG`]).
    pub offset_deg: f64,
    /// The tick of the shot, as planned.
    pub fire_tick: u64,
    /// The tick it was planned.
    pub planned_tick: u64,
}

/// The firing ranges tried, nearest first: the stand-off's usual ranges
/// and, with the host's answers, every range step out to the store's reach
/// whose shot hits reliably.
fn ranges(standoff: &StandOff<'_>) -> Vec<f64> {
    let usual = [
        STANDOFF_SHARE * standoff.reach_ft,
        FIRE_RANGE_SHARE * standoff.fire_ft,
    ];
    let mut out: Vec<f64> = match standoff.shots {
        Some(shots) => {
            let cap = reach::ground_reach(standoff.reach_ft, shots.launch.height_ft);
            usual
                .into_iter()
                .chain(reach::ranges_beyond(usual[0].max(usual[1]), cap))
                .filter(|range| flight_to(standoff, *range).is_some())
                .collect()
        }
        None => usual.into_iter().filter(|r| *r > 0.).collect(),
    };
    out.sort_by(f64::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < 1.);
    out
}

/// The store's flight to a static target `range_ft` away, seconds, when
/// the shot hits: reliably on the host's answers ([`reach::reliable`]),
/// else inside its reach at its known time of flight.
fn flight_to(standoff: &StandOff<'_>, range_ft: f64) -> Option<f64> {
    match standoff.shots {
        Some(shots) => reach::reliable(shots.shots, shots.station, shots.launch, range_ft),
        None => (range_ft <= standoff.reach_ft).then(|| {
            standoff.flight.map_or_else(
                || standoff.advance_ft(range_ft) / standoff.guide_fps.max(1.),
                |f| f.seconds_to(range_ft),
            )
        }),
    }
}

/// The store's flight from `point` at a shot `fire_s` from now to where the
/// moving target will be when it gets there (two rounds of the intercept:
/// a target driving away is struck farther out, later), when the shot hits
/// reliably both at the target's place at the shot and there.
fn intercept(standoff: &StandOff<'_>, point: Vector, fire_s: f64, target: &Track) -> Option<f64> {
    let mut flight = flight_to(standoff, horizontal(point, target.at(fire_s)))?;
    for _ in 0..2 {
        flight = flight_to(standoff, horizontal(point, target.at(fire_s + flight)))?;
    }
    Some(flight)
}

/// The helicopter as the plan needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flier {
    pub position: Vector,
    pub heading_deg: f64,
    /// Its speed on the strike legs, ft/s.
    pub speed_fps: f64,
}

/// The way from `own` to the firing point `point` (fired at a target at
/// `aim`) as the helicopter flies it ([`go`]): straight in when it is on
/// the firing radial beyond the point heading in (within [`INBOUND_DEG`]),
/// else by the lead-in point, so it arrives nose on. Whether it is straight
/// in, and the way's length, feet.
pub fn route(own: &Flier, aim: Vector, point: Vector) -> (bool, f64) {
    let at = own.position;
    let range = horizontal(aim, point).max(1.);
    let u = [(point[0] - aim[0]) / range, (point[2] - aim[2]) / range];
    let (dx, dz) = (at[0] - aim[0], at[2] - aim[2]);
    let along_ft = dx * u[0] + dz * u[1];
    let across = (dx * u[1] - dz * u[0]).abs();
    let off = (own.heading_deg - bearing(point, aim)).rem_euclid(360.);
    let inbound = off.min(360. - off) <= INBOUND_DEG;
    if along_ft >= range && across <= crate::ai::strike::helicopter::ON_RADIAL_FT && inbound {
        (true, horizontal(at, point))
    } else {
        let lead = lead_in(aim, point);
        (false, horizontal(at, lead) + LEAD_IN_FT)
    }
}

/// The lead-in point: [`LEAD_IN_FT`] beyond the firing point on its radial
/// from `aim`.
fn lead_in(aim: Vector, point: Vector) -> Vector {
    let [x, z] = along(
        aim,
        bearing(aim, point),
        horizontal(aim, point) + LEAD_IN_FT,
    );
    [x, point[1], z]
}

/// How far `own` has run past the firing point toward the aim point, feet
/// (negative short of it): the shot leaves no later than
/// [`ADVANCE_MARGIN_FT`] past it, the unmask the advance allows for.
pub fn past_ft(plan: &Ahead, own: Vector) -> f64 {
    let range = horizontal(plan.aim, plan.point).max(1.);
    let u = [
        (plan.aim[0] - plan.point[0]) / range,
        (plan.aim[2] - plan.point[2]) / range,
    ];
    (own[0] - plan.point[0]) * u[0] + (own[2] - plan.point[2]) * u[1]
}

/// Whether a helicopter flying straight from `a` at `ta` seconds to `b` at
/// `tb` stays outside `ring` widened by `extra`, the ring moving with its
/// site. With `may_leave` a start inside counts when the way leads straight
/// out (the helicopter may always leave a ring), as
/// [`crate::ai::helicopter_threat::clear`].
fn passes(
    ring: &MovingRing,
    extra: f64,
    (a, ta): (Vector, f64),
    (b, tb): (Vector, f64),
    may_leave: bool,
) -> bool {
    let (ca, cb) = (ring.at(ta), ring.at(tb));
    let a = [a[0] - ca[0], a[2] - ca[2]];
    let b = [b[0] - cb[0], b[2] - cb[2]];
    let radius = ring.ring.radius_ft + extra;
    if a[0].hypot(a[1]) < radius {
        let outward = (b[0] - a[0]) * a[0] + (b[1] - a[1]) * a[1] >= 0.;
        return may_leave && outward && b[0].hypot(b[1]) >= radius;
    }
    // The nearest the relative path comes to the ring's centre.
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dz * dz;
    let t = if length2 > 0. {
        (-(a[0] * dx + a[1] * dz) / length2).clamp(0., 1.)
    } else {
        0.
    };
    (a[0] + t * dx).hypot(a[1] + t * dz) >= radius
}

/// One firing point at one fire time, as the search and [`holds`] judge it.
struct Shot {
    point: Vector,
    orbit: Vector,
    flight_s: f64,
    /// Seconds from now.
    fire_s: f64,
}

impl Shot {
    /// The helicopter flying in from `own` stays outside every ring
    /// widened by `margin`, each where it is then: on its way in (with
    /// `way_in`: straight to the firing point, or to the lead-in point and
    /// in along the radial); and, when it has more than [`WAIT_SLACK_S`] to
    /// spare, on the wait circle from when it gets there to the shot.
    fn wait_safe(
        &self,
        own: &Flier,
        aim: Vector,
        rings: &[MovingRing],
        way_in: bool,
        margin: f64,
    ) -> bool {
        let speed = own.speed_fps.max(1.);
        let at = own.position;
        let (straight, way) = route(own, aim, self.point);
        let waits = self.fire_s - (way / speed + LEAD_S) > WAIT_SLACK_S;
        let lead = self.orbit;
        let to_circle = horizontal(at, lead) - WAIT_RADIUS_FT;
        let reach_s = (to_circle.max(0.) / speed).min(self.fire_s);
        let entry = if to_circle > 0. {
            let k = to_circle / (to_circle + WAIT_RADIUS_FT);
            [
                at[0] + (lead[0] - at[0]) * k,
                at[1],
                at[2] + (lead[2] - at[2]) * k,
            ]
        } else {
            at
        };
        let inbound_s = LEAD_IN_FT / speed;
        let shot_s = (self.fire_s - LEAD_S).max(0.);
        rings.iter().all(|r| {
            let way_clear = !way_in
                || if waits {
                    to_circle <= 0. || passes(r, margin, (at, 0.), (entry, reach_s), true)
                } else if straight {
                    passes(r, margin, (at, 0.), (self.point, shot_s), true)
                } else {
                    let at_lead = (shot_s - inbound_s).max(0.);
                    passes(r, margin, (at, 0.), (lead, at_lead), true)
                };
            let wait_clear = if waits {
                passes(
                    r,
                    WAIT_RADIUS_FT + margin,
                    (lead, reach_s),
                    (lead, self.fire_s),
                    false,
                )
            } else {
                straight
                    || passes(
                        r,
                        margin,
                        (lead, (shot_s - inbound_s).max(0.)),
                        (self.point, shot_s),
                        false,
                    )
            };
            way_clear && wait_clear
        })
    }

    /// The advance after the shot stays outside every ring widened by
    /// `margin`, each where it is then: the unmask's last
    /// [`ADVANCE_MARGIN_FT`] straight at the target, then the store's
    /// flight at the guiding speed, the nose held `offset_deg` off the
    /// target (positive to the right), followed in [`GUIDE_STEP_S`] steps.
    fn guide_safe(
        &self,
        target: &Track,
        guide_fps: f64,
        offset_deg: f64,
        rings: &[MovingRing],
        margin: f64,
    ) -> bool {
        let mut at = self.point;
        let mut t = self.fire_s;
        let leg = |from: Vector, ta: f64, to: Vector, tb: f64| {
            rings
                .iter()
                .all(|r| passes(r, margin, (from, ta), (to, tb), false))
        };
        let aim = target.at(t);
        let first = ADVANCE_MARGIN_FT.min(horizontal(at, aim));
        let [x, z] = along(at, bearing(at, aim), first);
        let next = [x, at[1], z];
        let tn = t + first / guide_fps;
        if !leg(at, t, next, tn) {
            return false;
        }
        (at, t) = (next, tn);
        let end = self.fire_s + self.flight_s;
        while t < end {
            let step = GUIDE_STEP_S.min(end - t);
            let aim = target.at(t);
            let left = horizontal(at, aim);
            if left <= guide_fps * step {
                break;
            }
            let [x, z] = along(at, bearing(at, aim) + offset_deg, guide_fps * step);
            let next = [x, at[1], z];
            if !leg(at, t, next, t + step) {
                return false;
            }
            (at, t) = (next, t + step);
        }
        true
    }
}

/// The move-ahead plan on `target` from `own`, if
/// one stands clear of every ring (see the module notes). `None` for a
/// target that is not moving or a store with no stand-off reach.
pub fn plan(
    own: &Flier,
    tick: u64,
    target: Track,
    standoff: &StandOff<'_>,
    rings: &[MovingRing],
) -> Option<Ahead> {
    if !target.moving() || standoff.reach_ft <= 0. {
        return None;
    }
    let speed = own.speed_fps.max(1.);
    let guide_fps = standoff.guide_fps.max(1.);
    let ranges = ranges(standoff);
    if ranges.is_empty() {
        return None;
    }
    let steps = (HORIZON_S / TIME_STEP_S).round() as u32;
    for step in 1..=steps {
        let step_s = f64::from(step) * TIME_STEP_S;
        let step_aim = target.at(step_s);
        let base = bearing(step_aim, own.position);
        for &range_ft in &ranges {
            for (_, theta) in approach_bearings(base) {
                let [x, z] = along(step_aim, theta, range_ft);
                let point = [x, own.position[1], z];
                // The target may come into range of this point before the
                // step's time: the shot is timed on that, at the place the
                // target is then.
                let Some((t_in, t_out)) = window(point, range_ft, &target) else {
                    continue;
                };
                let arrive_s = route(own, target.at(t_in), point).1 / speed + LEAD_S;
                let fire_s = t_in.max(arrive_s);
                if fire_s > t_out || fire_s - arrive_s > MAX_WAIT_S {
                    continue;
                }
                let aim = target.at(fire_s);
                let mut shot = Shot {
                    point,
                    orbit: lead_in(aim, point),
                    flight_s: 0.,
                    fire_s,
                };
                if !shot.wait_safe(own, aim, rings, true, PLAN_MARGIN_FT) {
                    continue;
                }
                let Some(flight_s) = intercept(standoff, point, fire_s, &target) else {
                    continue;
                };
                shot.flight_s = flight_s;
                let Some(offset_deg) = GUIDE_OFFSETS_DEG.into_iter().find(|offset| {
                    shot.guide_safe(&target, guide_fps, *offset, rings, PLAN_MARGIN_FT)
                }) else {
                    continue;
                };
                return Some(Ahead {
                    point,
                    orbit: shot.orbit,
                    aim,
                    range_ft,
                    offset_deg,
                    fire_tick: tick + (fire_s * TICKS).round() as u64,
                    planned_tick: tick,
                });
            }
        }
    }
    None
}

/// The stretch of time from now, within [`HORIZON_S`], that the target as
/// it drives now lies within `range_ft` of `point`: when it comes in (now
/// when it is already inside) and when it leaves (the horizon when it is
/// still inside then). `None` when its path never comes that near in time.
pub fn window(point: Vector, range_ft: f64, target: &Track) -> Option<(f64, f64)> {
    let d = [target.position[0] - point[0], target.position[2] - point[2]];
    let v = [target.velocity[0], target.velocity[2]];
    let a = v[0] * v[0] + v[1] * v[1];
    let b = 2. * (d[0] * v[0] + d[1] * v[1]);
    let c = d[0] * d[0] + d[1] * d[1] - range_ft * range_ft;
    let disc = b * b - 4. * a * c;
    if a <= 0. {
        return (c <= 0.).then_some((0., HORIZON_S));
    }
    if disc < 0. {
        return None;
    }
    let root = disc.sqrt();
    let t_in = ((-b - root) / (2. * a)).max(0.);
    let t_out = ((-b + root) / (2. * a)).min(HORIZON_S);
    (t_in < t_out).then_some((t_in, t_out))
}

/// Time to leave the wait circle for the firing point: the way there, less
/// the unmask's last [`APPROACH_REACHED_FT`](crate::ai::strike::helicopter::APPROACH_REACHED_FT),
/// plus [`LEAD_S`], takes as long as the target, as it drives now, has left
/// to come to the planned range (or within a range step of it).
pub fn go(plan: &Ahead, own: &Flier, target: &Track) -> bool {
    let way = (route(own, plan.aim, plan.point).1
        - crate::ai::strike::helicopter::APPROACH_REACHED_FT)
        .max(0.);
    window(plan.point, plan.range_ft, target)
        .or_else(|| window(plan.point, plan.range_ft + reach::SHOT_STEP_FT, target))
        .is_none_or(|(t_in, _)| way / own.speed_fps.max(1.) + LEAD_S >= t_in)
}

/// Whether the plan in hand still stands (see the module notes), and with
/// it the nose's offset to guide with ([`GUIDE_OFFSETS_DEG`], the planned
/// one first): the target, as it drives now, still comes within a range
/// step of the planned range of the firing point while the helicopter can
/// be there; the shot from there then still hits; and every ring, where it
/// will be, still leaves the way in along the radial, the wait and the
/// advance clear. The way to the lead-in point is not judged again: the
/// helicopter steers round the rings on it.
pub fn holds(
    plan: &Ahead,
    own: &Flier,
    target: &Track,
    standoff: &StandOff<'_>,
    rings: &[MovingRing],
) -> Option<f64> {
    if !target.moving() {
        return None;
    }
    // Timed on the planned range, as [`go`] times the way in; a range step
    // farther out gives the time it may come late by.
    let (wide_in, t_out) = window(plan.point, plan.range_ft + reach::SHOT_STEP_FT, target)?;
    let t_in = window(plan.point, plan.range_ft, target).map_or(wide_in, |w| w.0);
    let arrive_s = route(own, plan.aim, plan.point).1 / own.speed_fps.max(1.) + LEAD_S;
    if arrive_s > t_out {
        return None;
    }
    let fire_s = t_in.max(arrive_s);
    let flight_s = intercept(standoff, plan.point, fire_s, target)?;
    let shot = Shot {
        point: plan.point,
        orbit: plan.orbit,
        flight_s,
        fire_s,
    };
    if !shot.wait_safe(own, plan.aim, rings, false, 0.) {
        return None;
    }
    std::iter::once(plan.offset_deg)
        .chain(GUIDE_OFFSETS_DEG)
        .find(|offset| shot.guide_safe(target, standoff.guide_fps.max(1.), *offset, rings, 0.))
}

/// The shot from where the helicopter is, now: when it hits reliably (at
/// the target's place now and where the store meets it) and its advance
/// stays outside every ring, each where it will be, the nose's offset to
/// guide with (`planned` first, then [`GUIDE_OFFSETS_DEG`] in order). The
/// last check before a move-ahead shot leaves: the plan timed it, this
/// judges the shot actually there.
pub fn shot_clear(
    own: &Flier,
    target: &Track,
    standoff: &StandOff<'_>,
    rings: &[MovingRing],
    planned: f64,
) -> Option<f64> {
    let flight_s = intercept(standoff, own.position, 0., target)?;
    let shot = Shot {
        point: own.position,
        orbit: own.position,
        flight_s,
        fire_s: 0.,
    };
    std::iter::once(planned)
        .chain(GUIDE_OFFSETS_DEG)
        .find(|offset| shot.guide_safe(target, standoff.guide_fps.max(1.), *offset, rings, 0.))
}

/// The rings with their sites' motion: a ring whose site the AI's lists
/// hold moving (`velocity` gives its perceived ground velocity) centres on
/// its perceived place now and moves with it; every other stands.
pub fn moving_rings(rings: &[Ring], velocity: &dyn Fn(u32) -> Option<Track>) -> Vec<MovingRing> {
    rings
        .iter()
        .map(|ring| match velocity(ring.id).filter(Track::moving) {
            Some(track) => MovingRing {
                ring: Ring {
                    centre: track.position,
                    ..*ring
                },
                velocity: track.velocity,
            },
            None => MovingRing::standing(*ring),
        })
        .collect()
}

#[cfg(test)]
#[path = "helicopter_ahead_tests.rs"]
mod tests;
