//! The helicopter family of the strike tactics (docs/spec/ai-strike.md,
//! "Helicopters"; slice ST-F): the AH-64 and Mi-24 attack with retail's
//! helicopter pop-up, or, with a stand-off weapon under threat Missiles,
//! from low at the weapon's range.
//!
//! - **Pop-up** (retail shape, docs/spec/ai.md B20): toward a
//!   point 1,500 ft above the target until 10,000 ft out, pull 30 degrees
//!   over the line of sight for 6 s, then point at the target until 1,500 ft
//!   or it is no longer ahead. Retail's pull asks for minimum speed by an
//!   operand slip; T.O.R.E keeps its speed through it (John, 2026-10-11:
//!   "maintain proper speed for defense and quick exfil"), and every leg
//!   asks for the helicopter's strike speed, 80% of its maximum (John,
//!   2026-10-13: "about 120 kt"; [`crate::ai::helicopter_defense::strike_speed`]).
//!   Until slice HF1 every leg asked for the maximum.
//! - **Stand-off** ([`Pass::StandOff`], fitted, agent): with an AT-2 (or any
//!   guided or seeker store) under threat Missiles, it runs in low and
//!   masked (200 ft above the ground) to 60% of the weapon's zone-1 range
//!   (about where the guided T bar stands), then unmasks: climbs 500 ft for
//!   at most 10 s while the shot comes up,
//!   fires, holds the line, and masks again turning away. It does not stop
//!   to hover: the AI's input-only adapter cannot hold a helicopter at a
//!   hover (asked for a low speed it settles nose-down into the ground), so
//!   the unmask is flown at speed, which also keeps the helicopter's speed
//!   for its defence (John, 2026-10-11).
//! - **Move-ahead ambush** ([`crate::ai::helicopter_ahead`]; T.O.R.E
//!   opinionated change, John 2026-10-29): on a moving target under known
//!   rings, the stand-off is flown from a firing point ahead of the
//!   target's path, where it will be in range; the helicopter circles
//!   masked out of the rings until it is time, then comes in along the
//!   firing radial, unmasks and fires. A static target, or one with no such
//!   point, is attacked as before.
//! - **Rockets** fly the pop-up's final like a fighter's rocket run: the cue
//!   is flown onto the pass's drawn aim point and up to eight leave at the
//!   record's cadence while it stays inside the skill circle.
//! - **The AT-2** leaves when the shared release test passes (the target in
//!   the launcher's 45 degree cone with a clear line), and the helicopter
//!   then holds its nose on the target until the missile has hit or is gone
//!   (the launcher sight rule: command guidance needs the target in the
//!   launcher's cone every tick).
//! - **Exit**: retail's is 5 degrees up at maximum speed straight ahead,
//!   levelling at the pass's height, to max(20,000 ft, 2R) from the target,
//!   then turn back and choose again. A helicopter flying it after a
//!   pop-up crossed the target area at 1,800 ft, where a site it had not
//!   seen shot an AH-64 down (slice HF1). T.O.R.E's helicopter leaves the
//!   target area instead, always, threat or not (opinionated, John
//!   2026-10-13: "the helicopter leaves after its run", and no straight-on
//!   exit even with no threat near; the shape is fitted, agent): it turns away from
//!   the target, round every ring it knows and the ambush ring of a threat
//!   that engaged it, and masks at 200 ft over the highest ground ahead at
//!   its strike speed, to the same distance, then turns back and chooses
//!   again.
//!
//! Defence, the safe test and going home are the fighters'.
use super::{
    CHOICE_DISTANCE_FT, Context, Handling, Order, Pass, Phase, ROCKETS_PER_PASS, Release,
    STRAIGHT_EXIT_FT, Segment, Solution, Steer, Store, StoreKind, Strike, TURN_BACK_DEG, aim_draw,
    angle_difference, bearing_deg, depression_deg, elevation_deg, hold_pitch_deg, horizontal,
    known_position, objective_point, release_circle_ft, standing, steering_frame, turn_radius_ft,
};
use crate::ai::awareness::ground::{BriefedKind, ThreatLevel};
use crate::ai::controller::{Activity, OwnState};
use crate::ai::helicopter_ahead as ahead;
use crate::ai::helicopter_threat::{self as threat, Decision};
use crate::ai::motion::SpeedRequest;
use crate::attitude::Vector;

/// Pop-up (retail `H.AI`): the approach point's height over the target,
/// where the pull starts, its angle over the line of sight, its length, and
/// where the run ends.
pub const POPUP_OFFSET_FT: f64 = 1_500.;
pub const POPUP_PULL_AT_FT: f64 = 10_000.;
pub const POPUP_PULL_DEG: f64 = 30.;
pub const POPUP_PULL_TICKS: u64 = 6 * 120;
pub const POPUP_EXIT_FT: f64 = 1_500.;
/// Exit (retail): the distance the turn back waits for. (Retail's 5 degree
/// climb is not flown since slice HF1: the helicopter masks.)
pub const EXIT_TURN_MIN_FT: f64 = 20_000.;
/// Pitch requests are held within this (fitted, agent: a helicopter flies
/// its pass by its flight path, not by an 80 degree fighter dive).
pub const PITCH_LIMIT_DEG: f64 = 30.;
/// The steepest dive flown at the cue, and the height above the ground at
/// which the run ends whatever the cue says (fitted, agent: the AI's
/// adapter cannot pull a helicopter out of a steep dive near the ground).
pub const DIVE_LIMIT_DEG: f64 = 20.;
pub const RUN_FLOOR_AGL_FT: f64 = 500.;
/// Flying the rocket cue: the pitch and heading gains (fitted, agent).
pub const CUE_PITCH_GAIN: f64 = 3.;
pub const CUE_TURN_GAIN: f64 = 3.;
/// A helicopter releases its rockets once the cue is within this share of
/// its skill circle of the drawn aim (fitted, agent; a fighter's is 5%): its
/// slow sweep and its adapter hold the cue to about this.
pub const GATE_SHARE: f64 = 0.25;
/// The least a helicopter's rocket gate may be, feet (fitted, agent,
/// ST-INT): with John's tighter circles (Ace 33 ft) a quarter of the circle
/// is 8 to 29 ft at skills 3 and 2, and the AH-64's cue never came that
/// close in a 150 s pass; 30 ft released at both skills.
pub const GATE_FLOOR_FT: f64 = 40.;
/// Stand-off attack (fitted, agent): the share of the weapon's zone-1
/// range it unmasks at (the guided T bar stands 40% of the reach in from
/// its far edge, docs/spec/air-to-ground.md), the masked run's height above the ground, the
/// unmask's climb and its longest wait, and how far beyond the firing range
/// the masked exit goes before turning back.
pub const STANDOFF_SHARE: f64 = 0.6;
pub const MASK_AGL_FT: f64 = crate::ai::helicopter_defense::MASK_AGL_FT;
pub const UNMASK_CLIMB_FT: f64 = 500.;
pub const UNMASK_TICKS: u64 = 10 * 120;
pub const STANDOFF_EXIT_FT: f64 = 6_000.;
/// A guided round's flight is watched for this long after release before
/// "none in flight" ends the hold (the host sees it the next step).
pub const GUIDE_SETTLE_TICKS: u64 = 60;

/// The helicopter's pass (spec, "Helicopters"): the stand-off attack for a
/// guided or seeker store under threat Missiles, else the pop-up.
pub fn choose_pass(kind: StoreKind, threat: ThreatLevel) -> Pass {
    let stand_off = matches!(
        kind,
        StoreKind::Command | StoreKind::Seeker | StoreKind::RadarAgm | StoreKind::Laser
    );
    if stand_off && threat == ThreatLevel::Missiles {
        Pass::StandOff
    } else {
        Pass::PopUp
    }
}

/// The helicopter's store: the guided weapons first (they reach farthest),
/// then rockets (fitted, agent). Guns and bombs are not chosen.
pub fn choose_store(stores: &[Store], designator: bool) -> Option<Store> {
    use StoreKind as K;
    [K::Command, K::Seeker, K::RadarAgm, K::Laser, K::Rocket]
        .iter()
        .filter(|k| **k != K::Laser || designator)
        .find_map(|kind| {
            stores
                .iter()
                .find(|s| s.kind == *kind && s.rounds > 0)
                .copied()
        })
}

/// The heading and flight-path pitch that fly the rocket cue onto the aim
/// (fitted, agent): the flight path turned by [`CUE_PITCH_GAIN`] times the
/// angle between the aim and the cue, and the heading by [`CUE_TURN_GAIN`]
/// times the cross-track angle, continuously. A helicopter's cue moves
/// slowly and does not creep across the aim in a held dive as a fighter's
/// does, so it is flown all the way in.
pub fn fly_cue(own: &OwnState, velocity: Vector, aim: Vector, impact: Vector) -> (f64, f64) {
    let turn = elevation_deg(own.position, aim) - elevation_deg(own.position, impact);
    let pitch = (steering_frame(own.position, aim).1 + CUE_PITCH_GAIN * turn)
        .clamp(-DIVE_LIMIT_DEG, PITCH_LIMIT_DEG);
    let norm = velocity[0].hypot(velocity[2]);
    let heading = if norm > 1. {
        let u = [velocity[0] / norm, velocity[2] / norm];
        let cross = (aim[0] - impact[0]) * u[1] - (aim[2] - impact[2]) * u[0];
        let reach = horizontal(own.position, aim).max(500.);
        let correction = (CUE_TURN_GAIN * cross.atan2(reach).to_degrees()).clamp(-20., 20.);
        (own.heading_deg + correction).rem_euclid(360.)
    } else {
        bearing_deg(own.position, aim)
    };
    (heading, pitch)
}

/// Whether a store guides after release and so is held to impact.
fn guided(kind: StoreKind) -> bool {
    !kind.unguided()
}

/// One step of a helicopter (see the module notes).
pub(super) fn step(s: &mut Strike, cx: &Context<'_>) -> Order {
    let own = cx.own;
    let order = Order {
        steer: None,
        release: None,
        activity: None,
    };
    // A helicopter flies no lasing hold (slice ST-L's hold is the fixed-wing
    // tactics'; `AiWings::can_lase` gives it no duty): a lase is dropped.
    if s.phase == Phase::Lase || s.lase.is_some() {
        s.lase = None;
        if s.phase == Phase::Lase {
            s.enter(Phase::Ingress, cx.tick);
        }
    }
    track_ambush(s, cx);
    if cx.defending || s.threatened(cx) {
        if s.phase != Phase::Defend {
            s.enter(Phase::Defend, cx.tick);
            s.released = 0;
        }
        return mask_run(cx, order, &threat::known_rings(cx.picture, cx.side));
    }
    if s.phase == Phase::Defend {
        if !s.quiet(cx) {
            return mask_run(cx, order, &threat::known_rings(cx.picture, cx.side));
        }
        // The threat has gone quiet: back to the ingress, where the rings it
        // knows (and the ring it was ambushed in) decide whether it attacks
        // or holds off (slice H4), in place of the fighters' safe test.
        s.enter(Phase::Ingress, cx.tick);
    }
    let solution = s.solution(cx);
    let target_at = solution
        .map(|sol| sol.target_position)
        .or_else(|| s.target.and_then(|id| known_position(cx.picture, id)));
    let target_alive = match (s.target, solution) {
        (Some(_), Some(sol)) => sol.target_alive,
        (Some(id), None) => standing(cx.picture, id),
        (None, _) => false,
    };
    if s.phase == Phase::Attack && !target_alive {
        exit(s, cx);
    }
    // A pass not yet fired that a ring now blocks is given up for the
    // hold-off (slice H4): it does not press an attack through a threat. A
    // ring learned after the choice that blocks the pass in hand (a pop-up
    // whose target it now covers) blocks it even when another attack would
    // stand clear: the hold-off chooses that attack at once.
    if s.phase == Phase::Attack
        && s.released == 0
        && s.segment == Segment::Approach
        && s.forced.is_none()
        && let (Some(target), Some(point)) = (s.target, target_at)
    {
        match s.heli.ahead {
            // A move-ahead ambush (John, 2026-10-29) is planned again when
            // the target stops or turns off its path, or a ring comes over
            // the plan: the choice finds the next one, or the stand-off,
            // or the hold-off.
            Some(plan) if cx.tick >= plan.planned_tick + ahead::REPLAN_TICKS => {
                match ahead_holds(s, cx, &plan, target, point) {
                    // The nose's offset may change with the target's course.
                    Some(offset_deg) => s.heli.ahead = Some(ahead::Ahead { offset_deg, ..plan }),
                    None => choose(s, cx),
                }
            }
            Some(_) => {}
            // A stand-off on a moving target, flown from the choice before
            // its motion was known, looks for a move-ahead once a second
            // and takes it.
            None => {
                let search = s.pass == Some(Pass::StandOff) && ahead::search_due(cx.tick);
                match plan(s, cx, target, point, search) {
                    None => hold_off(s, cx, target),
                    Some(threat::Plan::Ahead(_)) => choose(s, cx),
                    Some(_) if !pass_holds(s, cx, point) => hold_off(s, cx, target),
                    Some(_) => {}
                }
            }
        }
    }
    match s.phase {
        Phase::Ingress | Phase::Wait => {
            let point = target_at.or_else(|| objective_point(cx.picture));
            let near = point.is_none_or(|p| horizontal(own.position, p) <= CHOICE_DISTANCE_FT);
            // A ring across the way in makes it choose (and hold off) now.
            let ringed = point.is_some_and(|p| !threat::clear(&rings(s, cx), own.position, p));
            let waited =
                s.phase != Phase::Wait || cx.tick.saturating_sub(s.since) >= super::WAIT_TICKS;
            if (near || ringed) && waited {
                choose(s, cx);
            }
        }
        Phase::HoldOff => hold_check(s, cx),
        Phase::TurnBack => match target_at {
            Some(point)
                if angle_difference(own.heading_deg, bearing_deg(own.position, point)).abs()
                    > TURN_BACK_DEG
                    && threat::clear(&rings(s, cx), own.position, point) => {}
            _ => choose(s, cx),
        },
        Phase::Exit => {
            if let Some(point) = target_at
                && horizontal(own.position, point) >= exit_distance(s, cx)
            {
                if !s.withdrawn {
                    s.enter(Phase::TurnBack, cx.tick);
                } else if s.safe(cx) {
                    s.withdrawn = false;
                    s.enter(Phase::TurnBack, cx.tick);
                } else {
                    s.enter(Phase::Home, cx.tick);
                }
            }
        }
        _ => {}
    }
    let solution = s.solution(cx);
    let target_at = solution
        .map(|sol| sol.target_position)
        .or_else(|| s.target.and_then(|id| known_position(cx.picture, id)));
    let toward = |point: Vector, speed: SpeedRequest, s: &Strike| Steer {
        heading_deg: bearing_deg(own.position, point),
        pitch_deg: hold_pitch_deg(own.position[1], cruise_altitude(s, cx, point)),
        speed,
        afterburner: false,
        bank_deg: None,
        handling: Handling::PLAIN,
    };
    match s.phase {
        Phase::Home | Phase::Defend | Phase::Regain | Phase::Lase => order,
        Phase::HoldOff => Order {
            steer: s
                .heli
                .hold
                .map(|(centre, _)| hold_steer(cx, centre, &rings(s, cx))),
            activity: Some(Activity::Pursuing),
            ..order
        },
        Phase::Ingress | Phase::Wait | Phase::TurnBack => Order {
            steer: target_at
                .or_else(|| objective_point(cx.picture))
                .map(|p| toward(p, strike_speed(cx), s)),
            activity: Some(Activity::Pursuing),
            ..order
        },
        Phase::Exit => Order {
            steer: Some(exit_steer(s, cx, target_at)),
            activity: Some(Activity::Attacking),
            ..order
        },
        Phase::Attack => match target_at {
            Some(point) => attack(s, cx, point, solution),
            None => {
                exit(s, cx);
                Order {
                    steer: Some(exit_steer(s, cx, None)),
                    ..order
                }
            }
        },
    }
}

/// While a threat is up and the controller is not flying a helicopter break
/// (a warning not yet answered, or the ten quiet seconds after one): hold the
/// course at its strike speed and settle to the masking height over the highest
/// ground ahead, which `helicopter_defense` does for the break itself. The
/// fighters' air logic would otherwise fly this time at corner speed, which
/// sinks a helicopter (slice ST-H, docs/spec/ai-strike.md, "Helicopter
/// defence"). Inside a ring it knows it leaves by the shortest way out
/// instead of holding a course deeper in (slice H2, the H4 follow-up).
fn mask_run(cx: &Context<'_>, order: Order, rings: &[threat::Ring]) -> Order {
    if cx.defending {
        // The controller's defence flies, and overrides a steer.
        return order;
    }
    let own = cx.own;
    let ground = (own.position[1] - own.agl_ft).max(own.terrain_ahead_ft);
    let floor = ground + MASK_AGL_FT;
    let heading = threat::inside(rings, own.position).map_or(own.heading_deg, |ring| {
        bearing_deg(ring.centre, own.position)
    });
    Order {
        steer: Some(Steer {
            heading_deg: heading,
            pitch_deg: hold_pitch_deg(own.position[1], floor),
            speed: strike_speed(cx),
            afterburner: false,
            bank_deg: None,
            handling: Handling::PLAIN,
        }),
        activity: Some(Activity::Defending),
        ..order
    }
}

/// The speed every strike leg, the hold-off and the exit ask for (slice
/// HF1): the helicopter's strike speed.
fn strike_speed(cx: &Context<'_>) -> SpeedRequest {
    SpeedRequest::Explicit(crate::ai::helicopter_defense::strike_speed(&cx.own.limits))
}

/// The height the helicopter cruises and turns back at: the pop-up's
/// approach height over the target, or the masked height for a stand-off
/// attack.
fn cruise_altitude(s: &Strike, cx: &Context<'_>, target: Vector) -> f64 {
    let ground = cx.own.position[1] - cx.own.agl_ft;
    if s.pass == Some(Pass::StandOff) {
        ground + MASK_AGL_FT
    } else {
        (target[1] + POPUP_OFFSET_FT).max(ground + MASK_AGL_FT)
    }
}

/// The choice: target, store, pass and aim; or wait, or home.
fn choose(s: &mut Strike, cx: &Context<'_>) {
    let own = cx.own;
    // Once it has waited out a hold-off, a target no attack outside the
    // rings can reach is not taken (slice H4).
    let picked = if s.heli.waited && s.forced.is_none() {
        let allow = |id: u32| {
            known_position(cx.picture, id)
                .is_some_and(|p| plan(s, cx, id, p, ahead::search_due(cx.tick)).is_some())
        };
        s.pick_target_where(cx, &allow)
    } else {
        s.pick_target(cx)
    };
    let Some(target) = picked else {
        if s.heli.waited && s.forced.is_none() && s.pick_target(cx).is_some() {
            // Everything left is inside a ring: home (John, 2026-10-12).
            s.heli.decide(Decision::Home, cx.tick, s.target, None);
            s.heli.hold = None;
            s.enter(Phase::Home, cx.tick);
            s.target = None;
            return;
        }
        let waiting = cx
            .picture
            .briefed()
            .iter()
            .any(|b| b.standing && b.side != cx.side && b.kind != BriefedKind::Ordered);
        s.enter(
            if waiting && cx.picture.ordered().is_none() {
                Phase::Wait
            } else {
                Phase::Home
            },
            cx.tick,
        );
        s.target = None;
        return;
    };
    let target_at = known_position(cx.picture, target)
        .or_else(|| cx.inputs.solution.map(|sol| sol.target_position))
        .unwrap_or(own.position);
    let threat = cx
        .picture
        .threats()
        .level(cx.side, &[target_at], cx.warnings);
    let store = match s.forced_station {
        Some(station) => cx
            .inputs
            .stores
            .iter()
            .find(|st| st.station == station && st.rounds > 0)
            .copied(),
        None => choose_store(&cx.inputs.stores, cx.inputs.designator),
    };
    let Some(store) = store else {
        s.enter(Phase::Home, cx.tick);
        s.target = None;
        return;
    };
    if s.target != Some(target) {
        s.pass_number = 0;
    }
    s.target = Some(target);
    s.station = Some(store.station);
    s.kind = Some(store.kind);
    s.threat = threat;
    // The rings it knows decide how, or whether, it attacks (slice H4).
    let wanted = choose_pass(store.kind, threat);
    s.heli.approach = None;
    s.heli.standoff_ft = None;
    s.heli.ahead = None;
    let pass = match s.forced {
        Some(pass @ (Pass::PopUp | Pass::StandOff)) => pass,
        _ => match plan_with(
            s,
            cx,
            target,
            target_at,
            Some(store),
            wanted == Pass::StandOff,
            ahead::search_due(cx.tick),
        ) {
            Some(threat::Plan::PopUp) => Pass::PopUp,
            Some(threat::Plan::StandOff { approach, range_ft }) => {
                s.heli.approach = approach;
                s.heli.standoff_ft = approach.map(|_| range_ft);
                Pass::StandOff
            }
            Some(threat::Plan::Ahead(plan)) => {
                s.heli.approach = Some(plan.point);
                s.heli.standoff_ft = Some(plan.range_ft);
                s.heli.ahead = Some(plan);
                Pass::StandOff
            }
            None => {
                hold_off(s, cx, target);
                return;
            }
        },
    };
    if s.heli.hold.take().is_some() && std::mem::take(&mut s.heli.announced) {
        let decision = if s.heli.waited && s.heli.call.is_some_and(|c| c.target != Some(target)) {
            Decision::Retarget
        } else {
            Decision::Resume
        };
        s.heli.decide(decision, cx.tick, Some(target), None);
    }
    s.pass = Some(pass);
    s.pass_number += 1;
    let radius_ft = release_circle_ft(cx.experience);
    let draw = aim_draw(s.seed, cx.aircraft, s.pass_number, radius_ft);
    s.aim_ft = draw.offset_ft;
    s.inside = draw.inside;
    s.jink_side = draw.side;
    s.gate_ft = (GATE_SHARE * radius_ft).max(GATE_FLOOR_FT);
    s.holding = None;
    s.released = 0;
    s.next_release = 0;
    s.hold_ft = Some(match pass {
        Pass::StandOff => own.position[1] - own.agl_ft + MASK_AGL_FT,
        _ => target_at[1] + POPUP_OFFSET_FT,
    });
    s.enter(Phase::Attack, cx.tick);
}

fn exit(s: &mut Strike, cx: &Context<'_>) {
    // Away from the target, whichever pass (slice HF1; a stand-off attack
    // always did).
    s.exit_heading_deg = s
        .target
        .and_then(|id| known_position(cx.picture, id))
        .map_or(cx.own.heading_deg, |target| {
            bearing_deg(target, cx.own.position)
        });
    s.holding = None;
    s.heli.ahead = None;
    s.enter(Phase::Exit, cx.tick);
}

fn exit_distance(s: &Strike, cx: &Context<'_>) -> f64 {
    let radius = turn_radius_ft(cx.own.speed.0, cx.own.g_limit);
    let base = EXIT_TURN_MIN_FT.max(2. * radius);
    if s.pass == Some(Pass::StandOff) {
        // From the planned firing range when the plan set one (a firing
        // point off the line, or farther out than usual).
        let usual = s
            .station
            .and_then(|st| cx.inputs.stores.iter().find(|x| x.station == st))
            .map_or(0., |st| STANDOFF_SHARE * st.maximum_range_ft);
        let standoff = s.heli.standoff_ft.map_or(usual, |ft| ft.max(usual));
        base.max(standoff + STANDOFF_EXIT_FT)
    } else {
        base
    }
}

/// The exit (slice HF1, see the module notes): away from the target on the
/// exit heading, round the rings it knows, masked at [`MASK_AGL_FT`] over
/// the highest ground under and ahead of it, at the strike speed.
fn exit_steer(s: &Strike, cx: &Context<'_>, _target_at: Option<Vector>) -> Steer {
    let own = cx.own;
    let ground = (own.position[1] - own.agl_ft).max(own.terrain_ahead_ft);
    Steer {
        heading_deg: clear_heading(&rings(s, cx), own.position, s.exit_heading_deg),
        pitch_deg: hold_pitch_deg(own.position[1], ground + MASK_AGL_FT),
        speed: strike_speed(cx),
        afterburner: false,
        bank_deg: None,
        handling: Handling::PLAIN,
    }
}

/// A stand-off flown to a firing point off the straight line unmasks
/// within this of it (fitted, agent).
pub const APPROACH_REACHED_FT: f64 = 1_000.;
/// A stand-off shot leaves only with the nose within this of the target's
/// bearing (fitted, agent).
pub const STANDOFF_NOSE_DEG: f64 = 15.;
/// On the firing point's radial within this, across it (fitted, agent).
pub const ON_RADIAL_FT: f64 = 1_000.;
/// How far ahead an exit looks for a ring across its heading (fitted,
/// agent).
pub const EXIT_LOOK_FT: f64 = 4_000.;

/// The rings the helicopter knows (slice H4): its picture's hostile
/// defences, and the ring of an ambush it is keeping out of.
fn rings(s: &Strike, cx: &Context<'_>) -> Vec<threat::Ring> {
    let mut rings = threat::known_rings(cx.picture, cx.side);
    if let Some((ring, _)) = s.heli.ambush {
        rings.push(ring);
    }
    rings
}

/// The attack on `target` at `target_at` with the store it
/// would choose; `search` lets a moving target's move-ahead be looked for
/// (the costly part, [`ahead::SEARCH_TICKS`]).
fn plan(
    s: &Strike,
    cx: &Context<'_>,
    target: u32,
    target_at: Vector,
    search: bool,
) -> Option<threat::Plan> {
    let store = choose_store(&cx.inputs.stores, cx.inputs.designator);
    let threat_level = cx
        .picture
        .threats()
        .level(cx.side, &[target_at], cx.warnings);
    let wanted = store.is_some_and(|st| choose_pass(st.kind, threat_level) == Pass::StandOff);
    plan_with(s, cx, target, target_at, store, wanted, search)
}

/// The attack on `target` at `target_at` (slice H4): the pop-up when it is
/// clear and wanted; on a moving target under rings, with `search`, the
/// move-ahead ambush (John, 2026-10-29); else the stand-off from its place
/// now, as before.
fn plan_with(
    s: &Strike,
    cx: &Context<'_>,
    target: u32,
    target_at: Vector,
    store: Option<Store>,
    prefer_standoff: bool,
    search: bool,
) -> Option<threat::Plan> {
    let standoff = standoff_with_shots(cx, store, target_at);
    let rings = rings(s, cx);
    let fixed = threat::plan_attack(
        cx.own.position,
        target_at,
        standoff,
        prefer_standoff,
        &rings,
    );
    if fixed == Some(threat::Plan::PopUp) || rings.is_empty() || !search {
        return fixed;
    }
    let moving = target_track(cx, target, target_at).filter(ahead::Track::moving);
    if let (Some(track), Some(standoff)) = (moving, standoff)
        && let Some(plan) = ahead::plan(
            &flier(cx),
            cx.tick,
            track,
            &standoff,
            &moving_rings(cx, &rings),
        )
    {
        return Some(threat::Plan::Ahead(plan));
    }
    fixed
}

/// The store's stand-off with the host's answers on its shots.
fn standoff_with_shots<'a>(
    cx: &'a Context<'_>,
    store: Option<Store>,
    target_at: Vector,
) -> Option<threat::StandOff<'a>> {
    standoff_of(cx, store).map(|st| threat::StandOff {
        shots: store
            .filter(|_| cx.inputs.shots.known())
            .map(|store| threat::StandOffShots {
                shots: &cx.inputs.shots,
                station: store.station,
                launch: shot_launch(cx, target_at),
            }),
        ..st
    })
}

/// The helicopter as the move-ahead plans it: where it is, its heading and
/// its speed on the strike legs.
fn flier(cx: &Context<'_>) -> ahead::Flier {
    ahead::Flier {
        position: cx.own.position,
        heading_deg: cx.own.heading_deg,
        speed_fps: crate::ai::helicopter_defense::strike_speed(&cx.own.limits).0,
    }
}

/// `target` at `target_at` as the AI perceives it moving: its velocity
/// from the host's solution on it, else from its listed contact. `None`
/// when it is not listed or solved (its motion is not known).
fn target_track(cx: &Context<'_>, target: u32, target_at: Vector) -> Option<ahead::Track> {
    cx.inputs
        .solution
        .filter(|sol| sol.target == target)
        .map(|sol| ahead::Track::new(sol.target_position, sol.target_velocity))
        .or_else(|| {
            cx.picture
                .contact(target)
                .filter(|c| !c.destroyed)
                .map(|c| ahead::Track::new(target_at, c.velocity))
        })
}

/// The rings with their sites' motion: a known defence its lists hold
/// moving (a gun driving with the column) moves its ring along.
fn moving_rings(cx: &Context<'_>, rings: &[threat::Ring]) -> Vec<ahead::MovingRing> {
    ahead::moving_rings(rings, &|id| {
        cx.picture
            .contact(id)
            .filter(|c| !c.destroyed)
            .map(|c| ahead::Track::new(c.position, c.velocity))
    })
}

/// Whether the move-ahead in hand still stands, and the nose's offset to
/// guide with ([`ahead::holds`]).
fn ahead_holds(
    s: &Strike,
    cx: &Context<'_>,
    plan: &ahead::Ahead,
    target: u32,
    target_at: Vector,
) -> Option<f64> {
    let track = target_track(cx, target, target_at)?;
    let standoff = standoff_with_shots(cx, s.store(cx), target_at)?;
    let rings = rings(s, cx);
    ahead::holds(
        plan,
        &flier(cx),
        &track,
        &standoff,
        &moving_rings(cx, &rings),
    )
}

/// The launch a stand-off shot at a target at `target_at` is asked for (the
/// plan's ranges past the usual ones, John 2026-10-29): from the top of the
/// unmask over the ground under the helicopter now (the height the unmask
/// climbs to, [`UNMASK_CLIMB_FT`] over the masked run), level, at its strike
/// speed.
fn shot_launch(cx: &Context<'_>, target_at: Vector) -> crate::ai::strike::ShotLaunch {
    let own = cx.own;
    let launch_ft = own.position[1] - own.agl_ft + MASK_AGL_FT + UNMASK_CLIMB_FT;
    crate::ai::strike::ShotLaunch {
        target_ft: target_at[1],
        height_ft: launch_ft - target_at[1],
        speed_fps: crate::ai::helicopter_defense::strike_speed(&own.limits).0,
    }
}

/// A guided store's stand-off for the plan (none for rockets): its ranges,
/// its time of flight and the helicopter's speed while it guides: the
/// strike speed it asks for, plus the [`ROTOR_SPEED_MARGIN_KT`] the rotor
/// adapter may stand over a speed (126 kt flown for 120 asked; fitted,
/// agent). No shots: [`plan_with`] adds the host's.
fn standoff_of(cx: &Context<'_>, store: Option<Store>) -> Option<threat::StandOff<'static>> {
    let guide_fps = crate::ai::helicopter_defense::strike_speed(&cx.own.limits).0
        + ROTOR_SPEED_MARGIN_KT * crate::runway_wind::FEET_PER_SECOND_PER_KNOT;
    store
        .filter(|st| !st.kind.unguided())
        .map(|st| threat::StandOff {
            reach_ft: st.maximum_range_ft,
            fire_ft: st.fire_range_ft,
            flight: st.flight,
            guide_fps,
            shots: None,
        })
}

/// See [`standoff_of`].
const ROTOR_SPEED_MARGIN_KT: f64 = crate::ai::rotor_adapter::SPEED_HOLD_MARGIN_KT;

/// Whether the pass in hand still stands clear of the rings it knows.
fn pass_holds(s: &Strike, cx: &Context<'_>, target_at: Vector) -> bool {
    let plan = match s.pass {
        Some(Pass::StandOff) => threat::Plan::StandOff {
            approach: s.heli.approach,
            range_ft: s.heli.standoff_ft.unwrap_or_else(|| {
                s.store(cx)
                    .map_or(POPUP_PULL_AT_FT, |st| STANDOFF_SHARE * st.maximum_range_ft)
            }),
        },
        _ => threat::Plan::PopUp,
    };
    let standoff = standoff_of(cx, s.store(cx));
    threat::holds(plan, cx.own.position, target_at, standoff, &rings(s, cx))
}

/// Holds off `target`: circles masked outside the rings (slice H4).
fn hold_off(s: &mut Strike, cx: &Context<'_>, target: u32) {
    let rings = rings(s, cx);
    let centre = threat::hold_centre(cx.own.position, &rings, cx.terrain);
    let since = match s.heli.hold {
        // A hold already running keeps its clock.
        Some((_, since)) if s.target == Some(target) => since,
        _ => cx.tick,
    };
    if s.heli.hold.is_none() || s.target != Some(target) {
        s.heli.announced = false;
    }
    s.heli.hold = Some((centre, since));
    s.target = Some(target);
    s.heli.approach = None;
    s.heli.standoff_ft = None;
    s.heli.ahead = None;
    s.released = 0;
    s.holding = None;
    s.enter(Phase::HoldOff, cx.tick);
}

/// The ring that stands between the helicopter and `target`: the first
/// covering the target, else the first across the line to it.
fn target_ring(cx: &Context<'_>, target: u32, rings: &[threat::Ring]) -> Option<u32> {
    let at = known_position(cx.picture, target)?;
    threat::inside(rings, at)
        .or_else(|| {
            rings
                .iter()
                .find(|r| !threat::clear(std::slice::from_ref(r), cx.own.position, at))
        })
        .map(|r| r.id)
}

/// While holding off: attack once a plan stands clear of the rings; after
/// [`threat::HOLD_WAIT_TICKS`] give the target up for another outside the
/// rings, or go home (John, 2026-10-12).
fn hold_check(s: &mut Strike, cx: &Context<'_>) {
    let Some((_, since)) = s.heli.hold else {
        choose(s, cx);
        return;
    };
    // The call, once the hold has lasted: "holding off" for a known ring,
    // nothing more for the ambush ring it already called leaving.
    if !s.heli.announced
        && cx.tick.saturating_sub(since) >= threat::HOLD_CALL_TICKS
        && let Some(target) = s.target
    {
        s.heli.announced = true;
        let rings = rings(s, cx);
        let ring = target_ring(cx, target, &rings);
        if ring != Some(threat::AMBUSH_ID) {
            s.heli
                .decide(Decision::HoldOff, cx.tick, Some(target), ring);
        }
    }
    // A ring learnt since over the hold point moves it out again.
    let rings_now = rings(s, cx);
    if let Some((centre, since)) = s.heli.hold
        && rings_now
            .iter()
            .any(|r| horizontal(r.centre, centre) < r.radius_ft + threat::HOLD_RADIUS_FT - 1.)
    {
        let moved = threat::hold_centre(cx.own.position, &rings_now, cx.terrain);
        s.heli.hold = Some((moved, since));
    }
    let target_at = s.target.and_then(|id| known_position(cx.picture, id));
    let alive = s.target.is_some_and(|id| standing(cx.picture, id));
    // A moving target's move-ahead is looked for once a second.
    let search = ahead::search_due(cx.tick);
    let planned = |p: Vector| {
        s.target
            .is_some_and(|id| plan(s, cx, id, p, search).is_some())
    };
    if !alive || target_at.is_some_and(planned) {
        choose(s, cx);
        return;
    }
    if cx.tick.saturating_sub(since) >= threat::HOLD_WAIT_TICKS {
        s.heli.waited = true;
        choose(s, cx);
        if s.phase == Phase::HoldOff {
            // The same target came back blocked: nothing outside the rings.
            s.heli.decide(Decision::Home, cx.tick, s.target, None);
            s.heli.hold = None;
            s.target = None;
            s.enter(Phase::Home, cx.tick);
        }
    }
}

/// The hold-off circle: on the way there the heading turns round any ring
/// across it; there, retail's circle law about `centre` at the hold
/// radius (as the gunship's orbit, with the adapter's own bank), at
/// its strike speed, down at the masked height over the highest ground under
/// and ahead of it.
fn hold_steer(cx: &Context<'_>, centre: Vector, rings: &[threat::Ring]) -> Steer {
    let own = cx.own;
    let radius = threat::HOLD_RADIUS_FT;
    let distance = horizontal(own.position, centre);
    let offset = (90. - 45. * (distance - radius) / radius).clamp(0., 135.);
    let inside = threat::inside(rings, own.position).is_some();
    let heading = if distance > threat::HOLD_REACHED_FT || inside {
        // On the way there, round any ring across the line (slice H4); inside
        // a ring, the shortest way out first, even near the hold point
        // (slice H2).
        clear_heading(rings, own.position, bearing_deg(own.position, centre))
    } else {
        // A left-hand circle.
        (bearing_deg(own.position, centre) - offset).rem_euclid(360.)
    };
    let ground = (own.position[1] - own.agl_ft).max(own.terrain_ahead_ft);
    Steer {
        heading_deg: heading,
        pitch_deg: hold_pitch_deg(own.position[1], ground + MASK_AGL_FT),
        speed: strike_speed(cx),
        afterburner: false,
        bank_deg: None,
        handling: Handling::PLAIN,
    }
}

/// Flying to a firing point off the straight line (slice H4): first the
/// lead-in point [`threat::LEAD_IN_FT`] beyond it on the target's radial,
/// then, once on the radial (within [`ON_RADIAL_FT`] of it and outside the
/// firing point), the firing point itself, so it arrives nose on.
fn on_radial(own: Vector, target: Vector, point: Vector) -> Vector {
    let range = horizontal(target, point).max(1.);
    let u = [
        (point[0] - target[0]) / range,
        (point[2] - target[2]) / range,
    ];
    let (dx, dz) = (own[0] - target[0], own[2] - target[2]);
    let along = dx * u[0] + dz * u[1];
    let across = (dx * u[1] - dz * u[0]).abs();
    if along >= range && across <= ON_RADIAL_FT {
        point
    } else {
        let k = (range + threat::LEAD_IN_FT) / range;
        [
            target[0] + (point[0] - target[0]) * k,
            point[1],
            target[2] + (point[2] - target[2]) * k,
        ]
    }
}

/// A move-ahead ambush's shot from here, now ([`ahead::shot_clear`]): the
/// nose's offset to guide with, the planned one first.
fn shot_clear(s: &Strike, cx: &Context<'_>, target: Vector, plan: &ahead::Ahead) -> Option<f64> {
    let id = s.target?;
    let track = target_track(cx, id, target)?;
    let standoff = standoff_with_shots(cx, s.store(cx), target)?;
    let rings = rings(s, cx);
    ahead::shot_clear(
        &flier(cx),
        &track,
        &standoff,
        &moving_rings(cx, &rings),
        plan.offset_deg,
    )
}

/// A move-ahead ambush: time to leave the wait circle for the firing point
/// ([`ahead::go`]), the target as the AI perceives it now (its motion
/// unknown: now).
fn going(s: &Strike, cx: &Context<'_>, plan: &ahead::Ahead, target: Vector) -> bool {
    s.target
        .and_then(|id| target_track(cx, id, target))
        .is_none_or(|track| ahead::go(plan, &flier(cx), &track))
}

/// The heading nearest `heading` whose next [`EXIT_LOOK_FT`] stays out of
/// every ring, in 15 degree steps, right first; `heading` itself when none
/// does (boxed in). Inside a ring it is straight out of it.
fn clear_heading(rings: &[threat::Ring], own: Vector, heading: f64) -> f64 {
    if let Some(ring) = threat::inside(rings, own) {
        return bearing_deg(ring.centre, own);
    }
    let ahead = |h: f64| {
        let b = h.to_radians();
        [
            own[0] + b.sin() * EXIT_LOOK_FT,
            own[1],
            own[2] + b.cos() * EXIT_LOOK_FT,
        ]
    };
    for k in 0..=12 {
        for side in [1., -1.] {
            let h = (heading + side * f64::from(k) * 15.).rem_euclid(360.);
            if threat::clear(rings, own, ahead(h)) {
                return h;
            }
            if k == 0 || k == 12 {
                break;
            }
        }
    }
    heading
}

/// The ring of a threat it did not know (slice H4, John's rule): engaged
/// (a defence flown) by a shooter that is not a site it knows
/// ([`threat::known_shooter`] by the threat's bearing, as the helicopter
/// knows the rings when the engagement starts; John, 2026-10-29), it marks
/// an ambush ring there; each engagement keeps it fresh, and once the threat
/// has been quiet for [`threat::AMBUSH_QUIET_TICKS`] and the helicopter is
/// out of it, it is forgotten and the attack may resume. A known site's shot
/// is no ambush: inside its ring the helicopter leaves it (the mask run),
/// and its ring keeps it out.
fn track_ambush(s: &mut Strike, cx: &Context<'_>) {
    let own = cx.own.position;
    if cx.defending {
        match s.heli.ambush.as_mut() {
            Some((_, last)) => *last = cx.tick,
            // Judged as the defence starts, and again whenever the break
            // has the threat's bearing: a tick without one (the break lost
            // the round) is no evidence of a new shooter.
            None if s.phase != Phase::Defend || cx.threat_bearing_deg.is_some() => {
                let known = threat::rings_before_shot(cx.picture, cx.side, cx.tick);
                if threat::known_shooter(&known, own, cx.threat_bearing_deg).is_none() {
                    let ring = threat::ambush_ring(own, cx.threat_bearing_deg);
                    s.heli.ambush = Some((ring, cx.tick));
                    s.heli
                        .decide(Decision::Leave, cx.tick, s.target, Some(threat::AMBUSH_ID));
                }
            }
            None => {}
        }
        return;
    }
    if let Some((ring, last)) = s.heli.ambush
        && cx.tick.saturating_sub(last) >= threat::AMBUSH_QUIET_TICKS
        && horizontal(ring.centre, own) >= ring.radius_ft
    {
        s.heli.ambush = None;
    }
}

/// The pass in flight.
fn attack(s: &mut Strike, cx: &Context<'_>, target: Vector, solution: Option<Solution>) -> Order {
    let own = cx.own;
    let pass = s.pass.unwrap_or(Pass::PopUp);
    let kind = s.kind.unwrap_or(StoreKind::Rocket);
    let mut order = Order {
        steer: None,
        release: None,
        activity: Some(Activity::Attacking),
    };
    // A moving target is led by the rocket's time of flight, as a fighter's.
    s.lead_ft = match solution {
        Some(sol) if kind.unguided() => {
            let t = sol.impact_seconds.unwrap_or(0.).clamp(0., 60.);
            [sol.target_velocity[0] * t, sol.target_velocity[2] * t]
        }
        _ => [0.; 2],
    };
    let aim = if kind.unguided() {
        [
            target[0] + s.aim_ft[0] + s.lead_ft[0],
            target[1],
            target[2] + s.aim_ft[1] + s.lead_ft[1],
        ]
    } else {
        target
    };
    let distance = horizontal(own.position, target);
    let ahead = angle_difference(own.heading_deg, bearing_deg(own.position, target)).abs() < 90.;
    let standoff = s.heli.standoff_ft.unwrap_or_else(|| {
        s.store(cx)
            .map_or(POPUP_PULL_AT_FT, |st| STANDOFF_SHARE * st.maximum_range_ft)
    });

    // Segment changes.
    match (pass, s.segment) {
        (Pass::PopUp, Segment::Approach) if distance < POPUP_PULL_AT_FT && ahead => {
            s.segment_to(Segment::Pull, cx.tick)
        }
        (Pass::PopUp, Segment::Pull) if cx.tick.saturating_sub(s.since) >= POPUP_PULL_TICKS => {
            s.segment_to(Segment::Final, cx.tick)
        }
        // The stand-off run unmasks at its range: the "pull" is the climb.
        (Pass::StandOff, Segment::Approach)
            if distance <= standoff && s.heli.approach.is_none() =>
        {
            s.segment_to(Segment::Pull, cx.tick)
        }
        // A move-ahead ambush (John, 2026-10-29): unmask at the firing
        // point once it is time to go there, not as the wait circle passes
        // it.
        (Pass::StandOff, Segment::Approach)
            if s.heli.ahead.is_some_and(|plan| {
                horizontal(own.position, plan.point) <= APPROACH_REACHED_FT
                    && angle_difference(own.heading_deg, bearing_deg(own.position, target)).abs()
                        <= ahead::INBOUND_DEG
                    && going(s, cx, &plan, target)
            }) =>
        {
            s.segment_to(Segment::Pull, cx.tick)
        }
        // A firing point off the straight line (slice H4): unmask there.
        (Pass::StandOff, Segment::Approach)
            if s.heli.ahead.is_none()
                && s.heli
                    .approach
                    .is_some_and(|p| horizontal(own.position, p) <= APPROACH_REACHED_FT) =>
        {
            s.segment_to(Segment::Pull, cx.tick)
        }
        _ => {}
    }
    let since = cx.tick.saturating_sub(s.since);
    let guiding = s.segment == Segment::Guide;
    let ended = if guiding {
        // The launcher holds the line until its round is gone (a short
        // settle covers the step before the host sees it).
        since >= GUIDE_SETTLE_TICKS && !solution.is_some_and(|sol| sol.guided_in_flight)
    } else {
        (s.released > 0 && !s.burst_left(cx, solution, aim))
            || (pass == Pass::PopUp
                && s.segment == Segment::Final
                && (distance < POPUP_EXIT_FT || !ahead || own.agl_ft < RUN_FLOOR_AGL_FT))
            || (pass == Pass::StandOff && s.segment == Segment::Pull && since >= UNMASK_TICKS)
            || (pass == Pass::StandOff && distance < STRAIGHT_EXIT_FT)
            // A move-ahead shot not away by the end of the unmask it was
            // planned with is given up: farther in, the advance was not
            // judged clear.
            || (s.segment == Segment::Pull
                && s.heli
                    .ahead
                    .is_some_and(|plan| ahead::past_ft(&plan, own.position) > threat::ADVANCE_MARGIN_FT))
    };
    if ended {
        exit(s, cx);
        order.steer = Some(exit_steer(s, cx, Some(target)));
        return order;
    }

    // Steering.
    let point_at = |p: Vector| {
        let (heading, pitch) = steering_frame(own.position, p);
        (heading, pitch.clamp(-PITCH_LIMIT_DEG, PITCH_LIMIT_DEG))
    };
    // Every leg at the helicopter's best speed (John, 2026-10-11: "maintain
    // proper speed for defense and quick exfil"): asked for corner, the
    // adapter slows a helicopter until it sinks.
    let corner = strike_speed(cx);
    // A move-ahead ambush: on the wait circle until it is time to go, then
    // in along the firing radial from the predicted aim point.
    let ambush = match (pass, s.segment, s.heli.ahead) {
        (Pass::StandOff, Segment::Approach, Some(plan)) => Some(if going(s, cx, &plan, target) {
            (
                bearing_deg(own.position, on_radial(own.position, plan.aim, plan.point)),
                hold_pitch_deg(own.position[1], own.position[1] - own.agl_ft + MASK_AGL_FT),
                corner,
            )
        } else {
            let wait = hold_steer(cx, plan.orbit, &rings(s, cx));
            (wait.heading_deg, wait.pitch_deg, wait.speed)
        }),
        _ => None,
    };
    let steer = ambush.unwrap_or_else(|| match (pass, s.segment) {
        (Pass::PopUp, Segment::Approach) => {
            let (heading, pitch) = point_at([target[0], target[1] + POPUP_OFFSET_FT, target[2]]);
            (heading, pitch, corner)
        }
        (Pass::PopUp, Segment::Pull) => (
            bearing_deg(own.position, aim),
            (-depression_deg(own.position, target) + POPUP_PULL_DEG)
                .clamp(-PITCH_LIMIT_DEG, PITCH_LIMIT_DEG),
            corner,
        ),
        (Pass::StandOff, Segment::Approach) => (
            bearing_deg(
                own.position,
                s.heli
                    .approach
                    .map_or(target, |p| on_radial(own.position, target, p)),
            ),
            hold_pitch_deg(own.position[1], own.position[1] - own.agl_ft + MASK_AGL_FT),
            corner,
        ),
        (Pass::StandOff, _) => {
            // Unmasked: nose on the target, up to the unmask height.
            let top = s.hold_ft.unwrap_or(own.position[1]) + UNMASK_CLIMB_FT;
            (
                bearing_deg(own.position, target),
                hold_pitch_deg(own.position[1], top),
                corner,
            )
        }
        (_, Segment::Guide) => {
            // Holding the line: the nose on the target, level; a move-ahead
            // may hold it off the nose, inside the launcher's cone.
            (
                (bearing_deg(own.position, target)
                    + s.heli.ahead.map_or(0., |plan| plan.offset_deg))
                .rem_euclid(360.),
                hold_pitch_deg(own.position[1], own.position[1]),
                corner,
            )
        }
        _ => match solution
            .and_then(|sol| sol.impact)
            .filter(|_| kind.unguided())
        {
            Some(impact) => {
                let (_, pitch) = fly_cue(own, cx.velocity, aim, impact);
                (bearing_deg(own.position, aim), pitch, corner)
            }
            None => {
                let (heading, pitch) = point_at(aim);
                (heading, pitch, corner)
            }
        },
    });
    order.steer = Some(Steer {
        heading_deg: steer.0,
        pitch_deg: steer.1,
        speed: steer.2,
        afterburner: false,
        bank_deg: None,
        handling: Handling::PLAIN,
    });

    // The release: in the pop-up's final (rockets also in its pull), or
    // unmasked on a stand-off run.
    let window = match pass {
        Pass::StandOff => s.segment == Segment::Pull,
        _ => matches!(s.segment, Segment::Final | Segment::Pull),
    };
    if window
        && let Some(sol) = solution
        && cx.tick >= s.next_release
    {
        let first = s.released == 0;
        // A move-ahead ambush (John, 2026-10-29) fires once the target has
        // come to the planned range (within a step of the shot table's
        // grid), and only when the shot from here, now, hits reliably and
        // its advance stays outside every ring ([`ahead::shot_clear`]):
        // the nose's offset to guide with.
        let ambush_shot = s.heli.ahead.filter(|_| pass == Pass::StandOff).map(|plan| {
            (distance <= plan.range_ft + crate::ai::strike::reach::SHOT_STEP_FT)
                .then(|| shot_clear(s, cx, target, &plan))
                .flatten()
        });
        let in_zone = !kind.unguided()
            || sol.impact.is_some_and(|i| {
                let range =
                    (horizontal(own.position, i).powi(2) + (own.position[1] - i[1]).powi(2)).sqrt();
                s.store(cx).is_none_or(|st| {
                    range >= st.minimum_range_ft
                        && (st.maximum_range_ft <= 0. || range <= st.maximum_range_ft)
                })
            });
        let fire = in_zone
            && match kind {
                StoreKind::Rocket => {
                    (first && sol.due)
                        || (!first
                            && s.released < ROCKETS_PER_PASS
                            && sol.impact.is_some_and(|i| {
                                horizontal(i, aim) <= release_circle_ft(cx.experience)
                            }))
                }
                // A stand-off shot leaves with the nose on the target: a
                // firing point off the straight line is reached turning
                // (slice H4).
                _ if pass == Pass::StandOff => {
                    first
                        && sol.due
                        && !sol.guided_in_flight
                        && angle_difference(own.heading_deg, bearing_deg(own.position, target))
                            .abs()
                            <= STANDOFF_NOSE_DEG
                        && ambush_shot.is_none_or(|offset| offset.is_some())
                }
                _ => first && sol.due && !sol.guided_in_flight,
            };
        if fire && !sol.warning && s.rounds_left(cx) > 0 {
            order.release = Some(Release {
                station: sol.station,
                target: sol.target,
            });
            s.released += 1;
            s.next_release = cx.tick
                + s.store(cx)
                    .map_or(super::super::QUARTER_SECOND_TICKS, |st| st.cadence_ticks);
            if guided(kind) {
                s.segment_to(Segment::Guide, cx.tick);
            }
            if let (Some(plan), Some(Some(offset_deg))) = (s.heli.ahead.as_mut(), ambush_shot) {
                plan.offset_deg = offset_deg;
            }
        }
    }
    order
}

#[cfg(test)]
#[path = "family_helicopter_tests.rs"]
mod tests;
