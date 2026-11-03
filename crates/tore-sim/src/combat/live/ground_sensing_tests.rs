//! The ground pages through combat (slice G1): the A-G command, silence to
//! anti-radiation seekers, registered ground objects on the page, the HARM
//! page's seeker choice and the Easy targeting cheat's nearest ground target.
use super::tests::fixture;
use super::*;
use crate::airport::OrientedBox;

const REDFOR: Side = Side(2);

fn launcher() -> Launcher {
    Launcher {
        position: [0., 3000., 0.],
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

fn tank(ahead_ft: f64) -> OrientedBox {
    OrientedBox {
        center: [0., 5., ahead_ft],
        half: [8., 5., 15.],
        heading: 0.,
        pitch: 0.,
        bank: 0.,
    }
}

/// The fixture with its range target gone and `ground` tanks registered as a
/// template's units would be: class 0x400, listed, `maxVisDist` 78.
fn scene(ground: &[(u32, f64, Side)]) -> State {
    let mut s = fixture(true);
    s.targets.clear();
    for &(id, ahead, side) in ground {
        s.add_ground_target(id, tank(ahead), 100, 0x400, side)
            .unwrap();
        assert!(s.set_ground_listing(
            id,
            GroundListing {
                flags: 0x821,
                max_visible: 78,
                team: false,
            }
        ));
    }
    s
}

fn tick(s: &mut State) {
    s.step(
        &[OwnshipInput {
            aircraft: 0,
            held: false,
            launcher: launcher(),
        }],
        |_, _| 0.,
    );
}

#[test]
fn the_ag_command_lists_registered_ground_objects_and_the_radar_falls_silent() {
    let mut s = scene(&[
        (0x5000_0001, 12_000., REDFOR),
        (0x5000_0002, 30_000., REDFOR),
    ]);
    assert!(ownship_target(s.own(), launcher()).radar_emitting);
    s.command(0, Command::ToggleAirGround, launcher());
    assert!(s.own().sensors.air_ground());
    tick(&mut s);
    let ids: Vec<u32> = s.own().sensors.ground().iter().map(|g| g.id()).collect();
    assert_eq!(ids, [0x5000_0001, 0x5000_0002]);
    assert_eq!(s.own().sensors.mode(), Some(sensors::Mode::AirGround));
    // The A-G page does not emit for anti-radiation seekers.
    assert!(!ownship_target(s.own(), launcher()).radar_emitting);
    // T designates the nearest; the HUD and the views follow it.
    s.command(0, Command::Designate, launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    for _ in 0..10 {
        tick(&mut s);
    }
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    assert_eq!(
        s.own_view().display_target().map(|t| t.id),
        Some(0x5000_0001)
    );
    // An object without the listing flag never shows.
    let mut s = scene(&[(0x5000_0003, 12_000., REDFOR)]);
    s.set_ground_listing(0x5000_0003, GroundListing::default());
    s.command(0, Command::ToggleAirGround, launcher());
    tick(&mut s);
    assert!(s.own().sensors.ground().is_empty());
}

#[test]
fn an_aircraft_shows_on_ag_only_while_the_host_says_it_is_in_an_airfield_sequence() {
    let mut s = scene(&[]);
    let mut landing = super::tests::target(0x0100_0001, [0., 300., 15_000.], 50, 0x8000);
    landing.velocity = [0., 0., -100.];
    s.targets.push(landing);
    s.command(0, Command::ToggleAirGround, launcher());
    tick(&mut s);
    assert!(s.own().sensors.ground().is_empty());
    s.set_airfield_attached(0x0100_0001, true);
    for _ in 0..sensors::track::GROUND_REBUILD_STEPS {
        tick(&mut s);
    }
    let listed: Vec<_> = s
        .own()
        .sensors
        .ground()
        .iter()
        .map(|g| g.symbol())
        .collect();
    assert_eq!(listed, [sensors::GroundSymbol::Square]);
}

#[test]
fn ctrl_x_designates_the_nearest_hostile_ground_object_only_with_easy_targeting() {
    let mut s = scene(&[
        (0x5000_0001, 40_000., REDFOR),
        (0x5000_0002, 20_000., REDFOR),
        (0x5000_0003, 5_000., NO_SIDE),
        (0x5000_0004, 8_000., DEFAULT_OWNSHIP_SIDE),
    ]);
    s.command(0, Command::NearestGround, launcher());
    assert_eq!(s.own().designated(), None);
    s.cheats.easy_targeting = true;
    s.command(0, Command::NearestGround, launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0002));
    // Easy targeting keeps it although no page lists it.
    for _ in 0..60 {
        tick(&mut s);
    }
    assert_eq!(s.own().designated(), Some(0x5000_0002));
}

#[test]
fn the_harm_page_uses_the_shortest_range_anti_radiation_seeker_aboard() {
    let mut s = fixture(true);
    assert_eq!(ground_sensing::harm_seeker(s.own()), None);
    let own = s.own_mut();
    let mut agm88 = own.config.stations[0].clone();
    agm88.weapon.seeker.signature = 4;
    agm88.weapon.seeker.zones[0].maximum_range = 200_000;
    let mut agm45 = agm88.clone();
    agm45.weapon.seeker.zones[0].maximum_range = 60_000;
    own.config.stations = vec![agm88.clone(), agm45];
    own.ammo = vec![1, 1];
    assert_eq!(
        ground_sensing::harm_seeker(s.own()).map(|v| v.maximum_ft),
        Some(60_000.)
    );
    // Fired off, the AGM-45 no longer counts.
    s.own_mut().ammo[1] = 0;
    assert_eq!(
        ground_sensing::harm_seeker(s.own()).map(|v| v.maximum_ft),
        Some(200_000.)
    );
}

#[test]
fn the_ground_facts_survive_a_checkpoint() {
    let mut s = scene(&[(0x5000_0001, 12_000., REDFOR)]);
    s.set_airfield_attached(7, true);
    s.set_sensor_weather(sensors::Weather {
        bands: vec![sensors::WeatherBand {
            low_ft: 0.,
            high_ft: 9_000.,
            effects: [10, 50, 10, 75, 75],
        }],
    });
    let facts = |s: &State| {
        (
            s.ground_listing(0x5000_0001),
            s.airfield_attached(7),
            s.sensor_weather().clone(),
        )
    };
    let models = crate::checkpoint::Models::default();
    let mut restored = scene(&[]);
    restored.ground_listing = crate::checkpoint::round_trip(&s.ground_listing, &models).unwrap();
    restored.airfield = crate::checkpoint::round_trip(&s.airfield, &models).unwrap();
    restored.sensor_weather = crate::checkpoint::round_trip(&s.sensor_weather, &models).unwrap();
    assert_eq!(facts(&restored), facts(&s));
}

// -- Hidden shoulder-fired teams (T.O.R.E opinionated, John 2026-10-29) ------

const TEAM: u32 = 0x5000_0009;

/// A launcher `alt_ft` over the ground, `back_ft` south of the origin,
/// flying north.
fn launcher_at(alt_ft: f64, back_ft: f64) -> Launcher {
    Launcher {
        position: [0., alt_ft, -back_ft],
        ..launcher()
    }
}

/// A tank and, beside it, a team the host marked hidden, both `ahead_ft`
/// north of the origin.
fn team_scene(ahead_ft: f64) -> State {
    let mut s = scene(&[(0x5000_0001, ahead_ft, REDFOR), (TEAM, ahead_ft, REDFOR)]);
    if let Some(row) = s.targets.iter_mut().find(|t| t.id == TEAM) {
        row.position[0] = 400.;
        row.category = 0x1000;
    }
    let listing = s.ground_listing(TEAM).unwrap();
    s.set_ground_listing(
        TEAM,
        GroundListing {
            team: true,
            ..listing
        },
    );
    s
}

fn tick_at(s: &mut State, l: Launcher) {
    s.step(
        &[OwnshipInput {
            aircraft: 0,
            held: false,
            launcher: l,
        }],
        |_, _| 0.,
    );
}

fn page_ids(s: &State) -> Vec<u32> {
    s.own().sensors.ground().iter().map(|g| g.id()).collect()
}

fn on_map(s: &State, id: u32) -> bool {
    s.own()
        .sensors
        .map_contacts()
        .iter()
        .any(|m| m.contact.id == id)
        || s.own().sensors.visual().iter().any(|c| c.id == id)
}

#[test]
fn the_player_s_spotting_distance_is_the_ace_s() {
    assert_eq!(super::ground_sensing::player_team_spot_ft(100.), 4_000.);
    assert_eq!(super::ground_sensing::player_team_spot_ft(50.), 2_000.);
}

#[test]
fn a_hidden_team_is_off_the_ag_page_and_the_map_until_the_pilot_spots_it() {
    let far = launcher_at(3_000., 0.);
    let mut s = team_scene(12_000.);
    s.command(0, Command::ToggleAirGround, far);
    tick_at(&mut s, far);
    assert_eq!(page_ids(&s), [0x5000_0001], "the tank, not the team");
    assert!(on_map(&s, 0x5000_0001) && !on_map(&s, TEAM));
    assert!(s.spotted_teams(0).is_empty());
    // Easy targeting's Ctrl+X passes the unseen team by.
    s.cheats.easy_targeting = true;
    if let Some(row) = s.targets.iter_mut().find(|t| t.id == 0x5000_0001) {
        row.position[2] = 30_000.;
    }
    s.command(0, Command::NearestGround, far);
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    s.cheats.easy_targeting = false;

    // Within 4,000 ft, ahead and in clear sight: spotted, and on the page at
    // its next rebuild.
    let near = launcher_at(800., -9_000.);
    tick_at(&mut s, near);
    assert_eq!(s.spotted_teams(0), [TEAM]);
    for _ in 0..sensors::track::GROUND_REBUILD_STEPS {
        tick_at(&mut s, near);
    }
    assert!(page_ids(&s).contains(&TEAM));
    // Once spotted it stays known, even far away.
    for _ in 0..sensors::track::GROUND_REBUILD_STEPS {
        tick_at(&mut s, far);
    }
    assert!(page_ids(&s).contains(&TEAM));
}

#[test]
fn a_team_is_not_spotted_from_just_beyond_the_distance_or_behind_a_ridge() {
    // 4,600 ft out at 300 ft: not spotted.
    let mut s = team_scene(12_000.);
    let l = launcher_at(300., -7_400.);
    for _ in 0..30 {
        tick_at(&mut s, l);
    }
    assert!(s.spotted_teams(0).is_empty());
    // 3,000 ft out but a ridge between: not spotted.
    let mut s = team_scene(12_000.);
    let l = launcher_at(300., -9_000.);
    let ridge = |_x: f64, z: f64| {
        if (10_000.0..10_500.).contains(&z) {
            2_000.
        } else {
            0.
        }
    };
    s.step(
        &[OwnshipInput {
            aircraft: 0,
            held: false,
            launcher: l,
        }],
        ridge,
    );
    assert!(s.spotted_teams(0).is_empty());
}

fn team_round(target: u32) -> Projectile {
    Projectile {
        id: 900,
        owner: TEAM,
        weapon: None,
        guidance: None,
        motion: None,
        guidance_ticks: None,
        age: 0,
        incoming: None,
        station: 0,
        position: [400., 10., 12_000.],
        previous: [400., 10., 12_000.],
        direction: [0., 0., -1.],
        speed_f8: 0,
        launched_t: 0,
        target: Some(target),
        fall: FallState::default(),
        gun_round: None,
        tracer: false,
    }
}

fn spot_launches(s: &mut State, at: Vector) {
    let mut ships = std::mem::take(&mut s.ownships);
    s.spot_team_launches(&mut ships[0], at);
    s.ownships = ships;
}

#[test]
fn a_team_that_launches_at_the_player_or_a_friendly_it_sees_is_spotted() {
    let at = [0., 3_000., 0.];
    // At the player: spotted at once.
    let mut s = team_scene(12_000.);
    s.projectiles.push(team_round(0));
    spot_launches(&mut s, at);
    assert_eq!(s.spotted_teams(0), [TEAM]);
    // At an aircraft the pilot neither flies with nor sees: nothing.
    let mut s = team_scene(12_000.);
    s.projectiles.push(team_round(77));
    spot_launches(&mut s, at);
    assert!(s.spotted_teams(0).is_empty());
    // At a friendly the pilot's sensors observe now, the team within its
    // 19,968 ft listing distance: spotted. Beyond it: nothing.
    let mut s = team_scene(12_000.);
    let mut friend = super::tests::target(0x0100_0005, [500., 3_000., 6_000.], 50, 0x8000);
    friend.side = DEFAULT_OWNSHIP_SIDE;
    s.targets.push(friend);
    tick_at(&mut s, launcher_at(3_000., 0.));
    assert!(
        s.own().sensors.observation(0x0100_0005).is_some(),
        "the fixture sees its friend"
    );
    s.projectiles.push(team_round(0x0100_0005));
    spot_launches(&mut s, [0., 3_000., -30_000.]);
    assert!(s.spotted_teams(0).is_empty(), "too far from the team");
    spot_launches(&mut s, at);
    assert_eq!(s.spotted_teams(0), [TEAM]);
}

#[test]
fn spotted_teams_and_the_team_flag_survive_a_checkpoint() {
    let mut s = team_scene(12_000.);
    s.projectiles.push(team_round(0));
    spot_launches(&mut s, [0., 3_000., 0.]);
    s.projectiles.clear();
    let models = crate::checkpoint::Models::default();
    let bytes = crate::checkpoint::to_bytes(&s, &models).unwrap();
    let back: State = crate::checkpoint::from_bytes(&bytes, &models).unwrap();
    assert_eq!(back.spotted_teams(0), [TEAM]);
    assert!(back.ground_listing(TEAM).unwrap().team);
}

#[test]
fn a_team_the_targeting_sight_holds_is_spotted() {
    let far = launcher_at(3_000., 0.);
    let mut s = team_scene(12_000.);
    tick_at(&mut s, far);
    assert!(s.spotted_teams(0).is_empty());
    s.ownships[0].sensors.hold(TEAM);
    tick_at(&mut s, far);
    assert_eq!(s.spotted_teams(0), [TEAM]);
}
