//! Splash damage for every weapon (slice X1): the falloff, the ground share,
//! who is spared, what is reached and who is credited. Also the launch grace
//! (slice nr-sd): a round inside its own launcher's hit volume harms neither
//! the launcher nor, by its burst, anyone else's claim on it.
use super::super::tests::{fixture, target};
use super::super::*;
use super::*;
use crate::airport::OrientedBox;
use tore_formats::weapons::Movement;

const BLUE: Side = Side(1);
const RED: Side = Side(2);
/// An AI shooter's actor id, as the AI aircraft rounds carry.
const SHOOTER: u32 = 9;

fn far() -> Launcher {
    Launcher {
        position: [90_000., 30_000., -90_000.],
        basis: Basis::new(0., 0., 0.),
        speed_fps: 300.,
        velocity: [0., 0., 300.],
        bay_ready: true,
        radar_power: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: sensors::Controls::default(),
    }
}
fn run_from(s: &mut State, ticks: usize, own: Launcher) -> Vec<Event> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        events.extend(s.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: own,
            }],
            |_, _| 0.,
        ));
    }
    events
}
fn scene() -> State {
    let mut s = fixture(false);
    s.targets.clear();
    s
}
fn aircraft(id: u32, position: Vector, side: Side) -> Target {
    let mut t = target(id, position, 1000, 0x80);
    t.side = side;
    t
}
/// A parked aircraft as slice PA1 registers one: an aircraft target row with
/// the Surface role, on the ground.
fn parked(id: u32, position: Vector) -> Target {
    let mut t = target(id, position, 1000, 0x80);
    t.role = TargetRole::Surface;
    t.airborne = false;
    t.on_ground = true;
    t
}
fn ground_object(s: &mut State, id: u32, center: Vector, hp: i32, side: Side) {
    s.add_ground_target(
        id,
        OrientedBox {
            center,
            half: [15., 8., 15.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
        },
        hp,
        0x800,
        side,
    )
    .unwrap();
}
fn constant(speed: i16) -> Movement {
    Movement {
        minimum_speed: speed,
        corner_speed: speed,
        maximum_speed: speed,
        acceleration: 0,
        deceleration: 0,
        initial_speed: speed,
        final_speed: speed,
        launch_retard: 0,
        ignite_t: 0,
        fuel_t: 0,
        remove_t: 60,
        powered_turn_rate: 0,
        unpowered_turn_rate: 0,
        performance_at_0: 100,
        performance_at_20: 100,
        cruise: [0; 4],
        jink: [0; 3],
    }
}
/// The retail AIM-7M's damage, fuze and collateral (750 ft at 35 percent),
/// flying straight at a constant 1,000 ft/s so a test is short.
fn sparrow() -> Weapon {
    let mut w = fixture(false).own().configuration().stations[0]
        .weapon
        .clone();
    w.source = "AIM7.JT".into();
    w.flags = 0x1;
    // This constant-speed fixture still represents a radar missile. A zero
    // seeker signature plus no fuel describes an unpowered coordinate bomb.
    w.seeker.signature = 3;
    w.movement = constant(1000);
    w.damage.by_class = [170, 17, 51, 34, 170];
    w.damage.fuze_radius = 100;
    w.damage.fuze_arm_t = 0;
    w.damage.collateral_radius = 750;
    w.damage.collateral_percent = 35;
    w.effects.object_explosion = 30;
    w.effects.land_explosion = 21;
    w
}
/// The retail Mk 84's damage and collateral (1,000 ft at 100 percent),
/// falling straight down at 500 ft/s.
fn mk84() -> Weapon {
    let mut w = sparrow();
    w.source = "MK84.JT".into();
    w.flags = 0x10;
    w.movement = constant(500);
    w.damage.by_class = [400; 5];
    // (The record's 300 ft fuze would burst it on an aircraft row it
    // passes; these tests drop it clear of aircraft onto the ground.)
    w.damage.fuze_radius = 0;
    w.damage.collateral_radius = 1000;
    w.damage.collateral_percent = 100;
    w.effects.land_explosion = 35;
    w.effects.crater_size = 18;
    w
}
fn round(id: u32, owner: u32, w: &Weapon, from: Vector, toward: Vector) -> Projectile {
    let direction = crate::attitude::unit(sub(toward, from));
    Projectile {
        id,
        owner,
        weapon: Some(w.clone()),
        guidance: None,
        motion: None,
        guidance_ticks: None,
        age: 0,
        incoming: None,
        station: 0,
        position: from,
        previous: from,
        direction,
        speed_f8: i32::from(w.movement.initial_speed) * 256,
        launched_t: 0,
        target: None,
        fall: FallState::default(),
        gun_round: None,
        tracer: false,
    }
}
fn hp(s: &State, id: u32) -> i32 {
    s.targets.iter().find(|t| t.id == id).unwrap().hp
}
fn burst(w: &Weapon, detonation: Detonation) -> Burst {
    Burst::new(
        &round(1, SHOOTER, w, [0.; 3], [0., 0., 1.]),
        w,
        Shooter {
            side: NO_SIDE,
            surface: None,
            ownship: false,
        },
        [0.; 3],
        detonation,
        None,
        false,
    )
}

#[test]
fn the_share_falls_off_in_a_straight_line_to_the_radius_edge() {
    let air = burst(&sparrow(), Detonation::Air);
    assert_eq!(air.percent_at(0.), 35);
    assert_eq!(air.percent_at(-5.), 35);
    // 35 * 375 / 750 = 17.5, whole percent 17 off: 18 left.
    assert_eq!(air.percent_at(375.9), 18);
    assert_eq!(air.percent_at(749.), 1);
    assert_eq!(air.percent_at(750.), 0);
    assert_eq!(air.percent_at(5_000.), 0);
    // On the ground a missile passes half its share on, a bomb 70 percent.
    let ground = burst(&sparrow(), Detonation::Ground);
    assert_eq!(ground.percent_at(0.), 17);
    assert_eq!(ground.percent_at(375.), 9);
    let bomb = burst(&mk84(), Detonation::Ground);
    assert_eq!(bomb.percent_at(0.), 70);
    assert_eq!(bomb.percent_at(500.), 35);
    assert_eq!(bomb.percent_at(999.), 0);
    assert_eq!(burst(&mk84(), Detonation::Air).percent_at(250.), 75);
    assert_eq!(ground_share(100), 70);
    assert_eq!(ground_share(35), 50);
    // A record without collateral reaches no one.
    let mut gun = sparrow();
    gun.damage.collateral_radius = 0;
    assert!(!burst(&gun, Detonation::Air).collateral());
    assert_eq!(burst(&gun, Detonation::Air).percent_at(0.), 0);
}

#[test]
fn distance_is_measured_to_the_target_surface() {
    let bounds = OrientedBox {
        center: [0., 10., 0.],
        half: [15., 8., 30.],
        heading: 0.,
        pitch: 0.,
        bank: 0.,
    };
    assert_eq!(box_distance(&bounds, [5., 10., 5.]), 0.);
    assert!((box_distance(&bounds, [115., 10., 0.]) - 100.).abs() < 1e-9);
    assert!((box_distance(&bounds, [0., 10., 130.]) - 100.).abs() < 1e-9);
    // Past a corner, to the corner.
    let corner = box_distance(&bounds, [18., 22., 34.]);
    assert!((corner - (9_f64 + 16. + 16.).sqrt()).abs() < 1e-9);
    // Turned a quarter, the long side faces east.
    let turned = OrientedBox {
        heading: std::f64::consts::FRAC_PI_2,
        ..bounds
    };
    assert!((box_distance(&turned, [130., 10., 0.]) - 100.).abs() < 1e-6);
    assert!((sphere_distance([0.; 3], 20., [0., 0., 120.]) - 100.).abs() < 1e-12);
    assert_eq!(sphere_distance([0.; 3], 20., [0., 0., 10.]), 0.);
}

/// An AI shooter's Sparrow flying at aircraft 1 from the south; aircraft 2
/// flies 300 ft beside it, aircraft 3 900 ft beside it. Returns the events.
fn sparrow_at_a_formation(s: &mut State, wing_side: Side) -> Vec<Event> {
    s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
    s.targets.push(aircraft(1, [0., 5000., 0.], BLUE));
    s.targets.push(aircraft(2, [300., 5000., 0.], wing_side));
    s.targets.push(aircraft(3, [900., 5000., 0.], wing_side));
    s.projectiles.push(round(
        70,
        SHOOTER,
        &sparrow(),
        [0., 5000., -600.],
        [0., 5000., 0.],
    ));
    run_from(s, 120, far())
}

#[test]
fn missile_splash_hurts_a_wingman_in_formation_and_credits_the_shooter() {
    let mut s = scene();
    let events = sparrow_at_a_formation(&mut s, BLUE);
    // The struck aircraft takes the direct hit only.
    assert_eq!(hp(&s, 1), 1000 - 170);
    // The fuze fires 120 ft short of aircraft 1 (20 ft body, 100 ft fuze):
    // the wingman's surface is sqrt(300^2 + 120^2) - 20 = 303 ft away, so
    // 35 - 14 = 21 percent of 170.
    assert_eq!(hp(&s, 2), 1000 - 170 * 21 / 100);
    assert_eq!(hp(&s, 3), 1000, "beyond 750 ft");
    assert_eq!(events.iter().filter(|e| **e == Event::Hit(2)).count(), 1);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Jolt(Jolt { target: 2, strength, .. }) if (*strength - 0.35).abs() < 1e-12
    )));
    let strikes = s.take_strikes();
    assert!(strikes.contains(&Strike {
        owner: SHOOTER,
        victim: 2,
        weapon_flags: 0x1,
        destroyed: false,
        amount: 35,
    }));
    assert_eq!(s.history.last().unwrap().target, 2);
    assert!(s.ledger.kills().is_empty());
}

#[test]
fn own_missile_blast_is_immune_but_own_bomb_damage_is_uncredited() {
    for setting in [FriendlyFire::On, FriendlyFire::Off] {
        let mut s = scene();
        s.friendly_fire = setting;
        // The shooter flies 200 ft from the burst.
        s.targets.push(aircraft(SHOOTER, [200., 5000., 0.], RED));
        s.targets.push(aircraft(1, [0., 5000., 0.], BLUE));
        s.projectiles.push(round(
            70,
            SHOOTER,
            &sparrow(),
            [0., 5000., -600.],
            [0., 5000., 0.],
        ));
        run_from(&mut s, 120, far());
        assert_eq!(hp(&s, 1), 830, "{setting:?}");
        assert_eq!(
            hp(&s, SHOOTER),
            1000,
            "{setting:?}: permanent missile immunity"
        );
        assert!(s.ledger.kills().is_empty());
        assert!(s.take_strikes().iter().all(|k| k.victim != SHOOTER));
    }
    // The player's own bomb dropped too low: damage, and no one credited.
    let mut s = scene();
    s.projectiles
        .push(round(70, 0, &mk84(), [0., 60., 0.], [0., 0., 0.]));
    let low = Launcher {
        position: [0., 300., 50.],
        ..far()
    };
    let before = s.own().hp;
    let events = run_from(&mut s, 60, low);
    assert!(s.own().hp < before);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 0, .. }))
    );
    assert!(s.ledger.kills().iter().all(|k| k.owner != 0));
    assert!(s.take_strikes().iter().all(|k| k.owner != 0));
}

#[test]
fn friendly_fire_off_spares_the_shooters_side_from_splash() {
    for (setting, wing_side, hurt) in [
        (FriendlyFire::On, BLUE, true),
        (FriendlyFire::On, RED, true),
        (FriendlyFire::Off, BLUE, true),
        (FriendlyFire::Off, RED, false),
    ] {
        let mut s = scene();
        s.friendly_fire = setting;
        sparrow_at_a_formation(&mut s, wing_side);
        assert_eq!(hp(&s, 1), 830, "{setting:?} {wing_side:?}");
        assert_eq!(hp(&s, 2) < 1000, hurt, "{setting:?} {wing_side:?}");
    }
    // Friendly fire off spares the side's ground objects and parked
    // aircraft too.
    for (side, hurt) in [(RED, false), (BLUE, true), (NO_SIDE, true)] {
        let mut s = scene();
        s.friendly_fire = FriendlyFire::Off;
        s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
        ground_object(&mut s, 0x4000_0001, [200., 8., 0.], 1000, side);
        let mut plane = parked(0x5000_0001, [0., 8., 300.]);
        plane.side = side;
        s.targets.push(plane);
        s.projectiles
            .push(round(70, SHOOTER, &mk84(), [0., 60., 0.], [0., 0., 0.]));
        run_from(&mut s, 60, far());
        assert_eq!(hp(&s, 0x4000_0001) < 1000, hurt, "{side:?}");
        assert_eq!(hp(&s, 0x5000_0001) < 1000, hurt, "{side:?}");
    }
}

#[test]
fn a_bomb_on_the_ground_hurts_ground_objects_and_parked_aircraft() {
    let mut s = scene();
    s.targets.push(aircraft(SHOOTER, [0., 3000., -3000.], RED));
    // A vehicle 200 ft east of the impact: its near side is 185 ft away.
    ground_object(&mut s, 0x4000_0001, [200., 8., 0.], 1000, BLUE);
    // A parked aircraft 400 ft north: its body's surface is 380 ft away.
    s.targets.push(parked(0x5000_0001, [0., 0., 400.]));
    // A building beyond the radius.
    ground_object(&mut s, 0x4000_0002, [1100., 8., 0.], 1000, BLUE);
    s.projectiles
        .push(round(70, SHOOTER, &mk84(), [0., 60., 0.], [0., 0., 0.]));
    let events = run_from(&mut s, 60, far());
    assert!(events.contains(&Event::Ground));
    // On the ground the Mk 84 passes on 70 percent of its share:
    // (100 - 18) * 70 / 100 = 57 percent, and (100 - 38) * 70 / 100 = 43.
    assert_eq!(hp(&s, 0x4000_0001), 1000 - 400 * 57 / 100);
    assert_eq!(hp(&s, 0x5000_0001), 1000 - 400 * 43 / 100);
    assert_eq!(hp(&s, 0x4000_0002), 1000);
    // No jolt for things on the ground that are not aircraft rows.
    assert!(!events.iter().any(|e| matches!(
        e,
        Event::Jolt(Jolt {
            target: 0x4000_0001,
            ..
        })
    )));
}

#[test]
fn a_surface_units_splash_reaches_aircraft_and_the_other_sides_ground_units_only() {
    let mut s = scene();
    ground_object(&mut s, 0x5000_0009, [0., 8., -5000.], 1000, RED);
    // A parked aircraft with no side, a Blue vehicle and a Red one beside
    // the burst.
    s.targets.push(parked(0x5000_0001, [0., 0., 300.]));
    ground_object(&mut s, 0x4000_0001, [200., 8., 0.], 1000, BLUE);
    ground_object(&mut s, 0x4000_0002, [-200., 8., 0.], 1000, RED);
    s.targets.push(aircraft(1, [0., 200., 300.], BLUE));
    let mut sam = sparrow();
    sam.source = "SA6.JT".into();
    s.projectiles
        .push(round(70, 0x5000_0009, &sam, [0., 60., 0.], [0., 0., 0.]));
    s.surface_rounds.insert(
        70,
        SurfaceRound {
            end_tick: None,
            flak: false,
        },
    );
    run_from(&mut s, 60, far());
    // Neutral scenery and the shooter's own side are spared, whatever the
    // friendly fire setting (it is on here).
    assert_eq!(hp(&s, 0x5000_0001), 1000);
    assert_eq!(hp(&s, 0x4000_0002), 1000);
    // The Blue vehicle's near side is 185 ft from the ground burst: half of
    // (35 - 8) percent, 13 percent of the record's 170.
    assert_eq!(hp(&s, 0x4000_0001), 1000 - 170 * 13 / 100);
    // The aircraft overhead: its surface is sqrt(200^2 + 300^2) - 20 = 340
    // ft from the ground burst, so half of (35 - 15) percent.
    assert_eq!(hp(&s, 1), 1000 - 170 * 10 / 100);
    // The unit that fired is the Blue vehicle's last attacker.
    assert_eq!(
        s.ledger.credit(0x4000_0001).map(|kill| kill.owner),
        Some(0x5000_0009)
    );
}

#[test]
fn a_collateral_kill_is_credited_to_its_shooter() {
    // The player's bomb lands beside a fuel truck and an enemy fighter
    // taxiing past.
    let mut s = scene();
    let truck = 0x5000_0002;
    ground_object(&mut s, truck, [100., 8., 0.], 50, RED);
    assert!(s.set_ground_look(
        truck,
        GroundLook {
            explosion: 21,
            crater: 6,
            burns: false,
            lasting: false,
            pavement: false,
        }
    ));
    let mut taxiing = aircraft(4, [0., 10., 150.], RED);
    taxiing.hp = 20;
    s.targets.push(taxiing);
    s.projectiles
        .push(round(70, 0, &mk84(), [0., 60., 0.], [0., 0., 0.]));
    let events = run_from(&mut s, 60, far());
    assert_eq!(hp(&s, truck), 0);
    assert_eq!(hp(&s, 4), 0);
    let kills = s.ledger.kills();
    assert!(kills.contains(&Kill {
        owner: 0,
        victim: truck,
        category: 0x800,
        aircraft: false,
    }));
    assert!(kills.contains(&Kill {
        owner: 0,
        victim: 4,
        category: 0x80,
        aircraft: true,
    }));
    assert!(events.contains(&Event::Destroyed(truck)));
    assert!(events.contains(&Event::Destroyed(4)));
    let strikes = s.take_strikes();
    assert!(
        strikes
            .iter()
            .any(|k| k.owner == 0 && k.victim == truck && k.destroyed && k.amount == 50)
    );
    // Both kills count on the player's score; the bomb itself struck nothing.
    assert_eq!(s.own().kills, 2);
    // The truck explodes as its record says, where it stands, and craters.
    assert!(s.effects.iter().any(|e| e.kind == EffectKind::Destroyed
        && e.blast.is_some_and(|b| (21..=23).contains(&b))
        && (e.position[1] - 0.).abs() < 1e-9));
    assert!(
        s.marks
            .iter()
            .any(|m| m.kind == crate::combat::blast::MarkKind::Crater(6))
    );
}

#[test]
fn an_ai_missile_splash_kill_of_a_wingman_goes_to_the_ai_shooter() {
    let mut s = scene();
    s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
    s.targets.push(aircraft(1, [0., 5000., 0.], BLUE));
    let mut wingman = aircraft(2, [150., 5000., 0.], BLUE);
    wingman.hp = 10;
    s.targets.push(wingman);
    s.projectiles.push(round(
        70,
        SHOOTER,
        &sparrow(),
        [0., 5000., -600.],
        [0., 5000., 0.],
    ));
    let events = run_from(&mut s, 120, far());
    assert_eq!(hp(&s, 2), 0);
    assert_eq!(
        s.ledger.kills(),
        &[Kill {
            owner: SHOOTER,
            victim: 2,
            category: 0x80,
            aircraft: true,
        }]
    );
    assert!(events.contains(&Event::Destroyed(2)));
    // An AI shooter has no ownship score.
    assert_eq!(s.own().kills, 0);
}

#[test]
fn an_aircraft_missile_splash_reaches_the_player() {
    // An enemy's missile strikes an aircraft 400 ft ahead of the player.
    let mut s = scene();
    let own = Launcher {
        position: [0., 5000., 400.],
        ..far()
    };
    s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
    s.targets.push(aircraft(1, [0., 5000., 0.], BLUE));
    s.projectiles.push(round(
        70,
        SHOOTER,
        &sparrow(),
        [0., 5000., -600.],
        [0., 5000., 0.],
    ));
    let before = s.own().hp;
    let events = run_from(&mut s, 120, own);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::OwnshipDamaged { aircraft: 0, .. }))
    );
    assert!(s.own().hp < before);
    // The blast is no hit on the player, so it makes the shooter no last
    // attacker: a later loss of the player credits nobody.
    assert_eq!(s.ledger.credit(0), None);
}

// ---- Launch grace (John, 2026-10-13) ----------------------------------

/// Where the launcher sits in the launch grace tests.
const LAUNCH_AT: Vector = [0., 5000., 0.];

fn at(position: Vector) -> Launcher {
    Launcher {
        position,
        speed_fps: 0.,
        velocity: [0.; 3],
        ..far()
    }
}
/// A scene with one launcher at [`LAUNCH_AT`]: the player (`ai` false, aircraft
/// 0) or an AI aircraft (`ai` true, [`SHOOTER`]). Returns the scene, the
/// ownship's launcher pose (the player's, or one far away) and the launcher's
/// aircraft id.
fn launch_scene(ai: bool) -> (State, Launcher, u32) {
    let mut s = scene();
    if ai {
        s.targets.push(aircraft(SHOOTER, LAUNCH_AT, RED));
        (s, far(), SHOOTER)
    } else {
        (s, at(LAUNCH_AT), 0)
    }
}
fn launcher_hp(s: &State, id: u32) -> i32 {
    if id == 0 { s.own().hp } else { hp(s, id) }
}
/// Steps the scene one tick at a time with the ownship at `own`.
fn tick_once(s: &mut State, own: Launcher) -> Vec<Event> {
    run_from(s, 1, own)
}
fn hurt(events: &[Event], id: u32) -> bool {
    events.iter().any(|e| match e {
        Event::OwnshipDamaged { aircraft, .. } => *aircraft == id,
        Event::Hit(hit) => *hit == id,
        _ => false,
    })
}

#[test]
fn a_store_spawned_inside_its_launchers_box_with_no_separation_does_no_harm() {
    for ai in [false, true] {
        for fuze in [0, 100, 400] {
            for weapon in [sparrow(), mk84()] {
                let (mut s, own, id) = launch_scene(ai);
                let mut w = weapon.clone();
                w.damage.fuze_radius = fuze;
                w.damage.fuze_arm_t = 0;
                // At the launcher's exact centre, heading along its nose.
                s.projectiles.push(round(
                    70,
                    id,
                    &w,
                    LAUNCH_AT,
                    [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 1.],
                ));
                let before = launcher_hp(&s, id);
                let events = run_from(&mut s, 120, own);
                assert_eq!(
                    launcher_hp(&s, id),
                    before,
                    "ai {ai} fuze {fuze} {}: {events:?}",
                    w.source
                );
                assert!(!hurt(&events, id), "ai {ai} fuze {fuze}: {events:?}");
                assert!(s.take_strikes().iter().all(|k| k.victim != id));
            }
        }
    }
}

#[test]
fn a_missile_returning_to_its_launcher_is_always_spared() {
    for ai in [false, true] {
        let (mut s, own, id) = launch_scene(ai);
        let mut w = sparrow();
        w.damage.fuze_radius = 0;
        s.projectiles.push(round(
            70,
            id,
            &w,
            LAUNCH_AT,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 1.],
        ));
        let before = launcher_hp(&s, id);
        // Out of the box (28 ft) and well past it: nothing happens.
        let mut events = Vec::new();
        for _ in 0..300 {
            events.extend(tick_once(&mut s, own));
            let p = &s.projectiles[0];
            if p.position[2] - LAUNCH_AT[2] > 300. {
                break;
            }
        }
        assert!(s.projectiles[0].position[2] - LAUNCH_AT[2] > 300.);
        assert_eq!(launcher_hp(&s, id), before, "ai {ai}: {events:?}");
        // It turns round (a missile that comes back) and flies into the box.
        s.projectiles[0].direction = [0., 0., -1.];
        let mut back = Vec::new();
        for _ in 0..400 {
            back.extend(tick_once(&mut s, own));
            if hurt(&back, id) {
                break;
            }
        }
        assert!(!hurt(&back, id), "ai {ai}: {back:?}");
        assert_eq!(launcher_hp(&s, id), before, "ai {ai}");
    }
}

/// A store that has dropped just out of its launcher's box (128 ft with the
/// 100 ft fuze) and now climbs straight back up through it, as a lofting
/// AIM-54 does when its motor lights under the launcher. `ignite_t` is the
/// record's motor delay in quarter seconds.
fn climbing_back(ai: bool, ignite_t: u16) -> (Vec<Event>, i32, i32) {
    let (mut s, own, id) = launch_scene(ai);
    let mut w = sparrow();
    w.movement.ignite_t = ignite_t;
    let below = [LAUNCH_AT[0], LAUNCH_AT[1] - 130., LAUNCH_AT[2] + 10.];
    let mut p = round(70, id, &w, below, [below[0], below[1] + 1., below[2]]);
    p.launched_t = (s.tick / 30) as u16;
    s.projectiles.push(p);
    let before = launcher_hp(&s, id);
    let events = run_from(&mut s, 60, own);
    let after = launcher_hp(&s, id);
    (events, before, after)
}

#[test]
fn a_drop_launched_store_climbing_back_past_its_launcher_does_no_harm() {
    for ai in [false, true] {
        // Its motor lit a moment ago (2 s delay, the AIM-54's): it is still
        // separating, so it passes its launcher and fuzes on nothing.
        let (events, before, after) = climbing_back(ai, 8);
        let id = if ai { SHOOTER } else { 0 };
        assert_eq!(after, before, "ai {ai}: {events:?}");
        assert!(!hurt(&events, id), "ai {ai}: {events:?}");
        // The same path from a store that lit on the rail is a store that left
        // the box and came back: permanent missile immunity still applies.
        let (events, before, after) = climbing_back(ai, 0);
        assert!(!hurt(&events, id), "ai {ai}: {events:?}");
        assert_eq!(after, before, "ai {ai}");
    }
}

#[test]
fn a_drop_launched_missile_returning_after_separation_is_spared() {
    for ai in [false, true] {
        let (mut s, own, id) = launch_scene(ai);
        let mut w = sparrow();
        w.damage.fuze_radius = 0;
        // Lights a quarter second after release; separated 1.25 s after it.
        w.movement.ignite_t = 1;
        let mut p = round(
            70,
            id,
            &w,
            LAUNCH_AT,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 1.],
        );
        p.launched_t = (s.tick / 30) as u16;
        s.projectiles.push(p);
        let before = launcher_hp(&s, id);
        let events = run_from(&mut s, 180, own);
        assert_eq!(launcher_hp(&s, id), before, "ai {ai}: {events:?}");
        s.projectiles[0].direction = [0., 0., -1.];
        let mut back = Vec::new();
        for _ in 0..400 {
            back.extend(tick_once(&mut s, own));
            if hurt(&back, id) {
                break;
            }
        }
        assert!(!hurt(&back, id), "ai {ai}: {back:?}");
        assert_eq!(launcher_hp(&s, id), before, "ai {ai}");
    }
}

#[test]
fn a_store_bursting_inside_its_launchers_box_does_not_hurt_the_launcher() {
    for ai in [false, true] {
        // Another aircraft overlaps the launcher's box: the store is a hit on
        // it at once, and its splash spares the launcher. 100 ft of fuze
        // puts the burst at the store, inside the launcher's box.
        let (mut s, own, id) = launch_scene(ai);
        // (Overlapping aircraft would otherwise collide and die.)
        s.cheats.ignore_midair_collisions = true;
        s.targets.push(aircraft(
            1,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 10.],
            BLUE,
        ));
        s.projectiles.push(round(
            70,
            id,
            &sparrow(),
            LAUNCH_AT,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 1.],
        ));
        let before = launcher_hp(&s, id);
        let events = run_from(&mut s, 60, own);
        assert!(events.contains(&Event::Hit(1)), "ai {ai}: {events:?}");
        assert_eq!(hp(&s, 1), 1000 - 170, "ai {ai}: still hittable");
        assert_eq!(launcher_hp(&s, id), before, "ai {ai}: {events:?}");
        assert!(!hurt(&events, id), "ai {ai}: {events:?}");
    }
}

#[test]
fn a_missile_burst_outside_the_box_still_spares_the_launcher() {
    for ai in [false, true] {
        // Target 1 is 200 ft ahead: the 100 ft fuze (120 ft with the body)
        // fires the store at 80 ft, 52 ft beyond the launcher's 28 ft box.
        let (mut s, own, id) = launch_scene(ai);
        s.targets.push(aircraft(
            1,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 200.],
            BLUE,
        ));
        s.projectiles.push(round(
            70,
            id,
            &sparrow(),
            LAUNCH_AT,
            [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 1.],
        ));
        let before = launcher_hp(&s, id);
        let events = run_from(&mut s, 120, own);
        assert!(events.contains(&Event::Hit(1)), "ai {ai}: {events:?}");
        assert_eq!(
            launcher_hp(&s, id),
            before,
            "ai {ai}: own missile blast immunity"
        );
    }
}

#[test]
fn ground_bursts_spare_missile_launchers_but_preserve_bomb_damage() {
    // John, 2026-10-13: the grace is for airbursts only. A store that goes
    // off on the terrain or on a ground object or ship gives its launcher the
    // normal ground share, even with the burst inside the 28 ft box.
    for ai in [false, true] {
        for (weapon, on_object) in [
            (sparrow(), false),
            (mk84(), false),
            (sparrow(), true),
            (mk84(), true),
        ] {
            let low = [0., 24., 0.];
            let mut s = scene();
            let (own, id) = if ai {
                s.targets.push(aircraft(SHOOTER, low, RED));
                (far(), SHOOTER)
            } else {
                (at(low), 0)
            };
            if on_object {
                // A truck's box with its top 8 ft under the launcher's centre.
                ground_object(&mut s, 0x4000_0001, [0., 8., 0.], 1000, BLUE);
            }
            let mut w = weapon.clone();
            w.damage.fuze_radius = 0;
            w.damage.fuze_arm_t = 0;
            // The store starts at the launcher's centre, heading straight down.
            s.projectiles
                .push(round(70, id, &w, low, [low[0], low[1] - 1., low[2]]));
            let before = launcher_hp(&s, id);
            let events = run_from(&mut s, 60, own);
            // It went off close enough to be inside the launcher's box.
            assert!(
                (launcher_hp(&s, id) < before) != launcher_immune(&w),
                "ai {ai} {} object {on_object}: {events:?}",
                w.source
            );
            assert!(s.ledger.kills().iter().all(|k| k.victim != id));
        }
    }
}

#[test]
fn the_grace_is_only_for_the_launchers_own_stores() {
    // Another aircraft's missile bursting on a third aircraft inside the
    // player's box still hurts the player.
    let mut s = scene();
    s.cheats.ignore_midair_collisions = true;
    s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
    s.targets.push(aircraft(
        1,
        [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 10.],
        BLUE,
    ));
    s.projectiles.push(round(
        70,
        SHOOTER,
        &sparrow(),
        [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] - 600.],
        [LAUNCH_AT[0], LAUNCH_AT[1], LAUNCH_AT[2] + 10.],
    ));
    let before = s.own().hp;
    let events = run_from(&mut s, 200, at(LAUNCH_AT));
    assert!(events.contains(&Event::Hit(1)), "{events:?}");
    assert!(s.own().hp < before, "{events:?}");
}

#[test]
fn the_box_edge_is_the_aircraft_contact_sphere() {
    let c = [10., 20., 30.];
    assert!(inside_sphere(c, 28., c));
    assert!(inside_sphere(c, 28., [10., 20., 57.9]));
    assert!(!inside_sphere(c, 28., [10., 20., 58.]));
    assert!(!inside_sphere(c, 28., [10., 20., 59.]));
}

#[test]
fn a_flak_burst_knocks_aircraft_half_as_hard() {
    // John, 2026-10-14: flak's push cut by half, its damage unchanged.
    let mut flak = sparrow();
    flak.source = "KS19.JT".into();
    flak.damage.collateral_radius = 400;
    flak.damage.collateral_percent = 35;
    assert!(is_flak(&flak));
    assert_eq!(burst(&flak, Detonation::Air).jolt_share, FLAK_JOLT_SHARE);
    assert_eq!(FLAK_JOLT_SHARE, 0.5);
    assert_eq!(burst(&sparrow(), Detonation::Air).jolt_share, 1.);
    // The share changes the knock only: the splash still passes on 35
    // percent falling to nothing at 400 ft (27 percent at 100 ft).
    assert_eq!(burst(&flak, Detonation::Air).percent_at(100.), 27);
}

#[test]
fn gunship_corrected_damage_reaches_collateral_without_changing_coverage() {
    for raw in [50, 150] {
        let mut weapon = sparrow();
        weapon.damage.by_class = [raw; 5];
        let old = burst(&weapon, Detonation::Ground);
        weapon.damage.by_class = [raw * 2; 5];
        let new = burst(&weapon, Detonation::Ground);
        assert_eq!(old.radius, new.radius);
        for category in [0x100, 0x2000, 0x800] {
            for distance in [0., old.radius / 2., old.radius] {
                let before = old.nominal(category, distance);
                let after = new.nominal(category, distance);
                assert!((after - before * 2).abs() <= 1);
            }
        }
    }
}

fn howitzer() -> Weapon {
    let mut w = sparrow();
    w.source = "C_105.JT".into();
    w.flags = 0x300c0;
    w.damage.by_class = [300; 5];
    w.damage.fuze_radius = 0;
    w.damage.collateral_radius = 100;
    w.damage.collateral_percent = 35;
    w
}

#[test]
fn howitzer_fragmentation_by_class_and_distance() {
    let w = howitzer();
    let ground = burst(&w, Detonation::Ground);
    let air = burst(&w, Detonation::Air);
    for (distance, on_ground, in_air) in [
        (0., 51, 105),
        (25., 39, 81),
        (50., 27, 54),
        (75., 12, 27),
        (99., 0, 3),
        (100., 0, 0),
        (110., 0, 0),
    ] {
        for category in [0x8000, 0x2000, 0x100, 0x400, 0x40, 0x200, 0x800, 0x1000] {
            let armored = matches!(damage_class(category), 1 | 3);
            assert_eq!(
                ground.nominal(category, distance),
                if armored { 0 } else { on_ground }
            );
            assert_eq!(
                air.nominal(category, distance),
                if armored { 0 } else { in_air }
            );
        }
    }
    assert_eq!(w.damage.by_class, [300; 5], "direct-hit fields stay intact");
    for name in ["C_25.JT", "C_40.JT"] {
        let mut other = w.clone();
        other.source = name.into();
        other.damage.collateral_radius = 0;
        other.damage.collateral_percent = 0;
        let b = burst(&other, Detonation::Ground);
        assert!(!b.collateral());
        for category in [0x8000, 0x2000, 0x100, 0x400, 0x200] {
            assert_eq!(b.nominal(category, 0.), 0);
        }
    }
}

#[test]
fn howitzer_mixed_group_splash_applies_for_player_and_ai_shells() {
    for owner in [0, SHOOTER] {
        for enabled in [false, true] {
            let mut s = scene();
            let mut w = howitzer();
            if !enabled {
                w.damage.collateral_radius = 0;
                w.damage.collateral_percent = 0;
            }
            // Synthetic hit boxes, no world shapes or terrain placement.
            for (id, category, x) in [
                (1, 0x200, 40.),
                (2, 0x400, -40.),
                (3, 0x400, 40.),
                (4, 0x200, 125.),
            ] {
                ground_object(&mut s, id, [x, 8., 0.], 200, RED);
                s.targets.iter_mut().find(|t| t.id == id).unwrap().category = category;
            }
            s.projectiles
                .push(round(100, owner, &w, [0., 100., 0.], [0.; 3]));
            run_from(&mut s, 60, far());
            assert_eq!(hp(&s, 1), if enabled { 161 } else { 200 });
            for id in [2, 3, 4] {
                assert_eq!(hp(&s, id), 200, "armor and out-of-range truck");
            }
        }
    }
}

// ---- Who counts as the last attacker (nr-collkill, 2026-10-16) ---------

/// Runs `burst` over the scene's targets, as the step does, with no ownships.
fn burst_over(s: &mut State, burst: &Burst) {
    let (mut events, mut strikes, mut hits, mut impacts, mut scored) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    s.apply_burst(
        burst,
        &[],
        false,
        &|_, _| false,
        Outputs {
            events: &mut events,
            strikes: &mut strikes,
            ownship_hits: &mut hits,
            impacts: &mut impacts,
            scored: &mut scored,
        },
    );
}
/// Puts aircraft `a` and `b` nose to nose on the same spot and steps a tick.
fn collide(s: &mut State, a: u32, b: u32) -> Vec<Event> {
    for (id, z, v) in [(a, 1000., 600.), (b, 1035., -600.)] {
        let t = s.targets.iter_mut().find(|t| t.id == id).unwrap();
        t.position = [0., 8000., z];
        t.velocity = [0., 0., v];
    }
    run_from(s, 1, far())
}

#[test]
fn a_bystander_nicked_by_splash_is_no_ones_kill_when_it_later_collides() {
    // The Sparrow strikes aircraft 1 and its blast damages the wingman 2.
    let mut s = scene();
    sparrow_at_a_formation(&mut s, BLUE);
    assert!(hp(&s, 2) < 1000, "the wingman was damaged by the blast");
    let shooter = Kill {
        owner: SHOOTER,
        victim: 1,
        category: 0x80,
        aircraft: true,
    };
    assert_eq!(s.ledger.credit(1), Some(shooter), "the struck aircraft");
    assert_eq!(s.ledger.credit(2), None, "the bystander is no recorded hit");
    // The two then fly into each other: both are destroyed, only the one the
    // shot struck is credited, once.
    let events = collide(&mut s, 1, 2);
    assert!(events.contains(&Event::Destroyed(1)) && events.contains(&Event::Destroyed(2)));
    assert_eq!(hp(&s, 1), 0);
    assert_eq!(hp(&s, 2), 0);
    assert_eq!(s.ledger.credit(1), Some(shooter));
    assert_eq!(s.ledger.credit(2), None);
    assert!(
        s.ledger.kills().is_empty(),
        "a collision is no recorded kill"
    );
}

#[test]
fn a_collision_of_two_aircraft_nobody_hit_credits_nobody() {
    let mut s = scene();
    s.targets.push(aircraft(1, [0., 8000., 0.], BLUE));
    s.targets.push(aircraft(2, [0., 8000., 0.], BLUE));
    let events = collide(&mut s, 1, 2);
    assert!(events.contains(&Event::Destroyed(1)) && events.contains(&Event::Destroyed(2)));
    assert_eq!((hp(&s, 1), hp(&s, 2)), (0, 0));
    assert_eq!((s.ledger.credit(1), s.ledger.credit(2)), (None, None));
}

#[test]
fn a_burst_that_is_the_shots_own_outcome_makes_its_shooter_the_last_attacker() {
    // A flak shell that struck nothing resolves as a hit if its burst
    // damages anyone, so each aircraft it damaged has a hit behind it.
    let w = sparrow();
    let mut s = scene();
    s.targets.push(aircraft(SHOOTER, [0., 5000., -3000.], RED));
    s.targets.push(aircraft(1, [0., 5000., 100.], BLUE));
    let shell = round(70, SHOOTER, &w, [0., 5000., 0.], [0., 5000., 1.]);
    s.ledger.launch(
        70,
        SHOOTER,
        Some(1),
        crate::combat::ledger::ShotKind::AirToAir,
    );
    let burst = Burst::new(
        &shell,
        &w,
        Shooter {
            side: RED,
            surface: None,
            ownship: false,
        },
        [0., 5000., 0.],
        Detonation::Air,
        None,
        true,
    );
    burst_over(&mut s, &burst);
    assert!(hp(&s, 1) < 1000);
    assert_eq!(
        s.ledger.credit(1),
        Some(Kill {
            owner: SHOOTER,
            victim: 1,
            category: 0x80,
            aircraft: true,
        })
    );
    let tally = s
        .ledger
        .total(|k| k.owner == SHOOTER && k.kind == crate::combat::ledger::ShotKind::AirToAir);
    assert_eq!((tally.launched, tally.hit), (1, 1));
}

#[test]
fn missile_blast_spares_launcher_outside_launch_volume_and_on_ground() {
    for detonation in [Detonation::Air, Detonation::Ground] {
        let mut b = burst(&sparrow(), detonation);
        b.missile = true;
        assert!(b.graced(SHOOTER, [500., 0., 0.], 30.));
        assert!(!b.graced(SHOOTER + 1, [500., 0., 0.], 30.));
    }
}
