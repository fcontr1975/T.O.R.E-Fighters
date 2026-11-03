//! The gunship family (docs/spec/ai-strike.md, "AC-130"): the orbit from
//! the guns' reach, the bank cap, the orbit law, the flow from ingress to
//! the orbit's bursts, the hold-off under a SAM, determinism and the
//! checkpoint. Synthetic pictures only.
use super::super::tests::{OWN, TARGET, TARGET_AT, World, ground, inputs, own, picture, site};
use super::super::{Phase, Steer, Store, StoreKind, Strike};
use super::*;
use crate::ai::SpeedLimits;
use crate::ai::weapon_service::StationId;
use crate::checkpoint::{Models, from_bytes, to_bytes};
use crate::models::strike_class::StrikeClass;

const G: f64 = 32.174;

fn orbit(radius_ft: f64, height_ft: f64, speed_fps: f64) -> OrbitPlan {
    OrbitPlan {
        radius_ft,
        height_ft,
        slant_ft: radius_ft.hypot(height_ft),
        depression_deg: DEPRESSION_DEG,
        speed_fps,
        bank_deg: 0.,
    }
}

fn turn_radius(speed: f64, bank_deg: f64) -> f64 {
    speed * speed / (G * bank_deg.to_radians().tan())
}

#[test]
fn the_orbit_comes_from_the_guns_reach() {
    // Slow enough that the turn fits: the spec's numbers.
    let plan = orbit_plan(13_000., 230., 414., 30.);
    assert!((plan.slant_ft - 10_400.).abs() < 1., "{plan:?}");
    assert!((plan.radius_ft - 8_519.).abs() < 2., "{plan:?}");
    assert!((plan.height_ft - 5_965.).abs() < 2., "{plan:?}");
    assert_eq!(plan.depression_deg, 35.);
    // Retail's speed, halfway between corner and maximum.
    assert!((plan.speed_fps - 322.).abs() < 1e-9);
    // The bank that radius needs at that speed, under the cap.
    let bank = (322_f64 * 322. / (G * plan.radius_ft)).atan().to_degrees();
    assert!((plan.bank_deg - bank).abs() < 1e-9 && plan.bank_deg < 25.);
    // A shorter reach shrinks the orbit with it.
    let short = orbit_plan(8_000., 150., 250., 30.);
    assert!((short.slant_ft - 6_400.).abs() < 1.);
}

#[test]
fn the_radius_grows_instead_of_the_bank() {
    // Too fast for 8,520 ft at the planning bank (25): the radius grows.
    let plan = orbit_plan(13_000., 330., 390., 30.);
    let wanted = turn_radius(360., 25.);
    assert!(wanted > 8_520.);
    assert!((plan.radius_ft - wanted).abs() < 1., "{plan:?}");
    assert!((plan.bank_deg - 25.).abs() < 1e-6);
    assert!((plan.height_ft / plan.radius_ft - 35_f64.to_radians().tan()).abs() < 1e-9);
    // Faster still: the radius stops where the slant is 90% of the reach,
    // and the speed comes down to fit, never below corner.
    let plan = orbit_plan(13_000., 300., 560., 30.);
    let cap = 0.9 * 13_000. * 35_f64.to_radians().cos();
    assert!((plan.radius_ft - cap).abs() < 1., "{plan:?}");
    assert!((plan.slant_ft - 11_700.).abs() < 1.);
    assert!(plan.speed_fps < 430. && plan.speed_fps >= 300.);
    assert!((plan.bank_deg - 25.).abs() < 1e-6);
    // Corner itself is too fast: the bank stays capped, the radius grows.
    let plan = orbit_plan(13_000., 450., 600., 30.);
    assert!(plan.bank_deg <= 25. + 1e-6 && plan.radius_ft > cap);
}

#[test]
fn the_orbit_law_never_banks_past_the_maximum() {
    let centre = [0., 0., 0.];
    for x in [
        -30_000., -9_000., -100., 0., 4_000., 8_520., 12_000., 40_000.,
    ] {
        for z in [-20_000., -500., 0., 3_000., 25_000.] {
            for heading in (0..360).step_by(15) {
                for speed in [150., 330., 600.] {
                    let steer = orbit_steer(
                        [x, 6_000., z],
                        f64::from(heading),
                        speed,
                        centre,
                        &orbit(8_520., 5_965., 330.),
                        30.,
                    );
                    let bank = steer.bank_deg.expect("an explicit bank");
                    assert!(
                        bank.abs() <= 30. + 1e-9,
                        "{x} {z} {heading} {speed}: {bank}"
                    );
                    assert!(steer.pitch_deg.abs() <= PITCH_LIMIT_DEG);
                }
            }
        }
    }
}

#[test]
fn on_the_orbit_it_banks_left_to_hold_the_circle() {
    // 8,520 ft east of the centre, heading north: the centre on the left.
    let steer = orbit_steer(
        [8_520., 5_965., 0.],
        0.,
        322.,
        [0., 0., 0.],
        &orbit(8_520., 5_965., 322.),
        30.,
    );
    let steady = (322_f64 * 322. / (G * 8_520.)).atan().to_degrees();
    let bank = steer.bank_deg.unwrap();
    assert!((bank + steady).abs() < 0.5, "{bank} vs -{steady}");
    assert!(steer.heading_deg.abs() < 1e-6 || (steer.heading_deg - 360.).abs() < 1e-6);
    assert!(on_orbit([8_520., 5_965., 0.], 0., [0., 0., 0.], 8_520.));
    // Far out it converges at 45 degrees off the bearing, turning toward.
    let far = orbit_steer(
        [40_000., 5_965., 0.],
        0.,
        322.,
        [0., 0., 0.],
        &orbit(8_520., 5_965., 322.),
        30.,
    );
    assert!((far.heading_deg - 315.).abs() < 1e-6, "{}", far.heading_deg);
    assert!(far.bank_deg.unwrap() < 0.);
    assert!(!on_orbit([40_000., 5_965., 0.], 0., [0., 0., 0.], 8_520.));
}

fn gun(station: u8, rounds: u32) -> Store {
    Store {
        station: StationId(station),
        kind: StoreKind::Gun,
        rounds,
        cadence_ticks: 30,
        fire_range_ft: 13_000.,
        minimum_range_ft: 0.,
        maximum_range_ft: 13_000.,
        round_speed_fps: Some(3_450.),
        release_speed_share: 1.,
        bomb_body: Default::default(),
        fuze_arm_s: 0.,
        frag_radius_ft: 0.,
        falls_free: false,
        flight: None,
    }
}

/// An AC-130 on the orbit about the truck: east of it, heading north,
/// 5,965 ft above it.
fn gunship_world() -> World {
    let mut own = own(
        [TARGET_AT[0] + 8_520., TARGET_AT[1] + 5_965., TARGET_AT[2]],
        322.,
    );
    own.limits = SpeedLimits {
        minimum: crate::ai::ScalarSpeed(170.),
        maximum: crate::ai::ScalarSpeed(414.),
        corner: crate::ai::ScalarSpeed(230.),
    };
    let mut inputs = inputs(vec![gun(4, 3_000), gun(5, 256), gun(6, 100)], None);
    inputs.class = StrikeClass::Gunship;
    World {
        own,
        picture: picture(&[ground(TARGET, TARGET_AT, 0x200, None)], Vec::new()),
        inputs,
    }
}

fn steer_of(order: &super::super::Order) -> Steer {
    order.steer.expect("the gunship steers")
}

#[test]
fn on_the_orbit_it_fires_bursts_walked_in_from_the_skill_circle() {
    let world = gunship_world();
    let mut strike = Strike::new(7);
    let order = strike.step(&world.context(100));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::Orbit));
    assert_eq!(strike.target(), Some(TARGET));
    assert!(steer_of(&order).bank_deg.is_some());
    assert!(order.release.is_none(), "the guns fire through the host");
    // The first burst: the trigger held, the aim inside the ring's edge.
    let fire = strike.gunship_fire(100).expect("firing");
    assert_eq!(fire.burst, 1);
    assert!(fire.held);
    let r = release_circle_ft(crate::ai::Experience::Average);
    let first = fire.aim_ft[0].hypot(fire.aim_ft[1]);
    assert!(first <= 2. * r);
    assert!(!strike.gunship_fire(100 + FIRE_TICKS).unwrap().held);
    // Every burst opens on its own drawn offset and walks onto the target
    // over the skill's walk-in time (Average: 0.3 s), the same for each.
    let walk_ticks =
        (crate::ai::strike::gun_walk_in_s(crate::ai::Experience::Average) * 120.).round() as u64;
    let mut tick = 100;
    for burst in 2..=8 {
        let start = 100 + (burst - 1) * BURST_TICKS;
        let advance = |strike: &mut Strike, tick: &mut u64, to: u64| {
            while *tick < to {
                *tick += 1;
                strike.step(&world.context(*tick));
            }
        };
        advance(&mut strike, &mut tick, start);
        let fire = strike.gunship_fire(start).unwrap();
        assert_eq!(fire.burst, burst as u32);
        let drawn = aim_draw(7, OWN, burst as u32, r);
        assert!((fire.aim_ft[0] - drawn.offset_ft[0]).abs() < 1e-9);
        assert!((fire.aim_ft[1] - drawn.offset_ft[1]).abs() < 1e-9);
        assert_eq!(fire.inside, drawn.inside);
        // Halfway through the walk-in half the offset is left; after it,
        // none.
        advance(&mut strike, &mut tick, start + walk_ticks / 2);
        let half = strike.gunship_fire(tick).unwrap().aim_ft;
        assert!((half[0] - drawn.offset_ft[0] * 0.5).abs() < 1e-6);
        advance(&mut strike, &mut tick, start + walk_ticks);
        assert_eq!(strike.gunship_fire(tick).unwrap().aim_ft, [0.; 2]);
    }
    assert_eq!(walk_in_left(crate::ai::Experience::Ace, 0), 1.);
    assert!(walk_in_left(crate::ai::Experience::Ace, 6) > 0.);
    assert_eq!(walk_in_left(crate::ai::Experience::Ace, 12), 0.);
    assert!(walk_in_left(crate::ai::Experience::Novice, 12) > 0.6);
}

#[test]
fn far_from_the_target_it_flies_in_without_firing() {
    let mut world = gunship_world();
    world.own.position = [TARGET_AT[0], 6_000., TARGET_AT[2] - 40_000.];
    let mut strike = Strike::new(7);
    let order = strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Ingress);
    assert!(strike.gunship_fire(1).is_none());
    assert_eq!(order.activity, Some(Activity::Pursuing));
    // The circle law: 45 degrees off the bearing to the target.
    assert!((steer_of(&order).heading_deg - 45.).abs() < 1e-6);
}

#[test]
fn it_holds_off_while_a_sam_covers_the_target() {
    let mut world = gunship_world();
    const SAM: u32 = 0x5000_0009;
    let sam_at = [TARGET_AT[0] + 3_000., 0., TARGET_AT[2]];
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(SAM, sam_at, 0x1000, site(30_000., 0., true)),
        ],
        vec![SAM],
    );
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Wait);
    assert!(strike.gunship_fire(1).is_none());
    // It circles outside the SAM's reach: from the orbit it turns away.
    let order = strike.step(&world.context(2));
    let steer = steer_of(&order);
    assert!(
        steer.bank_deg.unwrap() > 0.,
        "it turns out, away from the SAM"
    );
    // The SAM gone from the picture: back to the orbit.
    world.picture = picture(&[ground(TARGET, TARGET_AT, 0x200, None)], Vec::new());
    strike.step(&world.context(3));
    assert_eq!(strike.phase(), Phase::Attack);
}

#[test]
fn with_no_rounds_left_it_goes_home() {
    let mut world = gunship_world();
    world.inputs.stores = vec![gun(4, 0)];
    let mut strike = Strike::new(7);
    strike.step(&world.context(1));
    assert!(strike.home());
}

#[test]
fn the_gunship_is_deterministic_and_round_trips_through_a_checkpoint() {
    let mut world = gunship_world();
    let mut a = Strike::new(11);
    let mut b = Strike::new(11);
    for tick in 0..600 {
        world.own.heading_deg = (world.own.heading_deg - 0.02).rem_euclid(360.);
        assert_eq!(
            a.step(&world.context(tick)),
            b.step(&world.context(tick)),
            "tick {tick}"
        );
        assert_eq!(a.gunship_fire(tick), b.gunship_fire(tick));
    }
    let models = Models::default();
    let coded = to_bytes(&a, &models).unwrap();
    let mut twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, a);
    for tick in 600..900 {
        assert_eq!(
            a.step(&world.context(tick)),
            twin.step(&world.context(tick))
        );
        assert_eq!(a.gunship_fire(tick), twin.gunship_fire(tick));
    }
}
