//! The guided air-to-ground weapons: laser bombs and missiles, the JDAMs and
//! the AT-2 (docs/spec/air-to-ground.md, "Weapon classes" 4, 5, 7 and 12, and
//! "Guided bomb release range indicator"). Their flight in combat is
//! `live::guided`; this module holds what both the flight and the cockpit
//! need: which records they are, how they leave the aircraft, the zone-1
//! cones, the fly-out that says whether one can still reach a point and how
//! long it takes, and the reach the HUD's release range scale shows.
//!
//! Bombs use the fitted gravity, drag and steering model in `bomb_flight`.
//! Powered weapons retain their missile motor and cruise profiles.
use super::live::Launcher;
use super::missiles::{self, DT, Guidance, Motion};
use crate::attitude::{Basis, Vector, dot, unit};
use crate::combat::EnginePhase;
use tore_formats::weapons::{Movement, Weapon, Zone};

/// A hit, as the missiles' fly-out counts one: within 25 ft.
pub const HIT_FT: f64 = 25.;
/// Samples of the reach search across the zone's ground range, before each
/// edge is bisected.
const REACH_SAMPLES: usize = 8;
/// Bisection steps for each sampled inside/outside reach boundary.
const REACH_BISECTIONS: usize = 8;
/// The steering error, radians, below which a fly-out treats the rest of
/// the path as straight and works it out from the record's speeds.
const STRAIGHT_RAD: f64 = 0.002;

/// The guidance of an air-to-ground weapon record: laser, coordinate (the
/// JDAMs) or command (the AT-2). `None` for any other record.
pub fn kind(w: &Weapon) -> Option<Guidance> {
    Guidance::air_to_ground(w)
}

/// A guided bomb: a guided air-to-ground record with no motor (the laser
/// bombs and the JDAMs), advanced by the shared bomb integrator.
pub fn is_glide_bomb(w: &Weapon) -> bool {
    kind(w).is_some() && w.movement.fuel_t <= w.movement.ignite_t
}

/// Guided weapons inherit the aircraft's velocity at release.
pub fn release_velocity(_w: &Weapon, launcher: &Launcher) -> Vector {
    launcher.velocity
}

/// The frame of a weapon flying along `direction`, wings level: what its
/// seeker's cone is measured about in flight.
pub fn nose(direction: Vector) -> Basis {
    let yaw = direction[0].atan2(direction[2]);
    let pitch = direction[1].atan2(direction[0].hypot(direction[2]));
    Basis::new(yaw, pitch, 0.)
}

/// Whether `point` lies inside the weapon's zone-1 cone about `basis` at
/// `position`: heading and pitch each within the zone's half angles (45
/// degrees on every guided air-to-ground record), angles only, as the
/// retail lock reruns in flight.
pub fn in_cone(w: &Weapon, position: Vector, basis: Basis, point: Vector) -> bool {
    let zone = Zone {
        minimum_range: 0,
        maximum_range: i32::MAX,
        minimum_altitude: i32::MIN,
        maximum_altitude: i32::MAX,
        ..w.seeker.zones[1]
    };
    missiles::geometry(&zone, position, basis, point, None)
}

/// Whether `point` lies inside the weapon's zone 1 at release from a mount
/// at `position` pointing along `basis`: the cone, the range (its maximum
/// times `scale`, the weather's share for a laser weapon) and the
/// altitudes, as the retail lock tests at launch.
pub fn in_release_zone(
    w: &Weapon,
    position: Vector,
    basis: Basis,
    point: Vector,
    scale: f64,
) -> bool {
    let zone = w.seeker.zones[1];
    let zone = Zone {
        maximum_range: (f64::from(zone.maximum_range.max(0)) * scale.clamp(0., 1.)) as i32,
        ..zone
    };
    missiles::geometry(&zone, position, basis, point, None)
}

/// The slant range the weapon's zone 1 allows at release, feet, scaled.
pub fn zone_range(w: &Weapon, scale: f64) -> f64 {
    f64::from(w.seeker.zones[1].maximum_range.max(0)) * scale.clamp(0., 1.)
}

/// The last tick of the weapon's life.
fn life_ticks(m: &Movement) -> u64 {
    u64::from(m.remove_t) * 30
}

/// Seconds a weapon flying straight at `speed` ft/s from age `age` (ticks)
/// needs to cover `distance` feet, with the record's motor, slowdown and
/// final speed and the motor's thrust still unspent (`budget - gain`).
/// `None` when its life ends first. The coast is worked out in closed form;
/// a burning motor is stepped tick by tick.
fn straight_seconds(
    m: &Movement,
    mut speed: f64,
    mut gain: f64,
    budget: f64,
    age: u64,
    distance: f64,
) -> Option<f64> {
    let end = life_ticks(m);
    let mut tick = age;
    let mut covered = 0.;
    while tick < end && missiles::phase(m, tick) != EnginePhase::Coast {
        if missiles::phase(m, tick) == EnginePhase::Powered {
            let step = (f64::from(m.acceleration.max(0)) * DT).min((budget - gain).max(0.));
            gain += step;
            speed += step;
        }
        covered += speed * DT;
        tick += 1;
        if covered >= distance {
            return Some((tick - age) as f64 * DT);
        }
    }
    if tick >= end {
        return None;
    }
    let left = distance - covered;
    let deceleration = f64::from(m.deceleration.max(0));
    let floor = f64::from(m.final_speed.max(0)).min(speed);
    let coast = if speed > floor && deceleration > 0. {
        let slowing = (speed - floor) / deceleration;
        let slowed = (speed + floor) / 2. * slowing;
        if left <= slowed {
            (speed - (speed * speed - 2. * deceleration * left).max(0.).sqrt()) / deceleration
        } else if floor > 0. {
            slowing + (left - slowed) / floor
        } else {
            return None;
        }
    } else if speed > 0. {
        left / speed
    } else {
        return None;
    };
    let seconds = (tick - age) as f64 * DT + coast;
    (age as f64 * DT + seconds <= end as f64 * DT).then_some(seconds)
}

/// Predicted time to a fixed aim. Bombs use the shared 120 Hz integrator;
/// powered weapons retain their turn and coast predictor. A bomb's guidance
/// expires at record lifetime, but its fall continues to ground.
pub fn time_to(
    w: &Weapon,
    motion: Motion,
    position: Vector,
    forward: Vector,
    point: Vector,
    age: u64,
) -> Option<f64> {
    time_to_with_terrain(w, motion, position, forward, point, age, None)
}

/// Fly-out time with the live cruise clearance rule. Bomb reach and the
/// launch envelope stay energy-only; an in-flight cruise uses real terrain.
pub fn time_to_with_terrain(
    w: &Weapon,
    mut motion: Motion,
    mut position: Vector,
    mut forward: Vector,
    point: Vector,
    age: u64,
    ground: Option<&dyn Fn(f64, f64) -> f64>,
) -> Option<f64> {
    if is_glide_bomb(w) {
        return bomb_time_to(w, position, motion.velocity, point, age, ground);
    }
    let m = &w.movement;
    let end = life_ticks(m);
    for tick in age..end {
        let gap = missiles::sub(point, position);
        let distance = missiles::length(gap);
        if distance <= HIT_FT {
            return Some((tick - age) as f64 * DT);
        }
        let aim = ground.map_or_else(
            || motion.aim(position, point),
            |g| motion.aim_over_terrain(position, point, g),
        );
        let desired =
            missiles::commanded_heading(forward, motion.path(), unit(missiles::sub(aim, position)));
        let error = dot(forward, desired).clamp(-1., 1.).acos();
        let pointing = dot(forward, unit(gap)).clamp(-1., 1.).acos();
        if error < STRAIGHT_RAD
            && pointing < STRAIGHT_RAD
            && !(ground.is_some() && motion.cruise.is_some())
        {
            return straight_seconds(
                m,
                missiles::length(motion.velocity),
                motion.gain,
                motion.budget,
                tick,
                distance - HIT_FT,
            )
            .map(|rest| (tick - age) as f64 * DT + rest);
        }
        let next = missiles::steer(m, tick, forward, desired);
        motion.turn(forward, next);
        forward = next;
        let delta = motion.step(m, tick, forward);
        for (p, d) in position.iter_mut().zip(delta) {
            *p += d;
        }
    }
    None
}

/// A guided weapon's reach from where it would leave now: the nearest and
/// farthest ground range ahead at which it can still reach a point on the
/// ground (docs/spec/air-to-ground.md, "Guided bomb release range
/// indicator"). Slice H1's type; [`reach`] is the search for guided weapons
/// (H1's `Reach::search` starts from the unguided throw, which a guided
/// glide with no fall does not make good).
pub use super::delivery::Reach;

/// The ground track a release from `launcher` runs along: its velocity's,
/// else its nose's, as a horizontal unit vector.
pub fn ground_track(launcher: &Launcher) -> Option<[f64; 2]> {
    [launcher.velocity, launcher.basis.forward]
        .into_iter()
        .map(|v| (v[0], v[2]))
        .find(|(x, z)| x.hypot(*z) > 1e-6)
        .map(|(x, z)| {
            let length = x.hypot(z);
            [x / length, z / length]
        })
}

/// The reach of guided weapon `w` released now from `launcher` at a point
/// on ground `ground` feet high: the points straight ahead along the ground
/// track that lie inside zone 1 at release (range scaled by `scale`) and,
/// for a guided bomb, that its glide reaches within its life (a fly-out of
/// [`time_to`]). For a missile the fly-out is not run (its motor reaches
/// the zone, fitted). The first stretch of good points is taken, each edge
/// bisected. `None` when no point ahead qualifies.
///
/// Agent design: retail gives no release range cue (retail manual p. 125).
/// Deterministic, in a bounded number of fly-outs.
// A bounded memo of pure fly-outs. It has no effect on simulation state or
// results and is intentionally absent from checkpoints. Horizontal translation
// does not affect a flat-ground envelope; evaluate it at the origin every time.
type ReachMemo = Vec<(Weapon, [f64; 15], Option<Reach>)>;
thread_local! { static REACH_MEMO: std::cell::RefCell<ReachMemo> = const { std::cell::RefCell::new(Vec::new()) }; }

pub fn reach(w: &Weapon, launcher: &Launcher, ground: f64, scale: f64) -> Option<Reach> {
    let b = launcher.basis;
    let key = [
        launcher.position[1],
        launcher.velocity[0],
        launcher.velocity[1],
        launcher.velocity[2],
        b.forward[0],
        b.forward[1],
        b.forward[2],
        b.right[0],
        b.right[1],
        b.right[2],
        b.up[0],
        b.up[1],
        b.up[2],
        ground,
        scale,
    ];
    if let Some(cached) = REACH_MEMO.with(|memo| {
        memo.borrow()
            .iter()
            .find(|(weapon, k, _)| weapon == w && *k == key)
            .map(|(_, _, result)| *result)
    }) {
        return cached;
    }
    let launcher = Launcher {
        position: [0., launcher.position[1], 0.],
        ..*launcher
    };
    let result = reach_uncached(w, &launcher, ground, scale);
    REACH_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if memo.len() == 32 {
            memo.remove(0);
        }
        memo.push((w.clone(), key, result));
    });
    result
}

fn reach_uncached(w: &Weapon, launcher: &Launcher, ground: f64, scale: f64) -> Option<Reach> {
    let track = ground_track(launcher)?;
    let height = launcher.position[1] - ground;
    let slant = zone_range(w, scale);
    if slant <= height.abs() {
        return None;
    }
    let cap = ((slant * slant - height * height).sqrt() - 0.01).max(0.);
    let velocity = release_velocity(w, launcher);
    let glide = is_glide_bomb(w);
    let motion = Motion::launch(w, velocity, launcher.position[1]);
    let forward = unit(velocity);
    let at = |range: f64| -> Vector {
        [
            launcher.position[0] + track[0] * range,
            ground,
            launcher.position[2] + track[1] * range,
        ]
    };
    // Find the cheap geometric window first. Sampling the entire slant
    // range can miss a narrow valid window at high altitude.
    let zone = search_span(0., cap, 64, 16, |range| {
        in_release_zone(w, launcher.position, launcher.basis, at(range), scale)
    })?;
    if !glide {
        return Some(zone);
    }
    search_span(
        zone.minimum_ft,
        zone.maximum_ft,
        REACH_SAMPLES,
        REACH_BISECTIONS,
        |range| time_to(w, motion, launcher.position, forward, at(range), 0).is_some(),
    )
}

fn search_span(
    start: f64,
    end: f64,
    samples: usize,
    bisections: usize,
    good: impl Fn(f64) -> bool,
) -> Option<Reach> {
    let step = (end - start) / samples as f64;
    let at = |i: usize| start + i as f64 * step;
    let values: Vec<bool> = (0..=samples).map(|i| good(at(i))).collect();
    let first = values.iter().position(|ok| *ok)?;
    let last = first + values[first..].iter().take_while(|ok| **ok).count() - 1;
    let edge = |mut inside: f64, mut outside: f64| {
        for _ in 0..bisections {
            let middle = (inside + outside) / 2.;
            if good(middle) {
                inside = middle;
            } else {
                outside = middle;
            }
        }
        inside
    };
    Some(Reach {
        minimum_ft: if first == 0 {
            start
        } else {
            edge(at(first), at(first - 1))
        },
        maximum_ft: if last == samples {
            end
        } else {
            edge(at(last), at(last + 1))
        },
    })
}

/// Shared bomb fly-out. Reach and TTI integrate the same ticks as live flight.
fn bomb_time_to(
    w: &Weapon,
    position: Vector,
    velocity: Vector,
    point: Vector,
    age: u64,
    ground: Option<&dyn Fn(f64, f64) -> f64>,
) -> Option<f64> {
    use super::bomb_flight::{Body, DT, Flight};
    let body = Body::of(w);
    let mut flight = Flight { position, velocity };
    for tick in age..36_000 {
        let before = flight.position;
        flight.step(body, tick, Some(point));
        let p = flight.position;
        let floor = ground.map_or(point[1], |g| g(p[0], p[2]));
        // A designated object's aim point can stand above terrain. Stop at
        // that height when the path reaches the aim, rather than following
        // it through the object and treating the later ground miss as no TTI.
        if ground.is_some() && point[1] > floor && before[1] > point[1] && p[1] <= point[1] {
            let f = (before[1] - point[1]) / (before[1] - p[1]);
            let hit: Vector = std::array::from_fn(|i| before[i] + f * (p[i] - before[i]));
            if ground_range(hit, point) <= HIT_FT {
                return Some((tick - age) as f64 * DT + f * DT);
            }
        }
        if p[1] <= floor {
            let f = ((before[1] - floor) / (before[1] - p[1])).clamp(0., 1.);
            let hit: Vector = std::array::from_fn(|i| before[i] + f * (p[i] - before[i]));
            return (ground.is_some() || ground_range(hit, point) <= HIT_FT)
                .then_some((tick - age) as f64 * DT + f * DT);
        }
    }
    None
}

/// Horizontal distance between two points, feet.
pub fn ground_range(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// What the cockpit shows for a selected guided weapon: the advisory the
/// readiness slot carries (none of them stops a release).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advisory {
    /// A laser weapon with no spot to home on: it can still be released and
    /// may find one once, inside its nose cone (NO LASER).
    NoLaser,
    /// The plane's laser is wanted but masked: the spot is gone (MASK).
    Masked,
    /// The weapon cannot reach the point from here (OUT OF REACH).
    OutOfReach,
    /// A JDAM or AT-2 with nothing designated (NO TARGET); this one does
    /// stop the release, through the readiness.
    NoTarget,
}
impl Advisory {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoLaser => "NO LASER",
            Self::Masked => "MASK",
            Self::OutOfReach => "OUT OF REACH",
            Self::NoTarget => "NO TARGET",
        }
    }
}

/// The selected guided weapon's sight: where it would go and whether it can
/// get there (docs/spec/air-to-ground.md, "Guided bomb release range
/// indicator" and "Warnings"). Worked out every tick for the cockpit; never
/// read back by combat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidedSight {
    /// Shared bounded model result for powered weapons; None is the prior model.
    pub energy_intercept: Option<bool>,
    pub kind: Guidance,
    /// The point the weapon would home on: the laser spot (own or a
    /// buddy's), the JDAM's point or the AT-2's target.
    pub aim: Option<Vector>,
    /// The aim is a buddy's laser spot, not the plane's own.
    pub buddy: bool,
    /// From the plane to the aim, feet: along the ground and slant.
    pub ground_range_ft: Option<f64>,
    pub slant_range_ft: Option<f64>,
    /// The reach ahead along the ground track (a guided bomb's fly-out, a
    /// missile's zone 1).
    pub reach: Option<Reach>,
    /// The aim lies inside the release zone and the reach: IN RNG.
    pub in_reach: bool,
    pub advisory: Option<Advisory>,
    /// The level release T, for a guided bomb in LVL with an aim (see
    /// [`GuidedSight::release_range_ft`] for where the bar stands).
    pub level: Option<GuidedLevel>,
}

/// How far in from the reach's far edge the level T's bar stands, as a
/// share of the reach (opinionated, agent): two fifths. The far edge is where
/// the glide only just gets there at the end of its 60 s life, and a lower
/// release shortens the flight and the time the laser must hold; measured on
/// F-16C, F/A-18D and A-10 passes from 4,000 to 15,000 ft (the
/// air-to-ground baseline) a bar a fifth in already hit within 60 ft with
/// flights of 47 to 58 s, two fifths within 56 ft with 29 to 57 s.
pub const LEVEL_DEPTH: f64 = 0.4;
/// A held trigger releases at the bar only when the aim is within this many
/// radians of the ground track (10 degrees; opinionated, agent): the stem
/// says where to point, and the glide's own turn does the rest.
pub const LEVEL_STEER_RAD: f64 = 10. * std::f64::consts::PI / 180.;
/// How many ticks of ground travel past the bar a held trigger may still
/// release in, so a bar that falls between two ticks is not missed (as
/// [`super::delivery::RELEASE_WINDOW_TICKS`]).
pub const LEVEL_WINDOW_TICKS: f64 = super::delivery::RELEASE_WINDOW_TICKS;

/// What the level T needs of the aircraft beyond the sight's own numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidedLevel {
    /// The turn that puts the ground track through the aim, radians,
    /// positive right: the stem stands this far from the flight path marker.
    pub steering_rad: f64,
    /// Ground speed, feet a second.
    pub ground_speed_fps: f64,
}

impl GuidedSight {
    /// The ground range from the aircraft at which the T's bar stands:
    /// [`LEVEL_DEPTH`] of the reach in from its far edge.
    pub fn release_range_ft(&self) -> Option<f64> {
        let reach = self.reach?;
        Some(reach.maximum_ft - LEVEL_DEPTH * (reach.maximum_ft - reach.minimum_ft))
    }
    /// How much ground the aim still has to close before the bar: its ground
    /// range less the release range, negative once past the bar.
    pub fn along_ft(&self) -> Option<f64> {
        Some(self.ground_range_ft? - self.release_range_ft()?)
    }
    /// Seconds to the bar at the ground speed, assuming the track is held;
    /// negative once past it.
    pub fn release_seconds(&self) -> Option<f64> {
        let level = self.level?;
        let along = self.along_ft()?;
        Some(if level.ground_speed_fps > 1. {
            along / level.ground_speed_fps
        } else if along >= 0. {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        })
    }
    /// The aim is nearer than the reach's near edge, or has gone behind the
    /// aircraft (more than a right angle off the track): OVERFLY.
    pub fn overflown(&self) -> bool {
        match (self.reach, self.ground_range_ft, self.level) {
            (Some(reach), Some(range), Some(level)) => {
                range < reach.minimum_ft || level.steering_rad.abs() > std::f64::consts::FRAC_PI_2
            }
            _ => false,
        }
    }
    /// Where the release marker rides on the stem: 1 at the top while the bar
    /// is [`super::delivery::MARKER_SECONDS`] or more away, 0 on the bar.
    pub fn marker_fraction(&self) -> f64 {
        self.release_seconds()
            .map_or(0., |s| (s / super::delivery::MARKER_SECONDS).clamp(0., 1.))
    }
    /// More than [`super::delivery::DASHED_SECONDS`] out: the stem is dashed.
    pub fn far(&self) -> bool {
        self.release_seconds()
            .is_some_and(|s| s > super::delivery::DASHED_SECONDS)
    }
    /// The T holds the trigger: LVL, the aim not overflown, and the bar not
    /// passed by more than the window. Past the window the trigger is a plain
    /// press again.
    pub fn takes_trigger(&self) -> bool {
        let (Some(level), Some(along)) = (self.level, self.along_ft()) else {
            return false;
        };
        !self.overflown() && along > -LEVEL_WINDOW_TICKS * level.ground_speed_fps / 120.
    }
    /// A held trigger releases now: the bar is reached (within the window),
    /// the aim is inside the reach and within [`LEVEL_STEER_RAD`] of the
    /// track.
    pub fn due(&self) -> bool {
        self.takes_trigger()
            && self.in_reach
            && self.along_ft().is_some_and(|a| a <= 0.)
            && self
                .level
                .is_some_and(|l| l.steering_rad.abs() <= LEVEL_STEER_RAD)
    }
    /// While the T holds the trigger, whether a held trigger releases now;
    /// `None` when the trigger works as a plain press.
    pub fn level_trigger(&self) -> Option<bool> {
        self.takes_trigger().then(|| self.due())
    }
}

/// The turn from the ground track of `launcher` to `aim`, radians, positive
/// right (`None` when it has no track).
pub fn steering_to(launcher: &Launcher, aim: Vector) -> Option<f64> {
    let track = ground_track(launcher)?;
    let to = [aim[0] - launcher.position[0], aim[2] - launcher.position[2]];
    if to[0].hypot(to[1]) < 1. {
        return Some(0.);
    }
    let turn = to[0].atan2(to[1]) - track[0].atan2(track[1]);
    Some(turn.sin().atan2(turn.cos()))
}

#[cfg(test)]
#[path = "guided_tests.rs"]
mod tests;
