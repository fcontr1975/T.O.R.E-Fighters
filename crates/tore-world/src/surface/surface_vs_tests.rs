//! Surface against surface (John, 2026-10-11: "If units are in range of
//! opposing sides they should shoot at each other"): what a weapon aims at by
//! its record's target flags, which kind a unit prefers, line of sight, the
//! anti-ship missile, the column's halt and determinism. On synthetic units
//! with the retail records' numbers; no retail data.
use super::*;
use crate::surface::movement;

const RED_TANK: u32 = 0x5000_0001;
const BLUE_TANK: u32 = 0x4000_0011;
const BLUE_TRUCK: u32 = 0x4000_0012;
const RED_SHIP: u32 = 0x5000_0002;
const BLUE_SHIP: u32 = 0x4000_0013;

/// The retail T-72 record (the T-72, T-80 and T-90 gun): the air and the
/// surface target flags, fire zone 50 to 5,000 ft below 3,000 ft.
fn t72() -> Weapon {
    let mut w = gun(
        "T72.JT",
        0x300c0,
        0,
        [
            zone(32767, 0, 20_000, i32::MIN, i32::MAX),
            zone(8190, 50, 5_000, i32::MIN, 3_000),
        ],
        5866,
    );
    w.burst.game_rounds_in_burst = 1;
    w.burst.game_burst_t = 1;
    w.burst.reload_t = 16;
    w.damage.by_class = [200; 5];
    w
}

/// The retail SS-N-9 record: the surface target flag only, supported radar.
fn ssn9() -> Weapon {
    let mut w = missile(
        "SSN9.JT",
        3,
        [
            zone(32767, 0, 250_000, i32::MIN, i32::MAX),
            zone(8190, 3_000, 120_000, i32::MIN, i32::MAX),
        ],
        32,
        1440,
    );
    w.flags = 0x22365;
    w.guidance.track_t = 12;
    w.damage.by_class = [170; 5];
    w
}

impl Placed {
    fn on(mut self, side: Side) -> Self {
        self.unit.side = side;
        self.arms.side = side;
        self
    }
}

fn tank(id: u32, x: f64, z: f64, side: Side) -> Placed {
    placed(id, "T72.NT", 0x0400, x, z, 1)
        .with_gun(t72(), "T72", arc(0, FREE, FREE), 0)
        .on(side)
}

fn passive(id: u32, resource: &str, class: u16, x: f64, z: f64, side: Side) -> Placed {
    let mut p = placed(id, resource, class, x, z, 1).on(side);
    p.unit.kind = UnitKind::Passive;
    p
}

/// The rounds `unit` fired at `target`.
fn rounds_at(f: &Fixture, unit: u32, target: u32) -> u32 {
    f.fired
        .iter()
        .map(|(_, t)| match t {
            Trace::Shot {
                unit: u,
                target: aimed,
                rounds,
                ..
            } if u.0 == unit && *aimed == target => *rounds,
            _ => 0,
        })
        .sum()
}

/// The targets `unit`'s first controller took, in order.
fn targets_of(f: &Fixture, unit: u32) -> Vec<u32> {
    let mut out: Vec<u32> = f
        .traces
        .iter()
        .filter_map(|(_, t)| match t {
            Trace::Phase {
                unit: u,
                weapon: 0,
                target: Some(target),
                ..
            } if u.0 == unit => Some(*target),
            _ => None,
        })
        .collect();
    out.dedup();
    out
}

#[test]
fn opposing_tanks_in_range_engage_each_other_before_an_aircraft() {
    // A Blue jet low near the Red tank, inside its gun's air zone and below
    // its 20 degree elevation: the tank still takes the Blue tank 3,000 ft
    // off first.
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 3_000., BLUE),
        ],
        Vec::new(),
    );
    f.jet.position = [0., 800., 3_500.];
    // Search at once, 20 s unready preparation, 1 s lock.
    f.run(25 * TPS);
    assert_eq!(targets_of(&f, RED_TANK), vec![BLUE_TANK]);
    assert_eq!(targets_of(&f, BLUE_TANK), vec![RED_TANK]);
    assert!(rounds_at(&f, RED_TANK, BLUE_TANK) >= 1);
    assert!(rounds_at(&f, BLUE_TANK, RED_TANK) >= 1);
    assert_eq!(rounds_at(&f, RED_TANK, JET), 0);
    // A ground target makes no RWR lock or painting.
    assert!(f.state.locks.is_empty() && f.state.painting.is_empty());
    // The Blue tank gone, the Red one turns to the jet.
    f.kill(BLUE_TANK);
    f.run(60 * TPS);
    assert_eq!(targets_of(&f, RED_TANK), vec![BLUE_TANK, JET]);
    assert!(rounds_at(&f, RED_TANK, JET) >= 1);
}

/// The retail BMP-2 record (an IFV's 2A42): air and surface flags, fire zone
/// 50 to 3,000 ft below 3,000 ft. Not a tank gun: the tank rules leave it be.
fn bmp2() -> Weapon {
    let mut w = gun(
        "BMP2.JT",
        0x300c0,
        0,
        [
            zone(32767, 0, 20_000, i32::MIN, i32::MAX),
            zone(8190, 50, 3_000, i32::MIN, 3_000),
        ],
        3666,
    );
    w.burst.game_rounds_in_burst = 2;
    w.burst.game_burst_t = 2;
    w.burst.reload_t = 12;
    w.damage.by_class = [5; 5];
    w
}

/// The first tick `unit` fired at `target`.
fn first_round_at(f: &Fixture, unit: u32, target: u32) -> Option<u64> {
    f.fired
        .iter()
        .find(|(_, t)| {
            matches!(t, Trace::Shot { unit: u, target: aimed, rounds, .. }
                if u.0 == unit && *aimed == target && *rounds > 0)
        })
        .map(|(tick, _)| *tick)
}

/// Tanks against aircraft (John, 2026-10-24: tanks "should be a surprise if
/// they kill you"): a tank's gun elevates to 20 degrees above its hull, so
/// it does not take, track or fire at a jet above that, and spends no
/// round; once the jet is lower in the sky it engages. An IFV's cannon keeps
/// its old rules and fires at the same high jet.
#[test]
fn a_tank_does_not_engage_an_aircraft_above_its_20_degree_elevation() {
    // About 26 degrees above the tank's sight, 2,240 ft away: inside the
    // gun's 5,000 ft reach and 3,000 ft ceiling.
    let high = [0., 1_000., 2_000.];
    let mut f = Fixture::new(vec![tank(RED_TANK, 0., 0., RED)], Vec::new());
    f.jet.position = high;
    f.run(60 * TPS);
    assert!(targets_of(&f, RED_TANK).is_empty(), "never takes the jet");
    assert_eq!(f.rounds(), 0);
    assert_eq!(
        f.unit_state(RED_TANK).mounts[0].loaded,
        f.surface.arsenal.units[0].weapons[0].magazine
    );
    // Awake, looking: the jet is in its detection range.
    assert_eq!(f.phase(RED_TANK), Phase::Search);
    // Lower in the sky (about 14 degrees): taken and fired at.
    f.jet.position = [0., 1_000., 4_000.];
    f.run(40 * TPS);
    assert_eq!(targets_of(&f, RED_TANK), vec![JET]);
    assert!(rounds_at(&f, RED_TANK, JET) >= 1);
    // The same high jet draws an IFV's fire.
    let ifv = placed(RED_TANK, "BMP2.NT", 0x0400, 0., 0., 1)
        .with_gun(bmp2(), "BMP2", arc(0, FREE, FREE), 0)
        .on(RED);
    let mut f = Fixture::new(vec![ifv], Vec::new());
    f.jet.position = high;
    f.run(30 * TPS);
    assert_eq!(first_round_at(&f, RED_TANK, JET), Some(21 * TPS));
}

/// A tank lays on an aircraft for 3 s inside its gates before the first
/// round, on top of its 1 s lock; at a ground target it keeps the record's
/// timing.
#[test]
fn a_tank_lays_on_an_aircraft_but_not_on_a_tank() {
    let mut f = Fixture::new(vec![tank(RED_TANK, 0., 0., RED)], Vec::new());
    f.jet.position = [0., 500., 3_000.];
    f.run(30 * TPS);
    // 20 s unready preparation, 1 s lock, 3 s lay.
    let lay = (control::TANK_AIR_LAY_S * TPS as f64) as u64;
    assert_eq!(lay, 3 * TPS);
    assert_eq!(first_round_at(&f, RED_TANK, JET), Some(21 * TPS + lay));
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 3_000., BLUE),
        ],
        Vec::new(),
    );
    f.run(30 * TPS);
    assert_eq!(first_round_at(&f, RED_TANK, BLUE_TANK), Some(21 * TPS));
}

/// The ceiling is in the vehicle's own frame, and the tracking rate is the
/// sight line's turn.
#[test]
fn hull_elevation_and_angular_rate() {
    let level = Basis::new(0., 0., 0.);
    let d: Vector = [0., 1_000., 2_000.];
    let world = d[1].atan2(d[2]).to_degrees();
    assert!((hull_elevation_deg(&level, d) - world).abs() < 1e-9);
    // A hull pitched on a slope: the same line, measured from its deck.
    let pitched = Basis::new(0., 0.2, 0.);
    let deck = attitude::dot(d, pitched.up)
        .atan2(attitude::dot(d, pitched.right).hypot(attitude::dot(d, pitched.forward)));
    assert!((hull_elevation_deg(&pitched, d) - deck.to_degrees()).abs() < 1e-9);
    assert!((hull_elevation_deg(&pitched, d) - world).abs() > 5.);
    // A 450 kt jet crossing 1,000 ft away: about 43.5 degrees a second.
    let rate = angular_rate_deg_s([0., 0., 1_000.], [759.5, 0., 0.], [0.; 3]);
    assert!((rate - 0.7595_f64.to_degrees()).abs() < 1e-6);
    // Head on, or hovering: the line does not turn.
    assert!(angular_rate_deg_s([0., 0., 1_000.], [0., 0., -759.5], [0.; 3]).abs() < 1e-9);
    assert_eq!(angular_rate_deg_s([0., 300., 3_000.], [0.; 3], [0.; 3]), 0.);
}

#[test]
fn air_defence_keeps_aircraft_first_and_a_land_aaa_gun_fires_at_the_ground_after_them() {
    // A Red ZSU-23 (a land AAA gun, air target flag only) beside a Blue tank:
    // John's ruling lets it shoot at the ground ("If it can shoot at ground
    // targets let it"), after any aircraft in reach.
    let zsu = || {
        placed(ZSU_ID, "ZSU23.NT", 0x0800, 0., 0., 1)
            .with_gun(zsu23(), "ZSU23", arc(0, FREE, [-10., 90.]), 0)
            .radar(false, None)
    };
    let mut f = Fixture::new(vec![zsu(), tank(BLUE_TANK, 0., 2_000., BLUE)], Vec::new());
    // No hostile aircraft: the tank engages the gun and the gun the tank.
    f.jet.side = RED;
    f.run(30 * TPS);
    assert!(
        rounds_at(&f, ZSU_ID, BLUE_TANK) >= 1,
        "the gun fires at the tank"
    );
    assert!(rounds_at(&f, BLUE_TANK, ZSU_ID) >= 1);
    // A ground target makes no RWR lock or painting.
    assert!(f.state.locks.is_empty() && f.state.painting.is_empty());

    // A Blue jet in reach comes first: the gun fires at it and not at the
    // tank while it stays.
    let mut f = Fixture::new(vec![zsu(), tank(BLUE_TANK, 0., 2_000., BLUE)], Vec::new());
    f.jet.side = BLUE;
    f.jet.position = [0., 2_000., 3_000.];
    f.run(30 * TPS);
    assert!(rounds_at(&f, ZSU_ID, JET) > 0);
    assert_eq!(rounds_at(&f, ZSU_ID, BLUE_TANK), 0);
    assert_eq!(targets_of(&f, ZSU_ID), vec![JET]);

    // The same gun on a ship stays in the air.
    let (ship_gun, _) =
        WeaponArms::gun(zsu23(), "ZSU23", arc(0, FREE, FREE), 1, true, GUN_NPC, 0).unwrap();
    assert!(!ship_gun.aims_at_any_surface());
    let (land_gun, _) =
        WeaponArms::gun(zsu23(), "ZSU23", arc(0, FREE, FREE), 1, false, GUN_NPC, 0).unwrap();
    assert!(land_gun.aims_at_any_surface());
}

#[test]
fn ground_units_aim_only_at_sided_units_they_can_see_and_reach() {
    // Beyond reach: 6,000 ft is past the gun's 5,000 ft zone.
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 6_000., BLUE),
        ],
        Vec::new(),
    );
    f.jet.side = RED;
    f.run(40 * TPS);
    assert_eq!(f.rounds(), 0);

    // Behind a ridge.
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 3_000., BLUE),
        ],
        Vec::new(),
    );
    f.jet.side = RED;
    f.ridge = Some(Ridge {
        x: (-1e9, 1e9),
        z: (1_200., 1_800.),
        height: 60.,
    });
    f.run(40 * TPS);
    assert_eq!(f.rounds(), 0, "a ridge between hides them");
    f.ridge = None;
    f.run(30 * TPS);
    assert!(rounds_at(&f, RED_TANK, BLUE_TANK) >= 1);

    // A truck with no side, a building of the other side and a unit the
    // scene cannot draw are never aimed at; a Blue truck is.
    let neutral = passive(0x4000_0020, "TRUCK.NT", 0x0200, 0., 1_000., NO_SIDE);
    let mut building = passive(0x4000_0021, "BNK6.OT", 0x0100, 0., 1_200., BLUE);
    building.unit.kind = UnitKind::Structure;
    let mut unseen = passive(0x4000_0022, "TRUCK.NT", 0x0200, 0., 1_400., BLUE);
    unseen.unit.in_scene = false;
    let mut f = Fixture::new(
        vec![tank(RED_TANK, 0., 0., RED), neutral, building, unseen],
        Vec::new(),
    );
    f.jet.side = RED;
    f.run(40 * TPS);
    assert_eq!(f.rounds(), 0);
    let blue_truck = passive(BLUE_TRUCK, "TRUCK.NT", 0x0200, 0., 2_000., BLUE);
    let mut f = Fixture::new(vec![tank(RED_TANK, 0., 0., RED), blue_truck], Vec::new());
    f.jet.side = RED;
    f.run(25 * TPS);
    assert!(rounds_at(&f, RED_TANK, BLUE_TRUCK) >= 1);
}

#[test]
fn the_ss_n9_aims_at_ships_only_and_its_radar_supports_it() {
    let sarancha = placed(RED_SHIP, "SARAN.NT", 0x2000, 0., 0., 1)
        .with_missile(ssn9(), &[(arc(2, FREE, [0., 45.]), 2)])
        .radar(false, None);
    assert_eq!(
        sarancha.arms.radar,
        Some(emitters::RadarRule::Detection {
            range: 0.,
            surface: 250_000.
        }),
        "an air-blind radar that searches for ships"
    );
    let ship = passive(BLUE_SHIP, "CARGO.NT", 0x2000, 0., 60_000., BLUE);
    let land = passive(BLUE_TRUCK, "TRUCK.NT", 0x0200, 0., 30_000., BLUE);
    let mut f = Fixture::new(vec![sarancha, land, ship], Vec::new());
    f.jet.side = RED;
    f.run(TPS);
    assert!(
        f.emitting(RED_SHIP),
        "a Blue ship in range turns the radar on"
    );
    // Search at once, 36 s unready preparation, 3 s lock.
    f.run(40 * TPS);
    assert_eq!(rounds_at(&f, RED_SHIP, BLUE_SHIP), 1);
    assert_eq!(rounds_at(&f, RED_SHIP, BLUE_TRUCK), 0);
    let support = f.supports.iter().find(|s| s.owner == RED_SHIP).unwrap();
    assert!(support.supported && support.radar_emitting);
    assert_eq!(support.observation.unwrap().id, BLUE_SHIP);
    // No ship in range: the land target alone never draws a missile.
    let sarancha = placed(RED_SHIP, "SARAN.NT", 0x2000, 0., 0., 1)
        .with_missile(ssn9(), &[(arc(2, FREE, [0., 45.]), 1)])
        .radar(false, None);
    let land = passive(BLUE_TRUCK, "TRUCK.NT", 0x0200, 0., 30_000., BLUE);
    let mut f = Fixture::new(vec![sarancha, land], Vec::new());
    f.jet.side = RED;
    f.run(60 * TPS);
    assert_eq!(f.rounds(), 0);
    assert!(!f.emitting(RED_SHIP));
}

#[test]
fn a_unit_engaging_a_surface_target_holds_and_one_engaging_aircraft_does_not() {
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 3_000., BLUE),
        ],
        Vec::new(),
    );
    f.jet.side = RED;
    assert!(!movement::engaging_surface(f.unit_state(RED_TANK)));
    f.run(TPS);
    // Preparing on the Blue tank does not count yet (review M2): only a
    // target inside the gates, or a burst started.
    assert_eq!(f.phase(RED_TANK), Phase::Prepare);
    assert!(!movement::engaging_surface(f.unit_state(RED_TANK)));
    // Past the preparation, inside the fire zone: it fights.
    f.run(20 * TPS);
    assert!(f.phase(RED_TANK).locked());
    assert!(movement::engaging_surface(f.unit_state(RED_TANK)));
    f.kill(BLUE_TANK);
    f.run(TPS);
    assert!(!movement::engaging_surface(f.unit_state(RED_TANK)));
    // Fighting the jet alone does not stop it (the jet below the gun's 20
    // degree elevation).
    f.jet.side = BLUE;
    f.jet.position = [0., 800., 3_500.];
    f.run(25 * TPS);
    assert!(f.phase(RED_TANK).locked());
    assert!(!movement::engaging_surface(f.unit_state(RED_TANK)));
}

/// Review M2: a routed tank that sees an enemy tank in its detection zone
/// (20,000 ft) but out of its fire zone (5,000 ft) keeps driving, closes, and
/// only then halts and fires. Before the fix it halted on sight, 12,000 ft
/// out, and never fired.
#[test]
fn a_column_closes_on_a_tank_it_sees_out_of_reach_then_halts_and_fires() {
    use movement::{Course, Leg, Mover, Rig};
    let mut f = Fixture::new(
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 12_000., BLUE),
        ],
        Vec::new(),
    );
    // The jet is Red's own and out of everyone's way.
    f.jet.side = RED;
    let rig = Rig {
        centre: [0.; 3],
        half: [6., 5., 15.],
    };
    let course = Course {
        legs: vec![Leg {
            to: [0., 20_000.],
            speed: 50.,
        }],
        start: [0, 0, 0],
        start_angles: [0, 0, 0],
        turn_rate: 15f64.to_radians(),
        top_speed: 50.,
        acceleration: 5.,
        ship: false,
        rig: Some(rig),
        lane: 0.,
        footing: Default::default(),
    };
    let flat = |_: f64, _: f64| 0.;
    let mut mover = Mover::start(&course, 0.);
    let mut seen_driving = None;
    let mut halted = None;
    for tick in 0..240 * TPS {
        let hold = movement::engaging_surface(f.unit_state(RED_TANK));
        mover.step_held(&course, true, hold, flat);
        f.state.unit_mut(UnitId(RED_TANK)).unwrap().mover = Some(mover);
        f.live
            .move_ground_target(RED_TANK, mover.bounds(&rig), mover.velocity(), true);
        f.step();
        let gap = 12_000. - mover.position()[2];
        let target = f.unit_state(RED_TANK).engagers[0].controller.target();
        if target == Some(BLUE_TANK) && gap > 6_000. && mover.speed_feet() > 40. {
            seen_driving.get_or_insert(gap);
        }
        if halted.is_none() && mover.speed_feet() == 0. && tick > TPS {
            halted = Some(gap);
        }
    }
    let seen = seen_driving.expect("it drives on with the tank in sight");
    assert!(seen > 6_000., "{seen}");
    let halted = halted.expect("it halts");
    assert!(
        halted <= 5_000. && halted > 1_000.,
        "halted {halted} ft out"
    );
    assert!(rounds_at(&f, RED_TANK, BLUE_TANK) > 0);
    // It stays halted while it fights.
    assert_eq!(mover.speed_feet(), 0.);
    assert!(movement::engaging_surface(f.unit_state(RED_TANK)));
}

#[test]
fn a_surface_duel_runs_the_same_twice() {
    let run = || {
        let mut f = Fixture::new(
            vec![
                tank(RED_TANK, 0., 0., RED),
                tank(RED_TANK + 1, 300., 0., RED),
                tank(BLUE_TANK, 0., 3_000., BLUE),
                passive(BLUE_TRUCK, "TRUCK.NT", 0x0200, 400., 2_600., BLUE),
            ],
            Vec::new(),
        );
        f.jet.position = [0., 1_500., 1_500.];
        f.run(90 * TPS);
        (f.traces, f.state)
    };
    let (a, state_a) = run();
    let (b, state_b) = run();
    assert!(a.iter().any(|(_, t)| matches!(t, Trace::Shot { .. })));
    assert_eq!(a, b);
    assert_eq!(state_a, state_b);
}

#[test]
fn a_surface_duel_survives_a_checkpoint() {
    let units = || {
        vec![
            tank(RED_TANK, 0., 0., RED),
            tank(BLUE_TANK, 0., 3_000., BLUE),
            passive(BLUE_TRUCK, "TRUCK.NT", 0x0200, 400., 2_600., BLUE),
        ]
    };
    let mut f = Fixture::new(units(), Vec::new());
    f.jet.position = [0., 1_500., 1_500.];
    // Past the preparation and the first rounds: both tanks hold a ground
    // target in the middle of their engagement.
    f.run(25 * TPS);
    assert!(movement::engaging_surface(f.unit_state(RED_TANK)));
    let copy = round_trip(&f.state, &Models::default()).expect("it round-trips");
    assert_eq!(copy, f.state);
    let mut resumed = Fixture::new(units(), Vec::new());
    resumed.jet = f.jet;
    resumed.state = copy;
    resumed.live = f.live.clone();
    resumed.tick = f.tick;
    let fired = f.traces.len();
    f.run(40 * TPS);
    resumed.run(40 * TPS);
    assert!(
        f.traces[fired..]
            .iter()
            .any(|(_, t)| matches!(t, Trace::Shot { .. }))
    );
    assert_eq!(resumed.traces, f.traces[fired..]);
    assert_eq!(resumed.state, f.state);
}
