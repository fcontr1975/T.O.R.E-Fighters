//! The flight of the guided air-to-ground weapons an ownship releases: laser
//! bombs and missiles, the JDAMs and the AT-2 (docs/spec/air-to-ground.md,
//! "The laser", "Buddy lasing", "Guided bomb release range indicator" and
//! "Weapon classes"). Their physics and cones are [`crate::combat::guided`];
//! this module keeps each one's guidance state, steers it every tick and
//! works out the cockpit's sight for the selected one.
//!
//! These weapons have no missile [`super::missiles::Profile`]: they fly on a
//! velocity state with their own guidance here. Unpowered bombs advance on
//! `bomb_flight`; powered weapons advance on `Motion`. The legacy pursuit never sees
//! them (their projectile target stays empty).
//!
//! - **Laser** (retail lock, TORE spot): at release a spot must lie inside
//!   zone 1 about the aircraft (45 degrees, the record's range times the
//!   laser weather share); the plane's own spot first, else the nearest
//!   same-side spot (buddy lasing, opinionated, John, 2026-10-10). With no
//!   spot at all the weapon waits and may lock once onto a spot that comes
//!   inside its 45 degree nose cone (lead ruling 3, 2026-10-10); with a spot
//!   that is out of reach it leaves blind, as retail. In flight it follows its
//!   spot owner's laser (the spot while lasing, the laser's point while
//!   masked: weather and masking never break a lock already made, retail),
//!   and loses guidance for good when that laser stops or its point leaves
//!   the 45 degree cone. A new designation moves the laser, so retail's
//!   retargeting (follow if inside the cone, else blind) falls out of it.
//! - **Coordinate** (JDAM, opinionated, John, 2026-10-10, decision 4): the
//!   designated ground object's position, else the sight's held point, is
//!   stored at release if it lies inside zone 1 (no weather); the bomb flies
//!   to that point whatever happens after. Outside zone 1 it falls blind.
//! - **Command** (AT-2): the designated object, inside zone 1 at release with
//!   a clear line from the launcher. It is steered at the object while the
//!   launcher lives, the object stands, the object stays inside the missile's
//!   45 degree nose cone and inside the launcher's zone-1 cone with a clear
//!   line (the launcher's sight, lead ruling 1); any loss is permanent.
//!
//! A guided bomb that is not guiding (waiting or blind) falls on the bomb
//! equation from where it is (fitted); a missile flies straight on.
use super::cockpit::{Cockpits, StrikeCockpit};
use super::{Configuration, Launcher, NO_SIDE, OwnRow, Ownship, Projectile, State, Target};
use crate::attitude::{Vector, dot, unit};
use crate::combat::ballistics::{BombMotion, FallCursors, ReleaseState};
use crate::combat::delivery::DeliveryMode;
use crate::combat::guided::{self, Advisory, GuidedLevel, GuidedSight, Reach};
use crate::combat::laser::{self, LaserSpot, LaserState};
use crate::combat::missiles::{self, Guidance};
use crate::combat::sight;
use crate::sensors::Weather;
use tore_formats::weapons::Weapon;

/// Where a guided weapon's guidance stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A laser weapon released with no spot: it may lock once.
    Waiting,
    /// Homing.
    Guiding,
    /// Guidance lost for good, or never had.
    Blind,
}

/// One guided weapon in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuidedFlight {
    pub kind: Guidance,
    pub mode: Mode,
    /// The aircraft that released it, and from which station.
    pub owner: u32,
    pub station: usize,
    /// The aircraft whose laser a laser weapon follows.
    pub spot_owner: Option<u32>,
    /// The point it steers for: the JDAM's stored point, or the last point a
    /// laser or command weapon homed on.
    pub point: Option<Vector>,
    /// The object an AT-2 is steered at.
    pub target: Option<u32>,
    /// The bomb equation of a guided bomb that is not guiding, and the age
    /// (ticks) its path starts at.
    pub fall: Option<(BombMotion, u64)>,
    /// Seconds to impact at the last step, when it can be worked out.
    pub seconds: Option<f64>,
}

impl GuidedFlight {
    /// Whether it is homing now.
    pub fn guiding(&self) -> bool {
        self.mode == Mode::Guiding
    }
}

/// Every laser spot this tick: the ownships', then those of the aircraft the
/// AI flies with a human's laser kept on ([`super::Kept`]), then the AI
/// strikers', each in aircraft order ([`Cockpits`]).
pub(super) fn spots(cockpits: Cockpits<'_>) -> Vec<LaserSpot> {
    cockpits
        .iter()
        .filter_map(|(aircraft, side, cockpit)| cockpit.laser.spot(aircraft, side))
        .collect()
}

/// The spot `aircraft` (of `side`) would use among `spots` that `accepts`:
/// its own first, else the nearest same-side spot to `from`, then the lower
/// owner id (as `State::laser_spot_for`). Never a spot of the other side,
/// and no buddy for an aircraft with no side.
fn choose(
    spots: &[LaserSpot],
    aircraft: u32,
    side: super::Side,
    from: Vector,
    accepts: impl Fn(&LaserSpot) -> bool,
) -> Option<LaserSpot> {
    if let Some(own) = spots.iter().find(|s| s.owner == aircraft && accepts(s)) {
        return Some(*own);
    }
    if side == NO_SIDE {
        return None;
    }
    spots
        .iter()
        .filter(|s| s.owner != aircraft && s.side == side && accepts(s))
        .map(|s| (distance(s.position, from), *s))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.owner.cmp(&b.1.owner)))
        .map(|(_, s)| s)
}

/// The point an aircraft's laser marks now for a weapon that follows it: the
/// spot while lasing, the laser's point while masked; nothing otherwise. The
/// laser is its strike cockpit's: the ownship's, the one its human left while
/// the AI flies it, or the AI striker's.
fn followed(cockpits: Cockpits<'_>, owner: u32) -> Option<Vector> {
    let laser = &cockpits.get(owner)?.1.laser;
    match laser.state {
        LaserState::Lasing | LaserState::Masked => laser.point,
        LaserState::Off | LaserState::Ready => None,
    }
}

/// A target row by id: a target or another ownship's row.
fn row<'a>(targets: &'a [Target], rows: &'a [OwnRow], id: u32) -> Option<&'a Target> {
    targets
        .iter()
        .find(|t| t.id == id)
        .or_else(|| rows.iter().map(|r| &r.target).find(|t| t.id == id))
}

/// The point a JDAM would store or an AT-2 would steer at, for a human's
/// cockpit between steps: the designated object (on the ground and
/// standing), else for a JDAM the sight's held point
/// ([`StrikeCockpit::aim_point`] with the sensors' designation).
pub(super) fn designated_point(
    own: &Ownship,
    kind: Guidance,
    targets: &[Target],
) -> Option<(Vector, u32)> {
    super::cockpit::aim_point(
        own.designated().map(super::cockpit::StrikeTarget::Object),
        own.sight.as_ref(),
        kind,
        targets,
    )
}

/// Who releases a guided weapon: the aircraft, its side and its cockpit.
pub(super) struct Releaser<'a> {
    pub aircraft: u32,
    pub side: super::Side,
    pub cockpit: &'a StrikeCockpit,
}

/// The laser weather share on the line from `from` to `to` (retail selector
/// 1, at release only).
fn laser_scale(weather: &Weather, from: Vector, to: Vector) -> f64 {
    laser::weather_scale(weather, from[1], to[1])
}

/// What a release starts: the flight, and the fall of a guided bomb that
/// leaves without guidance (`fall_velocity`: its release velocity with a
/// dumb bomb's scatter).
#[allow(clippy::too_many_arguments)]
pub(super) fn release<G: Fn(f64, f64) -> f64>(
    w: &Weapon,
    kind: Guidance,
    own: Releaser<'_>,
    station: usize,
    launcher: &Launcher,
    position: Vector,
    fall_velocity: Vector,
    spots: &[LaserSpot],
    targets: &[Target],
    weather: &Weather,
    ground: &G,
) -> GuidedFlight {
    let mut flight = GuidedFlight {
        kind,
        mode: Mode::Blind,
        owner: own.aircraft,
        station,
        spot_owner: None,
        point: None,
        target: None,
        fall: None,
        seconds: None,
    };
    let zone = |point: Vector, scale: f64| {
        guided::in_release_zone(w, launcher.position, launcher.basis, point, scale)
    };
    match kind {
        Guidance::Laser => {
            // The plane's own spot is this tick's; the others' are the
            // snapshot from before the ownships stepped.
            let mut all: Vec<LaserSpot> = spots
                .iter()
                .filter(|s| s.owner != own.aircraft)
                .copied()
                .collect();
            all.extend(own.cockpit.laser.spot(own.aircraft, own.side));
            let usable = choose(&all, own.aircraft, own.side, launcher.position, |s| {
                zone(
                    s.position,
                    laser_scale(weather, launcher.position, s.position),
                ) && missiles::launch_line_clear(launcher.position, s.position, ground)
            });
            if let Some(spot) = usable {
                flight.mode = Mode::Guiding;
                flight.spot_owner = Some(spot.owner);
                flight.point = Some(spot.position);
            } else if choose(&all, own.aircraft, own.side, launcher.position, |_| true).is_none() {
                flight.mode = Mode::Waiting;
            }
        }
        Guidance::Coordinate => {
            if let Some((point, id)) = own.cockpit.aim_point(kind, targets)
                && zone(point, 1.)
            {
                flight.mode = Mode::Guiding;
                flight.point = Some(point);
                flight.target = (id != u32::MAX).then_some(id);
            }
        }
        Guidance::Command => {
            if let Some((point, id)) = own.cockpit.aim_point(kind, targets)
                && zone(point, 1.)
                && missiles::launch_line_clear(launcher.position, point, ground)
            {
                flight.mode = Mode::Guiding;
                flight.point = Some(point);
                flight.target = Some(id);
            }
        }
        Guidance::Supported | Guidance::Active | Guidance::Infrared | Guidance::Emitter => {}
    }
    // A glide bomb let go with no spot falls like a dumb bomb, with its own
    // scatter (slice ST-G: `fall_velocity` is the release velocity turned by
    // it; a bomb that guides keeps its straight launch).
    if flight.mode != Mode::Guiding && guided::is_glide_bomb(w) {
        flight.fall = Some((
            BombMotion::new(ReleaseState {
                body: crate::combat::bomb_flight::Body::of(w),
                position,
                velocity: fall_velocity,
            }),
            0,
        ));
    }
    flight
}

/// How often a guiding cruise-profile weapon works out its time to impact
/// again, ticks (8 a second); it counts down in between. Agent design: the
/// HUD shows it to a tenth of a second.
pub const SECONDS_REFRESH_TICKS: u64 = 15;

/// What one guided step reads besides the projectile.
pub(super) struct Context<'a, G> {
    pub ships: &'a [Ownship],
    /// Every strike cockpit: the lasers weapons follow and their sides.
    pub cockpits: Cockpits<'a>,
    pub rows: &'a [OwnRow],
    pub targets: &'a [Target],
    pub ground: &'a G,
}

/// One tick of a guided weapon's guidance, before its motion: steers a
/// guiding weapon (turning its motion's velocity with it) and switches
/// modes. Returns the fall a guided bomb now takes, if it is not guiding;
/// the caller moves the projectile along it instead of its motion.
pub(super) fn steer<G: Fn(f64, f64) -> f64>(
    flight: &mut GuidedFlight,
    p: &mut Projectile,
    w: &Weapon,
    cx: &Context<'_, G>,
    falls: &mut FallCursors,
) -> Option<(BombMotion, u64)> {
    let glide = guided::is_glide_bomb(w);
    let was_guiding = flight.mode == Mode::Guiding;
    if p.motion
        .and_then(|m| m.energy)
        .is_some_and(|e| e.flight.lost || p.age >= u64::from(e.body.life))
    {
        flight.mode = Mode::Blind;
        flight.seconds = None;
    }
    if glide && p.age >= u64::from(w.movement.remove_t) * 30 && flight.mode != Mode::Blind {
        flight.mode = Mode::Blind;
        if let Some(motion) = p.motion {
            flight.fall = Some((
                BombMotion::new(ReleaseState {
                    position: p.position,
                    velocity: motion.velocity,
                    body: crate::combat::bomb_flight::Body::of(w),
                }),
                p.age,
            ));
        }
    }

    // Which way the weapon points now: a falling bomb along its fall.
    if glide
        && p.age == 0
        && let Some((fall, _)) = flight.fall
        && let Some(motion) = p.motion.as_mut()
    {
        motion.velocity = fall.velocity(0.);
        p.direction = unit(motion.velocity);
    }
    let heading = p.direction;
    let nose = guided::nose(heading);
    match flight.mode {
        Mode::Waiting => {
            let spots = spots(cx.cockpits);
            let side = cx
                .cockpits
                .get(flight.owner)
                .map_or(NO_SIDE, |(side, _)| side);
            if let Some(spot) = choose(&spots, flight.owner, side, p.position, |s| {
                guided::in_cone(w, p.position, nose, s.position)
                    && missiles::launch_line_clear(p.position, s.position, cx.ground)
            }) {
                flight.mode = Mode::Guiding;
                flight.spot_owner = Some(spot.owner);
                flight.point = Some(spot.position);
                if let (Some((fall, start)), Some(motion)) = (flight.fall.take(), p.motion.as_mut())
                {
                    motion.velocity = fall.velocity((p.age - start) as f64 / 120.);
                    p.direction = unit(motion.velocity);
                }
            }
        }
        Mode::Guiding => {
            let aim = match flight.kind {
                Guidance::Laser => flight
                    .spot_owner
                    .and_then(|owner| followed(cx.cockpits, owner))
                    .filter(|point| guided::in_cone(w, p.position, nose, *point)),
                Guidance::Coordinate => flight.point,
                Guidance::Command => command_aim(flight, p, w, nose, cx),
                Guidance::Supported | Guidance::Active | Guidance::Infrared | Guidance::Emitter => {
                    None
                }
            };
            match aim {
                Some(point) => flight.point = Some(point),
                None => {
                    flight.mode = Mode::Blind;
                    if glide && let Some(motion) = p.motion {
                        flight.fall = Some((
                            BombMotion::new(ReleaseState {
                                position: p.position,
                                velocity: motion.velocity,
                                body: crate::combat::bomb_flight::Body::of(w),
                            }),
                            p.age,
                        ));
                    }
                }
            }
        }
        Mode::Blind => {}
    }
    if flight.mode == Mode::Guiding
        && let (Some(point), Some(motion)) = (flight.point, p.motion.as_mut())
    {
        if !glide {
            if motion.energy.is_some() {
                let velocity = flight
                    .target
                    .and_then(|id| row(cx.targets, cx.rows, id))
                    .map_or([0.; 3], |t| t.velocity);
                motion.track(point, velocity);
            } else {
                let aim = motion.aim_over_terrain(p.position, point, cx.ground);
                let desired = missiles::commanded_heading(
                    p.direction,
                    motion.path(),
                    unit(missiles::sub(aim, p.position)),
                );
                let next = missiles::steer(&w.movement, p.age, p.direction, desired);
                motion.turn(p.direction, next);
                p.direction = next;
            }
        }
        // A cruise profile's fly-out samples the terrain on every step to
        // impact, so it is worked out on the first guiding tick and then
        // every SECONDS_REFRESH_TICKS (staggered by projectile id); between
        // those it counts down. Other weapons fly it every tick, as before
        // (review H1: every tick cost the square of the flight time).
        let refresh = !was_guiding
            || (!glide && motion.cruise.is_none())
            || p.age == 0
            || (p.age + u64::from(p.id)).is_multiple_of(SECONDS_REFRESH_TICKS);
        flight.seconds = if motion.energy.is_some() {
            flight.seconds.map(|s| (s - missiles::DT).max(0.))
        } else if refresh {
            guided::time_to_with_terrain(
                w,
                *motion,
                p.position,
                p.direction,
                point,
                p.age,
                Some(cx.ground),
            )
        } else {
            flight.seconds.map(|s| (s - missiles::DT).max(0.))
        };
    } else if glide && let Some(motion) = p.motion {
        if p.age == 0 || (p.age + u64::from(p.id)).is_multiple_of(SECONDS_REFRESH_TICKS) {
            flight.seconds = crate::combat::ballistics::predict_impact(
                ReleaseState {
                    position: p.position,
                    velocity: motion.velocity,
                    body: crate::combat::bomb_flight::Body::of(w),
                },
                cx.ground,
            )
            .map(|i| i.seconds);
        } else {
            flight.seconds = flight.seconds.map(|s| (s - missiles::DT).max(0.));
        }
    } else if let Some((fall, start)) = flight.fall {
        let elapsed = (p.age - start) as f64 / 120.;
        // The fall's cursor gives the same answers as the fall itself
        // without flying it again from its start each tick.
        let cursor = falls.follow(p.id, &fall);
        let here = cursor.at(&fall, elapsed).position;
        flight.seconds = cursor
            .fall_time(&fall, (cx.ground)(here[0], here[2]))
            .map(|t| (t - elapsed).max(0.));
    } else {
        flight.seconds = None;
    }
    if glide { None } else { flight.fall }
}

/// An AT-2's target now, if it still guides: the launcher alive, the target
/// standing, inside the missile's nose cone and inside the launcher's zone-1
/// cone with a clear line.
fn command_aim<G: Fn(f64, f64) -> f64>(
    flight: &GuidedFlight,
    p: &Projectile,
    w: &Weapon,
    nose: crate::attitude::Basis,
    cx: &Context<'_, G>,
) -> Option<Vector> {
    // The launcher is the ownship's, or the AI-flown row of an aircraft
    // whose human left it (slice FX-J) or of an AI striker: the AI flying
    // it keeps the line only as long as its flying does.
    let launcher = match cx.rows.iter().find(|r| r.target.id == flight.owner) {
        Some(row) => {
            cx.ships
                .iter()
                .find(|own| own.aircraft == flight.owner)
                .filter(|own| own.hp > 0)?;
            row.launcher
        }
        None if cx.cockpits.flown_by_ai(flight.owner) => cx
            .targets
            .iter()
            .find(|t| t.id == flight.owner)
            .map(super::sighting::row_launcher)?,
        None => return None,
    };
    if !launcher.alive {
        return None;
    }
    let target = row(cx.targets, cx.rows, flight.target?).filter(|t| t.hp > 0)?;
    (guided::in_cone(w, p.position, nose, target.position)
        && guided::in_cone(w, launcher.position, launcher.basis, target.position)
        && sight::clear_line(launcher.position, target.position, cx.ground))
    .then_some(target.position)
}

/// What the cockpit sight reads besides the cockpit.
pub(super) struct SightContext<'a, G> {
    /// Every spot (the other ownships' from the snapshot before they
    /// stepped).
    pub spots: &'a [LaserSpot],
    pub targets: &'a [Target],
    pub weather: &'a Weather,
    pub ground: &'a G,
}

/// The selected guided weapon's sight, when one is selected and armed: its
/// aim, range, reach and advisory (docs/spec/air-to-ground.md, "Guided bomb
/// release range indicator" and "Warnings"). The same for a human's cockpit
/// and an AI striker's.
pub(super) fn sight<G: Fn(f64, f64) -> f64>(
    aircraft: u32,
    side: super::Side,
    own: &StrikeCockpit,
    config: &Configuration,
    launcher: &Launcher,
    cx: &SightContext<'_, G>,
) -> Option<GuidedSight> {
    let w = own.armed_weapon(config)?;
    let kind = guided::kind(w)?;
    let (aim, buddy, mut advisory) = match kind {
        Guidance::Laser => {
            let mut all: Vec<LaserSpot> = cx
                .spots
                .iter()
                .filter(|s| s.owner != aircraft)
                .copied()
                .collect();
            all.extend(own.laser.spot(aircraft, side));
            if let Some(spot) = choose(&all, aircraft, side, launcher.position, |s| {
                missiles::launch_line_clear(launcher.position, s.position, cx.ground)
            }) {
                (Some(spot.position), spot.owner != aircraft, None)
            } else if let Some(spot) = choose(&all, aircraft, side, launcher.position, |_| true) {
                (
                    Some(spot.position),
                    spot.owner != aircraft,
                    Some(Advisory::Masked),
                )
            } else if own.laser.state == LaserState::Masked {
                (own.laser.point, false, Some(Advisory::Masked))
            } else {
                let object = own.aim_point(Guidance::Command, cx.targets);
                (object.map(|(p, _)| p), false, Some(Advisory::NoLaser))
            }
        }
        Guidance::Coordinate | Guidance::Command => {
            let aim = own.aim_point(kind, cx.targets).map(|(p, _)| p);
            let advisory = if aim.is_none() {
                Some(Advisory::NoTarget)
            } else if kind == Guidance::Command
                && aim
                    .is_some_and(|p| !missiles::launch_line_clear(launcher.position, p, cx.ground))
            {
                Some(Advisory::Masked)
            } else {
                None
            };
            (aim, false, advisory)
        }
        Guidance::Supported | Guidance::Active | Guidance::Infrared | Guidance::Emitter => {
            return None;
        }
    };
    let ground = aim.map_or_else(
        || (cx.ground)(launcher.position[0], launcher.position[2]),
        |point| point[1],
    );
    let scale = match (kind, aim) {
        (Guidance::Laser, Some(point)) => laser_scale(cx.weather, launcher.position, point),
        (Guidance::Laser, None) => laser_scale(
            cx.weather,
            launcher.position,
            [launcher.position[0], ground, launcher.position[2]],
        ),
        _ => 1.,
    };
    let reach = guided::reach(w, launcher, ground, scale).or_else(|| {
        // A missile's reach is its zone (fitted); with no ground ahead in
        // it the scale still shows the zone.
        (!guided::is_glide_bomb(w)).then(|| Reach {
            minimum_ft: f64::from(w.seeker.zones[1].minimum_range.max(0)),
            maximum_ft: guided::zone_range(w, scale),
        })
    });
    let ground_range = aim.map(|point| guided::ground_range(point, launcher.position));
    let slant_range = aim.map(|point| distance(point, launcher.position));
    // A missing spot cannot supply an aim. MASK is a separate advisory,
    // never a terrain penalty on the range/energy cue.
    let spotless = matches!(advisory, Some(Advisory::NoLaser | Advisory::Masked));
    let in_reach = advisory != Some(Advisory::NoLaser)
        && aim.is_some_and(|point| {
            guided::in_release_zone(w, launcher.position, launcher.basis, point, scale)
                && (!guided::is_glide_bomb(w) || {
                    let motion = missiles::Motion::launch(
                        w,
                        guided::release_velocity(w, launcher),
                        point[1],
                    );
                    guided::time_to(
                        w,
                        motion,
                        launcher.position,
                        unit(guided::release_velocity(w, launcher)),
                        point,
                        0,
                    )
                    .is_some()
                })
        });
    if advisory.is_none() && aim.is_some() && !in_reach {
        advisory = Some(Advisory::OutOfReach);
    }
    // The level T: a guided bomb in LVL with a real aim (a laser bomb needs
    // a spot to home on) and a reach to stand the bar in.
    let level = (own.delivery_mode == DeliveryMode::Level
        && guided::is_glide_bomb(w)
        && !spotless
        && reach.is_some()
        && ground_range.is_some())
    .then(|| {
        let steering_rad = guided::steering_to(launcher, aim?)?;
        Some(GuidedLevel {
            steering_rad,
            ground_speed_fps: launcher.velocity[0].hypot(launcher.velocity[2]),
        })
    })
    .flatten();
    Some(GuidedSight {
        energy_intercept: None,
        kind,
        aim,
        buddy,
        ground_range_ft: ground_range,
        slant_range_ft: slant_range,
        reach,
        in_reach,
        advisory,
        level,
    })
}

impl State {
    /// The guidance of guided air-to-ground weapon `id` in flight.
    pub fn guided_flight(&self, id: u32) -> Option<&GuidedFlight> {
        self.guided.get(&id)
    }
    /// Every guided air-to-ground weapon in flight, by projectile number
    /// (oldest first).
    pub fn guided_flights(&self) -> impl Iterator<Item = (u32, &GuidedFlight)> {
        self.guided.iter().map(|(id, flight)| (*id, flight))
    }
    /// The tick `aircraft`'s last guided air-to-ground weapon ended, if one
    /// has.
    pub fn guided_impact(&self, aircraft: u32) -> Option<u64> {
        self.guided_impacts.get(&aircraft).copied()
    }
}

fn distance(a: Vector, b: Vector) -> f64 {
    let d = missiles::sub(a, b);
    dot(d, d).sqrt()
}

// Exact checkpoints (docs/formats/checkpoint.md).
crate::checkpoint_enum!(Mode {
    Waiting = 0,
    Guiding = 1,
    Blind = 2,
});
crate::checkpoint_struct!(GuidedFlight {
    kind,
    mode,
    owner,
    station,
    spot_owner,
    point,
    target,
    fall,
    seconds,
});
