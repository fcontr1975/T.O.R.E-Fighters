//! A fixed-wing striker's stand-off release outside known rings (John,
//! 2026-10-29): planned from the host's answers, released at the plan from
//! outside the ring, the exit away from the target, and no change with no
//! defence covering the target. Synthetic pictures and answers only.
use super::super::tests::{
    TARGET, TARGET_AT, World, ground, inputs, own, picture, site, solution, store,
};
use super::super::{Pass, Phase, ShotLaunch, ShotTable, Shots, StoreKind, Strike};
use super::*;
use crate::ai::weapon_service::StationId;
use crate::checkpoint::{Models, from_bytes, to_bytes};

/// Every shot hits, at 1,000 ft/s.
struct AllHit;

impl ShotTable for AllHit {
    fn shot(&self, _: StationId, _: ShotLaunch, range_ft: f64) -> Option<f64> {
        Some(range_ft / 1_000.)
    }
}

const SAM: u32 = 0x5000_0009;
/// A SAM 2,000 ft beyond the target whose 25,000 ft reach covers it: its
/// ring (27,000 ft) spans the line from 1,000 ft south of the striker's
/// start line (z = 0) to 53,000 ft north.
const SAM_AT: Vector = [0., 0., 26_000.];
const SAM_REACH_FT: f64 = 25_000.;
/// The striker starts 44,000 ft south of the target.
const START_Z: f64 = -20_000.;

/// An attack jet at 5,000 ft and 700 ft/s, south of the target, with a
/// seeker missile (a 10,000 ft fire range, a 40,000 ft zone) and the host's
/// answers; the SAM there or not.
fn seeker_world(defended: bool) -> World {
    let mut missile = store(1, StoreKind::Seeker);
    missile.maximum_range_ft = 40_000.;
    missile.minimum_range_ft = 1_000.;
    let mut solution = solution(TARGET_AT, TARGET_AT, false);
    solution.impact = None;
    solution.reaches = true;
    let mut inputs = inputs(vec![missile], Some(solution));
    inputs.shots = Shots(Some(std::sync::Arc::new(AllHit)));
    let mut subjects = vec![ground(TARGET, TARGET_AT, 0x200, None)];
    if defended {
        subjects.push(ground(SAM, SAM_AT, 0x1000, site(SAM_REACH_FT, 0., true)));
    }
    World {
        own: own([0., 5_000., START_Z], 700.),
        picture: picture(&subjects, if defended { vec![SAM] } else { vec![] }),
        inputs,
    }
}

/// The striker `range` south of the target.
fn at(world: &mut World, range: f64) {
    world.own.position[2] = TARGET_AT[2] - range;
}

#[test]
fn a_seeker_leaves_from_outside_a_known_ring_covering_its_target() {
    let mut world = seeker_world(true);
    let mut strike = Strike::new(7);
    let order = strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::FastHigh));
    assert_eq!(order.release, None);
    // Its usual release (the 10,000 ft fire range from 5,000 ft up: 8,660 ft
    // out) lies inside the ring. The nearest range whose release point and
    // turn away (two turn radii, 3,211 ft) stand outside it is 28,500 ft.
    let plan = strike.release_plan.expect("a stand-off plan");
    assert_eq!(plan.range_ft, 28_500.);
    assert!((plan.advance_ft - 2. * 700. * 700. / (43.6 * 7.)).abs() < 1e-9);
    // Not yet at the plan: no release, and the committed window times it.
    at(&mut world, 29_500.);
    let order = strike.step(&world.context(2));
    assert_eq!(order.release, None);
    let in_s = strike.release_in_s().expect("timed");
    assert!((in_s - 1_000. / 700.).abs() < 1e-6, "{in_s}");
    // At the plan, outside the ring, it releases on the store's lock.
    at(&mut world, 28_400.);
    let order = strike.step(&world.context(3));
    assert!(order.release.is_some());
    assert!(horizontal(world.own.position, SAM_AT) > SAM_REACH_FT + 2_000.);
    // Then it turns away from the target, and opens past the plan's range,
    // its turn and the stand-off exit before it turns back.
    assert_eq!(strike.phase(), Phase::Exit);
    assert!((strike.exit_heading_deg - 180.).abs() < 1e-9);
    assert!(strike.standoff_until.is_some_and(|t| t > 3 + 28 * 120));
    let open = 28_500. + plan.advance_ft + STANDOFF_EXIT_FT;
    at(&mut world, open - 100.);
    strike.step(&world.context(4));
    assert_eq!(strike.phase(), Phase::Exit, "not yet far enough");
    at(&mut world, open + 100.);
    strike.step(&world.context(5));
    assert_eq!(strike.phase(), Phase::TurnBack);
}

#[test]
fn back_at_its_plan_while_its_round_flies_it_turns_away_again() {
    let mut world = seeker_world(true);
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    at(&mut world, 28_400.);
    assert!(strike.step(&world.context(2)).release.is_some());
    // A new pass while the round still flies: past its plan by the slack it
    // does not press on into the ring; it ends the pass there, turning
    // away.
    at(&mut world, 26_000.);
    strike.choose(&world.context(3));
    assert_eq!(strike.phase(), Phase::Attack);
    let order = strike.step(&world.context(4));
    assert_eq!(order.release, None);
    assert_eq!(strike.phase(), Phase::Exit);
    assert!((strike.exit_heading_deg - 180.).abs() < 1e-9);
}

#[test]
fn with_no_defence_covering_the_target_the_pass_is_as_before() {
    // No SAM: no plan, and nothing leaves until the usual lock in fire
    // range (the host's due).
    let mut world = seeker_world(false);
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    assert_eq!(strike.release_plan, None);
    at(&mut world, 28_400.);
    assert_eq!(strike.step(&world.context(2)).release, None);
    at(&mut world, 8_000.);
    if let Some(s) = world.inputs.solution.as_mut() {
        s.due = true;
    }
    assert!(strike.step(&world.context(3)).release.is_some());
    // The exit is straight on, as before.
    assert_eq!(strike.phase(), Phase::Exit);
    assert!(strike.exit_heading_deg.abs() < 1e-9);
}

#[test]
fn with_no_safe_range_in_reach_the_pass_is_as_before() {
    // A zone of 18,000 ft: every reliable range lies inside the ring. No plan; it presses to its usual release.
    let mut world = seeker_world(true);
    world.inputs.stores[0].maximum_range_ft = 18_000.;
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    assert_eq!(strike.release_plan, None);
    at(&mut world, 8_000.);
    assert_eq!(strike.step(&world.context(2)).release, None);
    if let Some(s) = world.inputs.solution.as_mut() {
        s.due = true;
    }
    assert!(strike.step(&world.context(3)).release.is_some());
}

#[test]
fn a_planned_stand_off_round_trips_through_a_checkpoint() {
    let mut world = seeker_world(true);
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    at(&mut world, 28_400.);
    strike.step(&world.context(2));
    assert!(strike.release_plan.is_some() && strike.standoff_until.is_some());
    let models = Models::default();
    let twin: Strike = from_bytes(&to_bytes(&strike, &models).unwrap(), &models).unwrap();
    assert_eq!(twin, strike);
}

#[test]
fn a_usual_release_outside_the_ring_keeps_its_point_and_turns_away_after_it() {
    // A short-range site (5,000 ft, a 7,000 ft ring) 2,000 ft beyond the
    // target covers it, but the usual release (8,660 ft out from 5,000 ft up)
    // and the turn after it stand outside its ring: the release is where it
    // was; the exit then turns away instead of flying on over the target
    // into the ring.
    let mut world = seeker_world(false);
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(SAM, SAM_AT, 0x1000, site(5_000., 0., true)),
        ],
        vec![SAM],
    );
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    let plan = strike
        .release_plan
        .expect("the usual point kept as the plan");
    assert!((plan.range_ft - 75_000_000_f64.sqrt()).abs() < 1e-6);
    at(&mut world, 9_000.);
    assert_eq!(strike.step(&world.context(2)).release, None);
    at(&mut world, 8_600.);
    assert!(strike.step(&world.context(3)).release.is_some());
    assert_eq!(strike.phase(), Phase::Exit);
    assert!((strike.exit_heading_deg - 180.).abs() < 1e-9);
}
