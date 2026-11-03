//! The player's air-to-ground missiles through combat (slice W2,
//! docs/spec/air-to-ground.md, "Missiles and the HARM boresight cue"): the
//! designated Maverick lock and its BORE pick, the HARM candidate and its
//! BORE launch, the CUED override, the HARM homing on after a shutdown, the
//! surface seekers seeing ground objects without the air notch, and the A-G
//! track that supports and retargets the AS-7 and AS-15.
use super::tests::fixture;
use super::*;
use crate::airport::OrientedBox;

const REDFOR: Side = Side(2);

fn launcher() -> Launcher {
    Launcher {
        position: [0., 3000., 0.],
        basis: Basis::new(0., 0., 0.),
        speed_fps: 400.,
        velocity: [0., 0., 400.],
        bay_ready: true,
        radar_power: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: sensors::Controls::default(),
    }
}

fn with_channel(channel: sensors::Channel) -> Launcher {
    Launcher {
        controls: sensors::Controls {
            channel,
            ..sensors::Controls::default()
        },
        ..launcher()
    }
}

/// A listed ground object `id` at `(x, z)`, class `class`.
fn ground(s: &mut State, id: u32, x: f64, z: f64, class: u16) {
    s.add_ground_target(
        id,
        OrientedBox {
            center: [x, 5., z],
            half: [8., 5., 15.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
        },
        100,
        class,
        REDFOR,
    )
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

/// The fixture without its range target, carrying `count` of record `source`
/// (seeker `signature`) with wide synthetic zones: zone 0 and zone 1 45
/// degrees, out to 60,000 ft.
fn armed(source: &str, signature: u8) -> State {
    let mut s = fixture(true);
    s.targets.clear();
    let own = s.own_mut();
    let w = &mut own.config.stations[0].weapon;
    w.source = source.into();
    w.flags = if matches!(source, "AS7.JT" | "AS15.JT") {
        0x241
    } else {
        0x41
    };
    w.seeker.signature = signature;
    w.movement.remove_t = 400;
    w.movement.fuel_t = 200;
    w.movement.maximum_speed = 3000;
    w.movement.acceleration = 800;
    for zone in &mut w.seeker.zones {
        zone.minimum_range = 0;
        zone.maximum_range = 60_000;
        zone.heading = 8190;
        zone.pitch = 8190;
    }
    own.config.stations[0].internal = true;
    own.ammo = vec![4];
    own.armed = true;
    s
}

fn step(s: &mut State, l: Launcher, held: bool) -> Vec<Event> {
    s.step(
        &[OwnshipInput {
            aircraft: 0,
            held,
            launcher: l,
        }],
        |_, _| 0.,
    )
}

/// Steps until the selected weapon reads READY, at most `ticks`.
fn until_ready(s: &mut State, l: Launcher, ticks: usize) -> Readiness {
    for _ in 0..ticks {
        step(s, l, false);
        if s.own_view().readiness(l) == Readiness::Ready {
            return Readiness::Ready;
        }
    }
    s.own_view().readiness(l)
}

/// Fires the selected weapon and returns the new projectile.
fn fire(s: &mut State, l: Launcher) -> Projectile {
    let before = s.projectiles.len();
    step(s, l, true);
    assert_eq!(s.projectiles.len(), before + 1, "one round left");
    s.projectiles.last().unwrap().clone()
}

#[test]
fn the_reviewed_air_to_ground_records_have_surface_profiles() {
    let profile = |source: &str| {
        let signature = match source {
            "AGM65A.JT" | "AGM65G.JT" | "AGM84E.JT" | "AIM9M.JT" => 2,
            "AGM45.JT" | "AGM88.JT" => 4,
            _ => 3,
        };
        let mut s = armed(source, signature);
        if source == "AIM7.JT" {
            s.own_mut().config.stations[0].weapon.flags |= 0x200;
        }
        missiles::Profile::for_weapon(&s.own().config.stations[0].weapon).unwrap()
    };
    for (source, guidance) in [
        ("AGM65G.JT", Guidance::Infrared),
        ("AGM65A.JT", Guidance::Infrared),
        ("AGM84E.JT", Guidance::Infrared),
        ("AGM45.JT", Guidance::Emitter),
        ("AGM88.JT", Guidance::Emitter),
        ("AGM84A.JT", Guidance::Active),
        ("AM39.JT", Guidance::Active),
        ("AS16.JT", Guidance::Active),
        ("AS7.JT", Guidance::Supported),
        ("AS15.JT", Guidance::Supported),
    ] {
        let p = profile(source);
        assert_eq!(
            (p.role, p.guidance),
            (TargetRole::Surface, guidance),
            "{source}"
        );
        assert_eq!(
            p.supports_boresight(),
            matches!(guidance, Guidance::Infrared | Guidance::Emitter),
            "{source}"
        );
        assert_eq!(
            p.needs_air_ground(),
            matches!(guidance, Guidance::Active | Guidance::Supported),
            "{source}"
        );
    }
    // The air-to-air profiles are unchanged.
    for source in ["AIM9M.JT", "AIM120.JT", "AIM7.JT"] {
        let p = profile(source);
        assert_eq!(p.role, TargetRole::Aircraft);
        assert!(!p.needs_air_ground() && !p.holds_on_angles());
        assert_eq!(p.search_cap(), 5f64.to_radians());
    }
    assert_eq!(profile("AGM88.JT").search_cap(), std::f64::consts::PI);
}

#[test]
fn every_sagging_missile_and_surface_guidance_holds_its_path() {
    let s = fixture(true);
    let mut w = s.own().config.stations[0].weapon.clone();
    for (source, holds_path) in [
        ("AGM65A.JT", true),
        ("AGM65G.JT", true),
        ("AGM88.JT", true),
        ("AGM84A.JT", true),
        ("AIM7.JT", false),
        ("AIM120.JT", false),
        ("AIM9M.JT", false),
        ("SA6.JT", false),
        ("SEA_SPAR.JT", false),
        ("SYNTHETIC.JT", false),
    ] {
        w.source = source.into();
        w.flags = if matches!(source, "AIM7.JT" | "SA6.JT" | "SEA_SPAR.JT") {
            0x241
        } else {
            0x41
        };
        w.seeker.signature = match source {
            "AGM65A.JT" | "AGM65G.JT" | "AIM9M.JT" => 2,
            "AGM88.JT" => 4,
            _ => 3,
        };
        for sags in [false, true] {
            w.flags = (w.flags & !missiles::SAG_FLAG) | if sags { missiles::SAG_FLAG } else { 0 };
            assert_eq!(
                missiles::Motion::launch(&w, launcher().velocity, launcher().position[1])
                    .holds_path,
                holds_path || sags,
                "{source}, sags={sags}"
            );
        }
    }

    // Record-based guidance has no named surface profile but holds its path too.
    w.source = "SYNTHETIC.JT".into();
    w.seeker.signature = 1;
    w.flags = 0x401;
    assert!(missiles::Motion::launch(&w, launcher().velocity, 3000.).holds_path);
}

#[test]
fn a_maverick_locks_a_designated_ground_target_and_in_bore_the_strongest_return() {
    // CUED: the A-G page and T give the target; the seeker locks it.
    let mut s = armed("AGM65G.JT", 2);
    ground(&mut s, 0x5000_0001, 300., 20_000., 0x400);
    ground(&mut s, 0x5000_0002, -300., 22_000., 0x400);
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    s.command(0, Command::Designate, launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    assert_eq!(s.own().launch_mode, LaunchMode::Cued);
    assert_eq!(s.own().mounted.target, Some(0x5000_0001));
    let p = fire(&mut s, launcher());
    assert_eq!(p.target, Some(0x5000_0001));
    assert!(p.guidance.as_ref().unwrap().seeker.acquired);

    // BORE: nothing designated; the hotter of two returns inside the bore
    // circle wins, and the trigger waits for the seeker to hold it. The nose
    // points down at them.
    let dive = Launcher {
        basis: Basis::new(0., -(3000f64 / 20_000.).atan(), 0.),
        ..launcher()
    };
    let mut s = armed("AGM65G.JT", 2);
    ground(&mut s, 0x5000_0001, 100., 20_000., 0x400);
    ground(&mut s, 0x5000_0002, -150., 20_000., 0x400);
    ground(&mut s, 0x5000_0003, 6_000., 20_000., 0x400);
    for t in &mut s.targets {
        t.signature.infrared = match t.id {
            0x5000_0002 => 90.,
            0x5000_0003 => 100.,
            _ => 40.,
        };
    }
    step(&mut s, dive, false);
    assert_eq!(s.own().launch_mode, LaunchMode::Boresight);
    assert_eq!(
        s.own_view().readiness(dive),
        Readiness::RadarAcquiring,
        "the seeker has not held the candidate yet"
    );
    assert_eq!(until_ready(&mut s, dive, 120), Readiness::Ready);
    // The hottest object outside the 5 degree circle is not the pick.
    assert_eq!(s.own().bore_observation.map(|o| o.id), Some(0x5000_0002));
    let p = fire(&mut s, dive);
    assert_eq!(p.target, Some(0x5000_0002));
    assert_eq!(p.guidance.as_ref().unwrap().mode, LaunchMode::Boresight);
    // An empty bore circle has nothing to fire at.
    let mut s = armed("AGM65G.JT", 2);
    step(&mut s, launcher(), false);
    assert_eq!(s.own_view().readiness(launcher()), Readiness::NoTarget);
}

/// A lock does not let a missile pass through another solid ground object.
/// The old Maverick battery approached three bridge sections end-on; extending
/// their contact bounds to the drawn lines put a nearer section in the path.
#[test]
fn locked_surface_missiles_hit_the_first_solid_on_their_path() {
    for source in ["AGM65A.JT", "AGM65G.JT", "AGM84E.JT"] {
        for obstructed in [false, true] {
            let mut s = armed(source, 2);
            let target = 0x5000_0001;
            let obstacle = 0x5000_0002;
            ground(&mut s, target, 0., 20_000., 0x400);
            if obstructed {
                s.add_ground_target(
                    obstacle,
                    OrientedBox {
                        center: [0., 1500., 15_000.],
                        half: [500., 1500., 500.],
                        heading: 0.,
                        pitch: 0.,
                        bank: 0.,
                    },
                    100,
                    0x100,
                    REDFOR,
                )
                .unwrap();
            }
            s.command(0, Command::ToggleAirGround, launcher());
            step(&mut s, launcher(), false);
            s.command(0, Command::DesignateTarget(target), launcher());
            assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
            let shot = fire(&mut s, launcher());
            assert_eq!(shot.target, Some(target), "{source}");
            for _ in 0..40 * 120 {
                if s.projectiles.is_empty() {
                    break;
                }
                let flight = s.projectiles[0].guidance.as_ref().unwrap();
                assert_eq!(flight.seeker.target, Some(target), "{source}");
                assert_eq!(flight.seeker.status, Status::Locked, "{source}");
                assert!(!flight.unguided, "{source}");
                step(&mut s, launcher(), false);
            }
            assert!(s.projectiles.is_empty(), "{source}");
            let hp = |id| s.targets.iter().find(|t| t.id == id).unwrap().hp;
            if obstructed {
                assert_eq!(hp(target), 100, "{source}: the farther target is protected");
                assert!(hp(obstacle) < 100, "{source}: the nearer solid was hit");
            } else {
                assert!(hp(target) < 100, "{source}: clear path reaches the lock");
            }
        }
    }
}

/// The HARM fixture: three ground radars ahead, emitting.
fn harm_scene() -> State {
    let mut s = armed("AGM88.JT", 4);
    ground(&mut s, 0x5000_0001, 500., 30_000., 0x1000);
    ground(&mut s, 0x5000_0002, 4_000., 30_000., 0x1000);
    ground(&mut s, 0x5000_0003, 40_000., 30_000., 0x1000);
    for t in &mut s.targets {
        t.radar_emitting = true;
    }
    s
}

#[test]
fn the_harm_cue_lists_emitters_in_the_cone_and_bore_fires_at_the_candidate() {
    let mut s = harm_scene();
    step(&mut s, launcher(), false);
    assert_eq!(s.own().launch_mode, LaunchMode::Boresight);
    // The cue: emitters inside the 45 degree cone, nearest the bore first;
    // the radar 53 degrees off the nose is not one.
    let cue: Vec<u32> = s
        .own()
        .harm_cue()
        .iter()
        .map(|e| e.observation.id)
        .collect();
    assert_eq!(cue, [0x5000_0001, 0x5000_0002]);
    assert!(s.own().harm_cue().iter().all(|e| e.emitting && !e.painting));
    // Nearest the bore is the candidate; the trigger fires at it once the
    // seeker holds it, with nothing designated.
    assert_eq!(s.own().bore_observation.map(|o| o.id), Some(0x5000_0001));
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    assert_eq!(s.own().designated(), None);
    let p = fire(&mut s, launcher());
    assert_eq!(p.target, Some(0x5000_0001));
    let flight = p.guidance.as_ref().unwrap();
    assert!(flight.seeker.acquired && flight.mode == LaunchMode::Boresight);

    // A radar painting the aircraft comes first, wherever it is in the cone.
    let mut s = harm_scene();
    s.set_painted_by(0, [0x5000_0002]);
    step(&mut s, launcher(), false);
    assert_eq!(s.own().bore_observation.map(|o| o.id), Some(0x5000_0002));
    assert!(
        s.own()
            .harm_cue()
            .iter()
            .any(|e| e.observation.id == 0x5000_0002 && e.painting)
    );
    // Nothing emitting: NO EMITTER, and nothing leaves.
    let mut s = harm_scene();
    for t in &mut s.targets {
        t.radar_emitting = false;
    }
    step(&mut s, launcher(), false);
    assert_eq!(s.own_view().readiness(launcher()), Readiness::NoEmitter);
    assert_eq!(Readiness::NoEmitter.label(), "NO EMITTER");
    step(&mut s, launcher(), true);
    assert!(s.projectiles.is_empty());
}

#[test]
fn a_harm_page_designation_turns_the_cue_cued_and_holds_it_there() {
    let mut s = harm_scene();
    let page = with_channel(sensors::Channel::Harm);
    step(&mut s, page, false);
    assert_eq!(s.own().sensors.ground_page(), Some(sensors::Mode::Harm));
    s.command(0, Command::Designate, page);
    s.command(0, Command::Designate, page);
    assert_eq!(s.own().designated(), Some(0x5000_0002));
    step(&mut s, page, false);
    assert_eq!(s.own().launch_mode, LaunchMode::Cued);
    // The seeker mode key cannot take a designated HARM back to BORE.
    s.command(0, Command::ToggleSeekerMode, page);
    assert_eq!(s.own().launch_mode, LaunchMode::Cued);
    assert_eq!(until_ready(&mut s, page, 120), Readiness::Ready);
    let p = fire(&mut s, page);
    assert_eq!(p.target, Some(0x5000_0002));
    assert_eq!(p.guidance.as_ref().unwrap().mode, LaunchMode::Cued);
}

#[test]
fn a_launched_harm_keeps_homing_on_its_radar_after_it_stops_emitting() {
    let mut s = harm_scene();
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    let id = fire(&mut s, launcher()).id;
    for _ in 0..60 {
        step(&mut s, launcher(), false);
    }
    // The radar shuts down: the HARM still sees it on angles and homes on.
    for t in &mut s.targets {
        t.radar_emitting = false;
    }
    for _ in 0..120 {
        step(&mut s, launcher(), false);
    }
    let p = s.projectiles.iter().find(|p| p.id == id).unwrap();
    let flight = p.guidance.as_ref().unwrap();
    assert_eq!(flight.seeker.status, Status::Locked);
    assert_eq!(flight.seeker.observation.map(|o| o.id), Some(0x5000_0001));
    // The cue keeps the held radar, dim.
    assert!(
        s.own()
            .harm_cue()
            .iter()
            .all(|e| !e.emitting && e.observation.id == 0x5000_0001)
    );
    // Flown on to the end, it hits.
    for _ in 0..3000 {
        step(&mut s, launcher(), false);
        if s.projectiles.is_empty() {
            break;
        }
    }
    let radar = s.targets.iter().find(|t| t.id == 0x5000_0001).unwrap();
    assert!(radar.hp < radar.initial_hp, "the shut-down radar was hit");
}

#[test]
fn a_surface_seeker_sees_a_stationary_ground_object_without_the_air_notch() {
    let mut s = armed("AGM84A.JT", 3);
    ground(&mut s, 0x5000_0001, 0., 30_000., 0x2000);
    let target = s.targets[0].clone();
    let w = s.own().config.stations[0].weapon.clone();
    let basis = Basis::new(0., -0.1, 0.);
    let at = [0., 3000., 0.];
    assert!(missiles::active_radar_visible(&w, at, basis, &target, 0.));
    // The same seeker under an air profile loses the stationary object in
    // the clutter notch.
    let mut air = w.clone();
    air.source = "AIM120.JT".into();
    assert!(!missiles::active_radar_visible(
        &air, at, basis, &target, 0.
    ));
}

#[test]
fn a_surface_radar_cued_solution_uses_the_ag_track() {
    let mut s = armed("AGM84A.JT", 3);
    ground(&mut s, 0x5000_0001, 0., 30_000., 0x2000);
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    s.command(0, Command::Designate, launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    assert_eq!(until_ready(&mut s, launcher(), 10), Readiness::Ready);
    // The A-G page off: the designation stays, the track is gone.
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    assert_eq!(s.own_view().readiness(launcher()), Readiness::TrackLost);
    assert_eq!(Readiness::TrackLost.label(), "NO TRACK");
    // Back on, the launch leaves for the point the track gives.
    s.command(0, Command::ToggleAirGround, launcher());
    assert_eq!(until_ready(&mut s, launcher(), 10), Readiness::Ready);
    let p = fire(&mut s, launcher());
    let flight = p.guidance.as_ref().unwrap();
    assert_eq!(p.target, Some(0x5000_0001));
    assert!(flight.last_intercept.is_some());
}

/// An AS-7 in flight on target 1 of two tanks, on the A-G track.
fn sarh_in_flight() -> (State, u32) {
    let mut s = armed("AS7.JT", 3);
    ground(&mut s, 0x5000_0001, 0., 20_000., 0x400);
    ground(&mut s, 0x5000_0002, 1_500., 21_000., 0x400);
    ground(&mut s, 0x5000_0003, -12_000., 9_000., 0x400);
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    // A scope click on the first tank.
    s.command(0, Command::DesignateTarget(0x5000_0001), launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0001));
    assert_eq!(until_ready(&mut s, launcher(), 10), Readiness::Ready);
    let id = fire(&mut s, launcher()).id;
    for _ in 0..60 {
        step(&mut s, launcher(), false);
    }
    (s, id)
}

fn flight(s: &State, id: u32) -> (Option<u32>, bool) {
    let p = s.projectiles.iter().find(|p| p.id == id).unwrap();
    (p.target, p.guidance.as_ref().unwrap().unguided)
}

#[test]
fn an_as7_rides_the_ag_track_and_loses_its_target_for_good_without_it() {
    let (mut s, id) = sarh_in_flight();
    assert_eq!(flight(&s, id), (Some(0x5000_0001), false));
    // The page off: the missile flies on unguided, and stays so.
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    assert_eq!(flight(&s, id), (None, true));
    s.command(0, Command::ToggleAirGround, launcher());
    for _ in 0..30 {
        step(&mut s, launcher(), false);
    }
    assert_eq!(flight(&s, id), (None, true));
}

#[test]
fn a_new_designation_retargets_an_as7_inside_its_cone_and_blinds_it_outside() {
    let (mut s, id) = sarh_in_flight();
    // T to the next tank, a few degrees off the missile's nose.
    s.command(0, Command::Designate, launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0002));
    assert_eq!(flight(&s, id), (Some(0x5000_0002), false));
    for _ in 0..30 {
        step(&mut s, launcher(), false);
    }
    assert_eq!(flight(&s, id), (Some(0x5000_0002), false));
    // Then to one far outside its 45 degree cone: blinded.
    s.command(0, Command::DesignateTarget(0x5000_0003), launcher());
    assert_eq!(s.own().designated(), Some(0x5000_0003));
    assert_eq!(flight(&s, id), (None, true));
}

#[test]
fn the_painting_list_and_the_harm_cue_survive_a_checkpoint() {
    let mut s = harm_scene();
    s.set_painted_by(0, [0x5000_0002]);
    step(&mut s, launcher(), false);
    let models = crate::checkpoint::Models::default();
    let own: Ownship = crate::checkpoint::round_trip(s.own(), &models).unwrap();
    assert_eq!(own.painted_by(), s.own().painted_by());
    assert_eq!(own.harm_cue(), s.own().harm_cue());
    assert!(!own.harm_cue().is_empty());
}

/// An AGM-65A-like record (synthetic numbers of the same shape): a 9 second
/// burn to about 1,900 ft/s, then a coast toward 953 ft/s with the record's
/// sag (flag 0x4) and the 1,024 ft cruise height until 7,424 ft out, 40 s of
/// life and 50,000 ft of seeker range.
fn slow_maverick() -> State {
    let mut s = armed("AGM65A.JT", 2);
    let w = &mut s.own_mut().config.stations[0].weapon;
    w.flags = 0x2a06f;
    let m = &mut w.movement;
    (m.maximum_speed, m.corner_speed, m.initial_speed) = (1906, 1906, 0);
    (m.acceleration, m.deceleration, m.final_speed) = (733, 146, 953);
    (m.ignite_t, m.fuel_t, m.remove_t) = (4, 40, 160);
    (m.powered_turn_rate, m.unpowered_turn_rate) = (10920, 8190);
    (m.performance_at_0, m.performance_at_20) = (70, 100);
    m.cruise = [29, 4, 29, 4];
    for zone in &mut w.seeker.zones {
        zone.maximum_range = 50_000;
    }
    s
}

/// Fires the slow Maverick CUED from 3,000 ft at a tank 6 nmi ahead, with
/// path holding as `holds_path` says, and flies it to its end. Returns the
/// tank's hit points left and where the missile last was.
fn slow_maverick_at_six_miles(holds_path: bool) -> (i32, Vector) {
    let mut s = slow_maverick();
    ground(&mut s, 0x5000_0001, 0., 6. * 6_076., 0x400);
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    s.command(0, Command::Designate, launcher());
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    let id = fire(&mut s, launcher()).id;
    let p = s.projectiles.iter_mut().find(|p| p.id == id).unwrap();
    let motion = p.motion.as_mut().unwrap();
    assert!(
        motion.holds_path && motion.sags,
        "a surface missile holds its path"
    );
    motion.holds_path = holds_path;
    let mut last = p.position;
    for _ in 0..40 * 120 {
        step(&mut s, launcher(), false);
        match s.projectiles.iter().find(|p| p.id == id) {
            Some(p) => last = p.position,
            None => break,
        }
    }
    let tank = s.targets.iter().find(|t| t.id == 0x5000_0001).unwrap();
    (tank.hp, last)
}

#[test]
fn a_coasting_maverick_holds_its_path_against_the_sag_and_reaches_a_tank_at_six_miles() {
    // The fault John saw (2026-10-11): steering pointed only the velocity at
    // the tank, the 80 ft/s sag carried the coasting missile under its line
    // of sight, and it met the ground about 1,000 ft short.
    let (hp, last) = slow_maverick_at_six_miles(false);
    assert_eq!(hp, 100, "without path holding the tank is untouched");
    assert!(
        6. * 6_076. - last[2] > 500.,
        "it ends short of the tank: {last:?}"
    );
    // Holding the path, the same shot hits.
    let (hp, last) = slow_maverick_at_six_miles(true);
    assert!(hp < 100, "the tank is hit, ended at {last:?}");
}

#[test]
fn surface_lock_and_launch_require_a_clear_terrain_line_for_every_profile() {
    let ridge = |_: f64, z: f64| {
        if (8000. ..=12000.).contains(&z) {
            4000.
        } else {
            0.
        }
    };
    for (name, signature) in [
        ("AGM65A.JT", 2),
        ("AGM65G.JT", 2),
        ("AGM84E.JT", 2),
        ("AGM88.JT", 4),
        ("AGM45.JT", 4),
        ("AGM84A.JT", 3),
        ("AM39.JT", 3),
        ("AS16.JT", 3),
        ("AS7.JT", 3),
        ("AS15.JT", 3),
    ] {
        let mut s = armed(name, signature);
        ground(&mut s, 0x5000_0001, 0., 20000., 0x400);
        s.targets[0].radar_emitting = true;
        s.command(0, Command::ToggleAirGround, launcher());
        step(&mut s, launcher(), false);
        s.command(0, Command::Designate, launcher());
        assert_eq!(
            until_ready(&mut s, launcher(), 120),
            Readiness::Ready,
            "{name}"
        );
        let at = s.own_view().at(launcher());
        let range = at.estimated_max_range();
        let in_range = at.in_estimated_range();
        let masked = at.with_terrain(&ridge);
        assert_eq!(masked.readiness(), Readiness::TerrainMask, "{name}");
        assert!(!masked.can_lock(), "{name}");
        assert_eq!(
            masked.estimated_max_range(),
            range,
            "terrain is not range: {name}"
        );
        assert_eq!(
            masked.in_estimated_range(),
            in_range,
            "terrain is not IN RNG: {name}"
        );
        for _ in 0..60 {
            s.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: launcher(),
                }],
                ridge,
            );
            assert!(s.projectiles.is_empty(), "masked launch: {name}");
            assert!(
                !matches!(s.own().mounted.status, Status::Locked | Status::Pitbull),
                "masked lock: {name}"
            );
        }
    }
}

#[test]
fn a_terrain_masked_harm_is_not_drawn_as_a_shutdown_emitter() {
    let mut s = harm_scene();
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    let ridge = |_: Vector, _: Vector| true;
    assert!(agm::harm_emitters(s.own(), &launcher(), s.targets.iter(), &ridge).is_empty());
}

/// Review L2: IN RNG ignores a terrain mask only. Master arm safe or an empty
/// station turns it off for a surface weapon as for any other.
#[test]
fn in_range_is_off_with_master_arm_safe_or_an_empty_station() {
    let ridge = |_: f64, z: f64| {
        if (8000. ..=12000.).contains(&z) {
            4000.
        } else {
            0.
        }
    };
    let mut s = armed("AGM65A.JT", 2);
    ground(&mut s, 0x5000_0001, 0., 20000., 0x400);
    s.command(0, Command::ToggleAirGround, launcher());
    step(&mut s, launcher(), false);
    s.command(0, Command::Designate, launcher());
    assert_eq!(until_ready(&mut s, launcher(), 120), Readiness::Ready);
    assert!(s.own_view().at(launcher()).in_estimated_range());
    assert!(
        s.own_view()
            .at(launcher())
            .with_terrain(&ridge)
            .in_estimated_range(),
        "a terrain mask keeps IN RNG"
    );
    s.own_mut().armed = false;
    let at = s.own_view().at(launcher());
    assert_eq!(at.readiness(), Readiness::Safe);
    assert!(!at.in_estimated_range(), "master arm safe");
    s.own_mut().armed = true;
    s.own_mut().ammo = vec![0];
    let at = s.own_view().at(launcher());
    assert_eq!(at.readiness(), Readiness::Empty);
    assert!(!at.in_estimated_range(), "empty station");
}

#[test]
fn unsupported_player_surface_sarh_launches_and_recovers_later_illumination() {
    for source in ["AS7.JT", "AS15.JT"] {
        for radar_off in [false, true] {
            let mut s = armed(source, 3);
            ground(&mut s, 0x5000_0001, 500., 20_000., 0x400);
            s.command(0, Command::ToggleAirGround, launcher());
            step(&mut s, launcher(), false);
            s.command(0, Command::DesignateTarget(0x5000_0001), launcher());
            s.command(0, Command::ToggleAirGround, launcher());
            let off = Launcher {
                radar: !radar_off,
                radar_power: !radar_off,
                ..launcher()
            };
            let p = fire(&mut s, off);
            assert_eq!(p.guidance.as_ref().unwrap().mode, LaunchMode::Boresight);
            assert!(!p.guidance.as_ref().unwrap().unguided);
            assert!(p.guidance.as_ref().unwrap().last_intercept.is_none());
            for _ in 0..30 {
                step(&mut s, off, false);
            }
            let shot = s.projectiles.iter().find(|x| x.id == p.id).unwrap();
            assert_eq!(shot.direction, p.direction);
            assert!(shot.guidance.as_ref().unwrap().last_intercept.is_none());
            s.command(0, Command::ToggleAirGround, launcher());
            for _ in 0..60 {
                step(&mut s, launcher(), false);
            }
            let shot = s.projectiles.iter().find(|x| x.id == p.id).unwrap();
            assert_eq!(shot.target, Some(0x5000_0001));
            assert_eq!(
                shot.guidance.as_ref().unwrap().seeker.status,
                Status::Locked
            );
            assert_ne!(shot.direction, p.direction);
            // Unlike a supported-at-launch surface shot, this uncued release
            // remains eligible for another support recovery.
            s.command(0, Command::ToggleAirGround, launcher());
            step(&mut s, launcher(), false);
            assert!(
                !s.projectiles
                    .iter()
                    .find(|x| x.id == p.id)
                    .unwrap()
                    .guidance
                    .as_ref()
                    .unwrap()
                    .unguided
            );
        }
    }
}
