//! A helicopter's threat ring covers every launch the energy gate allows at
//! it (docs/spec/ai-strike.md, "Helicopter threat awareness"; John,
//! 2026-10-29). A synthetic shoulder-launched missile with the retail
//! record's layout; no retail data.
use super::*;
use tore_formats::weapons::{
    Burst, Damage, Effects, Guidance, Movement, Seeker as SeekerRecord, Weapon, Zone,
};

fn zone(min_range: i32, max_range: i32, min_altitude: i32, max_altitude: i32) -> Zone {
    Zone {
        heading: 5460,
        pitch: 5460,
        minimum_range: min_range,
        maximum_range: max_range,
        minimum_altitude: min_altitude,
        maximum_altitude: max_altitude,
    }
}

/// A surface infrared missile from rest, as the LIB's shoulder-launched
/// records: an 8,100 ft launch zone.
fn manpad() -> Weapon {
    Weapon {
        source: "SA7.JT".into(),
        name: "SA7".into(),
        hud_name: "SA7".into(),
        shape: None,
        fire_sound: None,
        native_callback: String::new(),
        flags: 0x12341,
        object_flags: 0,
        weight: 0,
        movement: Movement {
            minimum_speed: 0,
            corner_speed: 1500,
            maximum_speed: 2600,
            acceleration: 800,
            deceleration: 0,
            initial_speed: 0,
            final_speed: 1026,
            launch_retard: 0,
            ignite_t: 8,
            fuel_t: 92,
            remove_t: 80,
            powered_turn_rate: 2000,
            unpowered_turn_rate: 1000,
            performance_at_0: 100,
            performance_at_20: 100,
            cruise: [0; 4],
            jink: [0; 3],
        },
        burst: Burst {
            projectiles_in_pod: 1,
            actual_rounds_per_game: 1,
            game_rounds_in_burst: 1,
            game_rounds_in_carpet_burst: 1,
            game_burst_t: 0,
            reload_t: 40,
            startup_shots: 0,
            random_fire_percent: 0,
            offset_fire_percent: 0,
            offset_fire_heading: 0,
            offset_fire_pitch: 0,
            sine_pattern: [0; 4],
        },
        seeker: SeekerRecord {
            flags: [0; 2],
            signature: 2,
            look_down: 0,
            doppler_above: 0,
            doppler_below: 0,
            doppler_minimum_range: 0,
            all_aspect: 0,
            zones: [
                zone(0, 15_000, 0, i32::MAX),
                zone(500, 8_100, i32::MIN, 6_000),
            ],
            chaff_flare_chance: 100,
            deception_chance: 100,
        },
        guidance: Guidance {
            track_t: 20,
            track_max_g_raw: 0,
            target_sun_chance: 0,
            max_aon: 0,
            chances: [0; 4],
            hit_modifiers: [0; 9],
        },
        damage: Damage {
            by_class: [75, 7, 22, 15, 75],
            fuze_arm_t: 4,
            fuze_radius: 100,
            side_hit_fuze_failure: 0,
            collateral_radius: 0,
            collateral_percent: 0,
        },
        effects: Effects {
            object_explosion: 0,
            land_explosion: 0,
            water_explosion: 0,
            crater_size: 0,
            smoke: [0; 5],
            max_sound_distance: 0,
            frequency_adjustment: 0,
        },
    }
}

fn launcher(skill: i32) -> WeaponArms {
    let arc = MountArc {
        index: 0,
        rest: [0., 0.],
        limit: [0., 0.],
        offset: [0., 6., 0.],
        barrel: None,
    };
    WeaponArms::missile(manpad(), arc, skill, [40, 144, 60, 32_767], false).unwrap()
}

const KT: f64 = tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT;

#[test]
fn the_reach_covers_every_launch_the_energy_gate_allows_at_a_helicopter() {
    for skill in [0, 2] {
        let weapon = launcher(skill);
        let arc = weapon.mounts[0];
        let reach = RotorReach::default()
            .reach_ft(&weapon, 0., [0.; 3])
            .expect("a powered missile reaches");
        // The bug it fixes: a slow helicopter is launched at from beyond the
        // record's launch zone.
        assert!(
            reach > weapon.launch.max_range,
            "skill {skill}: {reach} within the zone's {}",
            weapon.launch.max_range
        );
        // The bound is real: closing head on a little inside it, from the
        // top of its altitude band, the gate passes at one of the heights.
        assert!(
            HEIGHTS_FT
                .iter()
                .any(|h| gate_hits(&weapon, &arc, 249., 0., reach - 500., *h)),
            "skill {skill}: nothing hits inside {reach}"
        );
        // Past it nothing passes: any height, any speed up to the closing
        // bound, any heading, a site anywhere in its altitude band.
        for ground in [0., 120., 249.] {
            for beyond in [1., 500., 2_000.] {
                for height in [0., 50., 300., 700., 1_500., 2_000., 2_500.] {
                    for (speed_kt, heading_deg) in [
                        (0., 0.),
                        (60., 180.),
                        (120., 180.),
                        (160., 180.),
                        (160., 90.),
                        (160., 135.),
                        (160., 0.),
                    ] {
                        let h: f64 = f64::to_radians(heading_deg);
                        let velocity = [h.sin() * speed_kt * KT, 0., h.cos() * speed_kt * KT];
                        let at = [0., height, reach + beyond];
                        assert!(
                            !gate_hits_moving(&weapon, &arc, ground, [0.; 3], at, velocity),
                            "skill {skill}: launched at {at:?} moving {velocity:?}, \
                             ground {ground}, reach {reach}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_wind_is_taken_either_way_along_the_line() {
    let weapon = launcher(2);
    let arc = weapon.mounts[0];
    let calm = RotorReach::default()
        .reach_ft(&weapon, 0., [0.; 3])
        .unwrap();
    // 30 kt from the north-east, whichever way the helicopter lies.
    let wind = [-21. * KT, 0., -21. * KT];
    let windy = RotorReach::default().reach_ft(&weapon, 0., wind).unwrap();
    assert!((windy - calm).abs() < 2_000., "{windy} against {calm}");
    for bearing in [0., 45., 90., 180., 225., 270.] {
        let b: f64 = f64::to_radians(bearing);
        let at = [b.sin() * (windy + 1.), 0., b.cos() * (windy + 1.)];
        let closing = [-b.sin() * 160. * KT, 0., -b.cos() * 160. * KT];
        assert!(!gate_hits_moving(&weapon, &arc, 0., wind, at, closing));
    }
}

#[test]
fn the_reach_is_cached_and_the_same_every_time() {
    let weapon = launcher(1);
    let mut cache = RotorReach::default();
    let first = cache.reach_ft(&weapon, 500., [0.; 3]);
    assert_eq!(cache.reach.len(), 1);
    // The same band: the cached answer.
    assert_eq!(cache.reach_ft(&weapon, 700., [0.; 3]), first);
    assert_eq!(cache.reach.len(), 1);
    // A fresh cache works it out to the same feet.
    assert_eq!(
        RotorReach::default().reach_ft(&weapon, 500., [0.; 3]),
        first
    );
    // A lower skill reaches no farther.
    let novice = cache.reach_ft(&launcher(0), 500., [0.; 3]).unwrap();
    assert!(novice <= first.unwrap());
}

#[test]
fn a_store_with_no_motor_has_no_energy_reach() {
    let mut weapon = launcher(2);
    weapon.record.movement.fuel_t = weapon.record.movement.ignite_t;
    assert_eq!(RotorReach::default().reach_ft(&weapon, 0., [0.; 3]), None);
}
