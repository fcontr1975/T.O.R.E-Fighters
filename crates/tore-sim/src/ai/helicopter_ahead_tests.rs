//! The move-ahead ambush (docs/spec/ai-strike.md, "Move-ahead ambush"):
//! the firing point ahead of a moving target, the predicted rings, the
//! re-plan and the static target. Synthetic geometry only.
use super::*;
use crate::ai::helicopter_threat::{RING_MARGIN_FT, Ring, StandOff, StandOffShots};
use crate::ai::strike::{FlightTimes, ShotLaunch, ShotTable, Shots};
use crate::ai::weapon_service::StationId;
use std::sync::Arc;

/// Every shot out to 15,000 ft hits, at 550 ft/s (an AT-2-like round).
struct ToFifteen;

impl ShotTable for ToFifteen {
    fn shot(&self, _: StationId, _: ShotLaunch, range_ft: f64) -> Option<f64> {
        (range_ft <= 15_000.).then_some(range_ft / 550.)
    }
}

fn shots() -> Shots {
    Shots(Some(Arc::new(ToFifteen)))
}

/// The AT-2 as the plan sees it, with the host's answers.
fn at2(shots: &Shots) -> StandOff<'_> {
    StandOff {
        reach_ft: 18_000.,
        fire_ft: 10_000.,
        flight: Some(FlightTimes {
            points: [
                (4_500., 8.2),
                (9_000., 16.4),
                (13_500., 24.5),
                (18_000., 32.7),
            ],
        }),
        guide_fps: 213.,
        shots: Some(StandOffShots {
            shots,
            station: StationId(1),
            launch: ShotLaunch {
                target_ft: 0.,
                height_ft: 700.,
                speed_fps: 203.,
            },
        }),
    }
}

/// A gun's ring: 7,500 ft of reach plus the margin.
fn gun(id: u32, at: Vector) -> MovingRing {
    MovingRing::standing(Ring {
        id,
        centre: at,
        radius_ft: 7_500. + RING_MARGIN_FT,
    })
}

/// A column tank at the origin driving north at 50 ft/s.
fn column() -> Track {
    Track::new([0.; 3], [0., 0., 50.])
}

/// A Mi-24 at 120 kt, `at`, heading `heading_deg`.
fn heli(at: Vector, heading_deg: f64) -> Flier {
    Flier {
        position: at,
        heading_deg,
        speed_fps: 203.,
    }
}

/// Whether a ring holds `point` at `seconds` from now (widened by `extra`).
fn covers(ring: &MovingRing, point: Vector, seconds: f64, extra: f64) -> bool {
    horizontal(ring.at(seconds), point) < ring.ring.radius_ft + extra
}

/// Seconds from the plan's tick to its shot.
fn fire_s(plan: &Ahead) -> f64 {
    (plan.fire_tick - plan.planned_tick) as f64 / 120.
}

/// Guns beside the road the column drives up: one where the tank is now,
/// one ahead of it.
fn roadside_guns() -> [MovingRing; 2] {
    [gun(1, [-3_000., 0., 0.]), gun(2, [3_000., 0., 16_000.])]
}

#[test]
fn a_moving_target_among_fixed_guns_gets_a_firing_point_ahead_outside_every_ring() {
    // The Mi-24 comes in from the west.
    let rings = roadside_guns();
    let shots = shots();
    let standoff = at2(&shots);
    let own = heli([-26_000., 300., 8_000.], 90.);
    let target = column();
    let plan = plan(&own, 600, target, &standoff, &rings).expect("a move-ahead");
    let t = fire_s(&plan);
    // The shot is timed on where the target will be, up the road.
    assert!(horizontal(plan.aim, target.at(t)) < 1., "{plan:?}");
    assert!(
        plan.aim[2] > 1_000.,
        "the aim lies ahead on the road: {plan:?}"
    );
    // At the shot the target is in range (at the planned range, or nearer
    // when the helicopter gets there after the target does).
    assert!(horizontal(plan.point, plan.aim) <= plan.range_ft + 1.);
    // Outside every ring, with the search's room, at the shot.
    for ring in &rings {
        assert!(!covers(ring, plan.point, t, PLAN_MARGIN_FT), "{plan:?}");
        assert!(!covers(ring, plan.orbit, t, 0.), "{plan:?}");
    }
    // It can be there in time.
    assert!(route(&own, plan.aim, plan.point).1 / own.speed_fps + LEAD_S <= t + 1e-6);
    // The plan holds as made, and it is a fixed function of its inputs.
    assert!(holds(&plan, &own, &target, &standoff, &rings).is_some());
    assert_eq!(
        super::plan(&own, 600, target, &standoff, &rings),
        Some(plan)
    );
}

#[test]
fn a_ring_that_drives_with_the_column_is_predicted_where_it_will_be() {
    // An escort gun drives 1,000 ft ahead of the tank. Taken as standing at
    // its place now, the column leaves it behind and a shot from up the road
    // looks clear; predicted with the column, that shot's advance runs into
    // it.
    let shots = shots();
    let standoff = at2(&shots);
    let own = heli([0., 300., 30_000.], 180.);
    let target = column();
    let escort = Ring {
        id: 7,
        centre: [0., 0., 1_000.],
        radius_ft: 7_500. + RING_MARGIN_FT,
    };
    let standing = [MovingRing::standing(escort)];
    let moving = moving_rings(&[escort], &|id| {
        (id == 7).then_some(Track::new([0., 0., 1_000.], [0., 0., 50.]))
    });
    assert_eq!(moving[0].velocity, [0., 0., 50.]);
    let blind =
        plan(&own, 0, target, &standoff, &standing).expect("a plan with the gun left behind");
    // Where the escort will really be, that plan does not hold.
    assert_eq!(holds(&blind, &own, &target, &standoff, &moving), None);
    // Predicted, any firing point the plan takes keeps out of the escort's
    // ring where it will be at the shot, and holds.
    if let Some(plan) = plan(&own, 0, target, &standoff, &moving) {
        let t = fire_s(&plan);
        assert!(
            !covers(&moving[0], plan.point, t, PLAN_MARGIN_FT),
            "{plan:?}"
        );
        assert!(holds(&plan, &own, &target, &standoff, &moving).is_some());
    }
    // A gun that is not moving keeps its ring where it stands.
    let parked = moving_rings(&[escort], &|_| Some(Track::new([0., 0., 1_000.], [0.; 3])));
    assert_eq!(parked, standing.to_vec());
}

#[test]
fn a_target_that_turns_away_is_planned_again() {
    let rings = roadside_guns();
    let shots = shots();
    let standoff = at2(&shots);
    let own = heli([-26_000., 300., 8_000.], 90.);
    let target = column();
    let plan = plan(&own, 600, target, &standoff, &rings).expect("a move-ahead");
    // Driving on as predicted, it holds a second later.
    let later = Track::new(target.at(1.), target.velocity);
    assert!(holds(&plan, &own, &later, &standoff, &rings).is_some());
    // Turned away east, its path never comes to the planned range of the
    // firing point: the plan is dropped.
    let away = Track::new(target.at(1.), [50., 0., 0.]);
    assert_eq!(holds(&plan, &own, &away, &standoff, &rings), None);
    // Stopped, it is a static target again.
    let halted = Track::new(target.at(1.), [0., 0., 2.]);
    assert_eq!(holds(&plan, &own, &halted, &standoff, &rings), None);
    // A new ring over the firing point drops it too.
    let mut more = rings.to_vec();
    more.push(gun(3, plan.point));
    assert_eq!(holds(&plan, &own, &later, &standoff, &more), None);
}

#[test]
fn a_static_target_has_no_move_ahead() {
    let rings = [gun(1, [-3_000., 0., 0.])];
    let shots = shots();
    let standoff = at2(&shots);
    let own = heli([-26_000., 300., 8_000.], 90.);
    let parked = Track::new([0.; 3], [0.; 3]);
    assert_eq!(plan(&own, 600, parked, &standoff, &rings), None);
    // Creeping slower than MOVING_FPS counts as standing.
    let creeping = Track::new([0.; 3], [0., 0., MOVING_FPS - 0.5]);
    assert_eq!(plan(&own, 600, creeping, &standoff, &rings), None);
}

#[test]
fn the_window_is_when_the_path_lies_within_range() {
    let target = column();
    // 10,000 ft east of the road and 10,000 ft up it: within 12,500 ft
    // while the tank is 2,500 to 17,500 ft up, from 50 s to the horizon.
    let (t_in, t_out) = window([10_000., 0., 10_000.], 12_500., &target).expect("it passes");
    assert!((t_in - 50.).abs() < 1e-6, "{t_in}");
    assert_eq!(t_out, HORIZON_S);
    // Inside now: from now until it has driven 5,500 ft up the road.
    let (t_in, t_out) = window([10_000., 0., -2_000.], 12_500., &target).expect("inside");
    assert_eq!(t_in, 0.);
    assert!((t_out - 5_500. / 50.).abs() < 1e-6, "{t_out}");
    // Passing wide: never.
    assert_eq!(window([20_000., 0., 0.], 12_500., &target), None);
}

#[test]
fn the_last_check_holds_the_nose_off_to_keep_the_advance_out_of_a_ring() {
    // A gun to the right of the line from the helicopter to the tank: the
    // straight advance runs into its ring, a nose held off to the left
    // keeps out.
    let shots = shots();
    let standoff = at2(&shots);
    let target = Track::new([0.; 3], [50., 0., 0.]);
    let own = heli([0., 300., -13_000.], 0.);
    let ring = gun(1, [9_000., 0., -6_000.]);
    let offset = shot_clear(&own, &target, &standoff, &[ring], 0.).expect("a clear shot");
    assert!(offset < 0., "held off to the left: {offset}");
    assert!(GUIDE_OFFSETS_DEG.contains(&offset));
    // With no ring, straight on.
    assert_eq!(shot_clear(&own, &target, &standoff, &[], 0.), Some(0.));
}
