//! The strike tactics (docs/spec/ai-strike.md): the pass choice table, the
//! seeded skill draw, the release gates, the heavy floor, the SEAD order,
//! the store choice, the pass, exit and re-attack flow, and the checkpoint.
//! Synthetic pictures and solutions only.
use super::*;
use crate::ai::ScalarSpeed;
use crate::ai::SpeedLimits;
use crate::ai::awareness::ground::{
    DefenseFacts, GroundPicture, GroundSubject, RecentWarnings, Subjects, ThreatLevel,
};
use crate::ai::engagement::{StrikeObjective, StrikeRole};
use crate::checkpoint::{Models, from_bytes, to_bytes};
use crate::sensors::GroundObject;

pub(super) const TARGET: u32 = 0x5000_0001;
pub(super) const OWN: u32 = 1;

pub(super) fn own(position: Vector, speed: f64) -> OwnState {
    OwnState {
        position,
        heading_deg: 0.,
        flight_path_pitch_deg: 0.,
        body_pitch_offset_deg: 2.,
        bank_deg: 0.,
        speed: ScalarSpeed(speed),
        limits: SpeedLimits {
            minimum: ScalarSpeed(220.),
            maximum: ScalarSpeed(1_000.),
            corner: ScalarSpeed(600.),
        },
        altitude_msl_ft: position[1],
        agl_ft: position[1],
        terrain_ahead_ft: 0.,
        terrain_climb_deg: -90.,
        minimum_altitude_ft: 300.,
        at_ceiling: false,
        on_ground: false,
        g_limit: 7.,
        roll_limit_deg_per_s: 180.,
        maximum_bank_deg: 80.,
        alive: true,
        fuel_endurance_s: 3_600.,
        time_home_s: Some(600.),
        rotorcraft: false,
        internal_fuel_lbs: 8_000.,
        radar_emitting: true,
    }
}

pub(super) fn ground(
    id: u32,
    position: Vector,
    class: u16,
    defense: Option<DefenseFacts>,
) -> GroundSubject {
    GroundSubject {
        object: GroundObject {
            id,
            position,
            velocity: [0.; 3],
            class,
            listed: true,
            max_visible: 78,
            radar: 100.,
            infrared: 100.,
            emitting: defense.is_some_and(|d| d.radar),
            aircraft: false,
            airborne: false,
            airfield: false,
            destroyed: false,
        },
        side: Side(2),
        defense,
        hidden: false,
    }
}

pub(super) fn site(missile_ft: f64, gun_ft: f64, radar: bool) -> Option<DefenseFacts> {
    Some(DefenseFacts {
        missile_reach_ft: missile_ft,
        rotor_reach_ft: missile_ft,
        gun_reach_ft: gun_ft,
        radar,
        working: true,
        ship: false,
        heading_deg: 0.,
    })
}

/// A picture ordered onto `TARGET`, built from `subjects` (which hold it),
/// briefed on `defenses` too.
pub(super) fn picture(subjects: &[GroundSubject], defenses: Vec<u32>) -> GroundPicture {
    let mut picture = GroundPicture::new(StrikeRole::Strike);
    picture.set_ordered(Some(TARGET));
    picture.set_objective(StrikeObjective {
        targets: Vec::new(),
        defenses,
        point: None,
    });
    picture.rebuild(0, Vec::new(), &Subjects::new(subjects), Vec::new());
    picture
}

pub(super) fn bomb(rounds: u32) -> Store {
    Store {
        station: StationId(1),
        kind: StoreKind::Bomb,
        rounds,
        cadence_ticks: 30,
        fire_range_ft: 10_000.,
        minimum_range_ft: 0.,
        maximum_range_ft: 20_000.,
        round_speed_fps: None,
        release_speed_share: 1.,
        bomb_body: Default::default(),
        fuze_arm_s: 1.,
        frag_radius_ft: 300.,
        falls_free: false,
        flight: None,
    }
}

pub(super) fn store(station: u8, kind: StoreKind) -> Store {
    Store {
        station: StationId(station),
        kind,
        ..bomb(4)
    }
}

pub(super) fn inputs(stores: Vec<Store>, solution: Option<Solution>) -> Inputs {
    Inputs {
        class: StrikeClass::Attack,
        stores,
        designator: false,
        mission_height_ft: None,
        solution,
        lase: LaseInputs::default(),
        shots: Shots::default(),
    }
}

pub(super) fn solution(target: Vector, impact: Vector, due: bool) -> Solution {
    Solution {
        station: StationId(1),
        target: TARGET,
        target_position: target,
        target_velocity: [0.; 3],
        target_alive: true,
        impact: Some(impact),
        impact_seconds: Some(10.),
        due,
        warning: false,
        guided_in_flight: false,
        reach_ok: true,
        release_seconds: None,
        reaches: false,
        bar_ft: None,
    }
}

pub(super) struct World {
    pub(super) own: OwnState,
    pub(super) picture: GroundPicture,
    pub(super) inputs: Inputs,
}

impl World {
    pub(super) fn context(&self, tick: u64) -> Context<'_> {
        Context {
            tick,
            aircraft: OWN,
            side: Side(1),
            own: &self.own,
            velocity: [0., 0., self.own.speed.0],
            picture: &self.picture,
            inputs: &self.inputs,
            experience: Experience::Average,
            warnings: RecentWarnings::default(),
            defending: false,
            recovering: false,
            mates: &[],
            wing: WingFacts::default(),
            ceiling_ft: 50_000.,
            terrain: None,
            threat_bearing_deg: None,
        }
    }
}

pub(super) const TARGET_AT: Vector = [0., 0., 24_000.];

/// An A-4E-like striker at 5,000 ft, 4 nm south of a truck, nothing around.
pub(super) fn quiet_world() -> World {
    World {
        own: own([0., 5_000., 0.], 700.),
        picture: picture(&[ground(TARGET, TARGET_AT, 0x200, None)], Vec::new()),
        inputs: inputs(vec![bomb(4)], None),
    }
}

#[test]
fn the_pass_is_a_fixed_function_of_class_store_threat_and_speed() {
    use PassChoice::{Pass as P, Regain};
    let choose = |class, kind, threat, energy_good, dive_gate| {
        choose_pass(PassInputs {
            class,
            kind,
            retarded: false,
            threat,
            energy_good,
            dive_gate,
        })
    };
    let fighter = StrikeClass::Fighter;
    let bomb = StoreKind::Bomb;
    // The spec's table for bombs, first row that applies.
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Guns, false, true),
        Regain
    );
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Missiles, false, true),
        Regain
    );
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Missiles, true, true),
        P(Pass::PopUp)
    );
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Guns, true, true),
        P(Pass::Dive)
    );
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Clear, false, true),
        P(Pass::Dive)
    );
    assert_eq!(
        choose(fighter, bomb, ThreatLevel::Air, true, false),
        P(Pass::Straight)
    );
    assert_eq!(
        choose(StrikeClass::Attack, bomb, ThreatLevel::Clear, true, false),
        P(Pass::Straight)
    );
    // Other stores.
    for kind in [
        StoreKind::Seeker,
        StoreKind::RadarAgm,
        StoreKind::AntiRadiation,
    ] {
        assert_eq!(
            choose(fighter, kind, ThreatLevel::Missiles, true, false),
            P(Pass::FastHigh)
        );
        assert_eq!(
            choose(fighter, kind, ThreatLevel::Clear, false, false),
            P(Pass::Straight)
        );
    }
    for kind in [StoreKind::Laser, StoreKind::Coordinate] {
        assert_eq!(
            choose(fighter, kind, ThreatLevel::Missiles, false, true),
            P(Pass::Level)
        );
    }
    // Rockets fly the bombs' table (slice ST-T2: the roll and pull points
    // the nose at the target after a pop-up or into a dive).
    let rocket = StoreKind::Rocket;
    assert_eq!(
        choose(fighter, rocket, ThreatLevel::Missiles, true, true),
        P(Pass::PopUp)
    );
    assert_eq!(
        choose(fighter, rocket, ThreatLevel::Clear, true, true),
        P(Pass::Dive)
    );
    assert_eq!(
        choose(fighter, rocket, ThreatLevel::Clear, true, false),
        P(Pass::Straight)
    );
    assert_eq!(
        choose(fighter, rocket, ThreatLevel::Guns, false, true),
        Regain
    );
    // A gun strafes, a drag bomb lays down; both regain speed first under
    // a defended target.
    assert_eq!(
        choose(
            StrikeClass::Attack,
            StoreKind::Gun,
            ThreatLevel::Missiles,
            true,
            true
        ),
        P(Pass::Strafe)
    );
    assert_eq!(
        choose(
            StrikeClass::Attack,
            StoreKind::Gun,
            ThreatLevel::Guns,
            false,
            false
        ),
        Regain
    );
    let drag = |threat, energy_good| {
        choose_pass(PassInputs {
            class: StrikeClass::Attack,
            kind: StoreKind::Bomb,
            retarded: true,
            threat,
            energy_good,
            dive_gate: true,
        })
    };
    assert_eq!(drag(ThreatLevel::Missiles, true), P(Pass::Laydown));
    assert_eq!(drag(ThreatLevel::Clear, true), P(Pass::Laydown));
    assert_eq!(drag(ThreatLevel::Guns, false), Regain);
    // A heavy flies level whatever it carries or faces.
    for kind in [bomb, StoreKind::Seeker, StoreKind::Coordinate] {
        assert_eq!(
            choose(StrikeClass::Heavy, kind, ThreatLevel::Missiles, false, true),
            P(Pass::Level)
        );
    }
    // The dive gate: max(10,000 ft, 2R) of height and of distance.
    assert!(dive_gate(10_000., 10_000., 1_000.));
    assert!(!dive_gate(9_999., 20_000., 1_000.));
    assert!(!dive_gate(12_000., 11_000., 6_000.));
    assert!((turn_radius_ft(600., 7.) - 1_179.6).abs() < 1.);
}

#[test]
fn the_release_gate_is_a_tenth_of_the_circle_but_never_under_ten_feet() {
    let gate = |l| (GATE_SHARE * release_circle_ft(l)).max(GATE_FLOOR_FT);
    // Ace 33 ft: the floor. Experienced 115 ft: the floor's 11.5 ft is above it.
    assert_eq!(gate(Experience::Ace), GATE_FLOOR_FT);
    assert!((gate(Experience::Experienced) - 11.48).abs() < 0.01);
    assert!((gate(Experience::Novice) - 27.89).abs() < 0.01);
}

#[test]
fn the_release_circle_is_john_s_and_the_draw_is_deterministic() {
    let metres: Vec<f64> = Experience::ALL
        .iter()
        .map(|l| (release_circle_ft(*l) * 0.3048).round())
        .collect();
    assert_eq!(metres, vec![85., 60., 35., 10.]);
    let a = aim_draw(77, 3, 1, 1_000.);
    assert_eq!(
        a,
        aim_draw(77, 3, 1, 1_000.),
        "same seed, aircraft and pass"
    );
    assert_ne!(
        a.offset_ft,
        aim_draw(77, 3, 2, 1_000.).offset_ft,
        "the pass moves it"
    );
    assert_ne!(
        a.offset_ft,
        aim_draw(77, 4, 1, 1_000.).offset_ft,
        "the aircraft moves it"
    );
    assert_ne!(
        a.offset_ft,
        aim_draw(78, 3, 1, 1_000.).offset_ft,
        "the mission moves it"
    );
    // The radius scales the same draw.
    let half = aim_draw(77, 3, 1, 500.);
    assert!((half.offset_ft[0] * 2. - a.offset_ft[0]).abs() < 1e-9);
}

#[test]
fn ninety_percent_of_draws_are_inside_the_circle_uniform_by_area_and_the_rest_in_the_ring() {
    let r = 1_000.;
    let n = 20_000;
    let mut inside = 0;
    let mut inside_mean = 0.;
    let mut half = 0;
    let mut right = 0;
    for pass in 1..=n {
        let d = aim_draw(0x5eed, 7, pass, r);
        let dist = d.offset_ft[0].hypot(d.offset_ft[1]);
        if d.inside {
            inside += 1;
            assert!(dist <= r + 1e-9);
            inside_mean += dist;
            if dist <= r / 2f64.sqrt() {
                half += 1;
            }
        } else {
            assert!((r - 1e-9..=2. * r + 1e-9).contains(&dist), "{dist}");
        }
        if d.offset_ft[0] > 0. {
            right += 1;
        }
    }
    let share = f64::from(inside) / f64::from(n);
    assert!((share - 0.9).abs() < 0.01, "{share}");
    // Uniform by area: mean two thirds of R, half the draws inside R/√2.
    let mean = inside_mean / f64::from(inside);
    assert!((mean / r - 2. / 3.).abs() < 0.01, "{mean}");
    let half_share = f64::from(half) / f64::from(inside);
    assert!((half_share - 0.5).abs() < 0.015, "{half_share}");
    // A uniform bearing.
    assert!((f64::from(right) / f64::from(n) - 0.5).abs() < 0.015);
}

#[test]
fn the_release_floors_and_the_heavy_rule() {
    assert_eq!(release_floor_ft(ThreatLevel::Clear), 3_000.);
    assert_eq!(release_floor_ft(ThreatLevel::Air), 3_000.);
    assert_eq!(release_floor_ft(ThreatLevel::Guns), 5_000.);
    assert_eq!(release_floor_ft(ThreatLevel::Missiles), 5_000.);
    // max(5,000 ft, the mission's airborne height), +0/+5,000/+10,000 by
    // threat, at most 90% of the ceiling.
    assert_eq!(heavy_floor_ft(None, ThreatLevel::Clear, 50_000.), 5_000.);
    assert_eq!(
        heavy_floor_ft(Some(3_000.), ThreatLevel::Air, 50_000.),
        5_000.
    );
    assert_eq!(
        heavy_floor_ft(Some(12_000.), ThreatLevel::Clear, 50_000.),
        12_000.
    );
    assert_eq!(heavy_floor_ft(None, ThreatLevel::Guns, 50_000.), 10_000.);
    assert_eq!(
        heavy_floor_ft(Some(8_000.), ThreatLevel::Missiles, 50_000.),
        18_000.
    );
    assert_eq!(
        heavy_floor_ft(Some(30_000.), ThreatLevel::Missiles, 35_000.),
        31_500.
    );
}

#[test]
fn a_heavy_flies_its_level_run_at_its_floor() {
    let mut world = quiet_world();
    world.inputs.class = StrikeClass::Heavy;
    world.inputs.mission_height_ft = Some(9_000.);
    let mut strike = Strike::new(3);
    let order = strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Level));
    // Climbing from 5,000 ft toward 9,000 ft over the target.
    assert!(order.steer.unwrap().pitch_deg > 0.);
}

#[test]
fn stores_are_chosen_by_class_target_and_threat() {
    let none = TargetFacts::default();
    let radar = TargetFacts {
        radar: true,
        emitting: false,
    };
    let all = vec![
        store(0, StoreKind::Rocket),
        store(1, StoreKind::Bomb),
        store(2, StoreKind::Seeker),
        store(3, StoreKind::Laser),
        store(4, StoreKind::AntiRadiation),
        store(5, StoreKind::Command),
    ];
    let kind = |stores: &[Store], class, threat, target, designator| {
        choose_store(stores, class, threat, target, designator).map(|s| s.kind)
    };
    let fighter = StrikeClass::Fighter;
    let clear = ThreatLevel::Clear;
    // A HARM only at a radar, first.
    assert_eq!(
        kind(&all, fighter, clear, radar, false),
        Some(StoreKind::AntiRadiation)
    );
    assert_eq!(
        kind(&all, fighter, clear, none, false),
        Some(StoreKind::Bomb)
    );
    // Stand-off first under threat Missiles.
    assert_eq!(
        kind(&all, fighter, ThreatLevel::Missiles, none, false),
        Some(StoreKind::Seeker)
    );
    // A laser store only with its own designator.
    let laser_or_rocket = vec![store(0, StoreKind::Rocket), store(3, StoreKind::Laser)];
    assert_eq!(
        kind(&laser_or_rocket, fighter, clear, none, false),
        Some(StoreKind::Rocket)
    );
    assert_eq!(
        kind(&laser_or_rocket, fighter, clear, none, true),
        Some(StoreKind::Laser)
    );
    // A heavy fires no rockets; the AT-2 is never chosen here; empty stores are not.
    let rockets = [store(0, StoreKind::Rocket)];
    assert_eq!(kind(&rockets, StrikeClass::Heavy, clear, none, false), None);
    assert_eq!(
        kind(&[store(5, StoreKind::Command)], fighter, clear, none, false),
        None
    );
    assert_eq!(kind(&[bomb(0)], fighter, clear, none, false), None);
}

#[test]
fn an_anti_radiation_carrier_takes_sam_radars_then_aaa_radars_then_other_radars() {
    let subjects = [
        ground(TARGET, [0., 0., 30_000.], 0x200, None),
        ground(0x10, [0., 0., 10_000.], 0x800, site(0., 7_500., true)),
        ground(0x11, [0., 0., 20_000.], 0x1000, site(40_000., 0., true)),
        ground(0x12, [0., 0., 25_000.], 0x100, site(0., 0., true)),
        ground(0x13, [0., 0., 5_000.], 0x1000, site(40_000., 0., false)),
        ground(0x14, [0., 0., 28_000.], 0x1000, site(40_000., 0., true)),
    ];
    let pic = picture(&subjects, vec![0x10, 0x11, 0x12, 0x13, 0x14]);
    let none = |_: u32| 0;
    assert_eq!(
        sead_order(&pic, Side(1), [0.; 3], &none),
        vec![0x11, 0x14, 0x10, 0x12],
        "SAM radars nearest first, then AAA radar, then any radar; no radar, no place"
    );
    let full = |id: u32| if id == 0x11 { 2 } else { 0 };
    assert_eq!(
        sead_order(&pic, Side(1), [0.; 3], &full)[0],
        0x14,
        "two attackers fill it"
    );
    // A striker with a HARM and no order takes the SAM radar first.
    let mut briefed = GroundPicture::new(StrikeRole::Strike);
    briefed.set_objective(StrikeObjective {
        targets: vec![TARGET],
        defenses: vec![0x10, 0x11],
        point: Some([0., 0., 30_000.]),
    });
    briefed.rebuild(0, Vec::new(), &Subjects::new(&subjects), Vec::new());
    let world = World {
        own: own([0., 15_000., 0.], 700.),
        picture: briefed,
        inputs: inputs(vec![store(2, StoreKind::AntiRadiation), bomb(4)], None),
    };
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.target(), Some(0x11));
    assert_eq!(
        strike.selection().map(|s| s.kind),
        Some(StoreKind::AntiRadiation)
    );
}

/// The strike's aim is the draw's, to a billionth of a foot. Not bit for bit:
/// this crate's tests build optimised, so a draw with constant arguments can
/// be folded at compile time with the compiler's own sine while the strike's
/// call uses the target's maths library, and on i686 Windows the two differ
/// in the last bit (CI, 2026-10-12). The gunship's burst test uses the same
/// tolerance.
fn assert_same_aim(aim: [f64; 2], drawn: [f64; 2]) {
    assert!(
        aim.iter().zip(drawn).all(|(a, d)| (a - d).abs() < 1e-9),
        "aim {aim:?}, drawn {drawn:?}"
    );
}

#[test]
fn a_pass_releases_on_the_gate_exits_turns_back_and_attacks_again() {
    let mut world = quiet_world();
    let mut strike = Strike::new(9);
    let order = strike.step(&world.context(0));
    // Clear, 5,000 ft: no dive gate, so a straight run, already in its final.
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::Straight));
    assert_eq!(strike.pass_number(), 1);
    assert!(order.steer.is_some() && order.release.is_none());
    let selection = strike.selection().expect("a selection for the cockpit");
    assert_eq!(
        (selection.target, selection.station),
        (TARGET, StationId(1))
    );
    let (aim, inside) = strike.aim();
    let circle = release_circle_ft(Experience::Average);
    let draw = aim_draw(9, OWN, 1, circle);
    assert_eq!(inside, draw.inside);
    assert_same_aim(aim, draw.offset_ft);
    assert_eq!(selection.aim_ft, aim, "a still target: no lead");
    assert!((selection.gate_ft - (GATE_SHARE * circle).max(GATE_FLOOR_FT)).abs() < 1e-9);

    // The cue far short: no release while the host says not due.
    world.own.position = [0., 3_000., 18_000.];
    world.inputs.solution = Some(solution(TARGET_AT, [0., 0., 21_000.], false));
    assert_eq!(strike.step(&world.context(30)).release, None);
    // Due, inside the zone: the bomb goes, and the run exits.
    let aim_point = [aim[0], 0., 24_000. + aim[1]];
    world.inputs.solution = Some(solution(TARGET_AT, aim_point, true));
    let order = strike.step(&world.context(60));
    assert_eq!(
        order.release,
        Some(Release {
            station: StationId(1),
            target: TARGET
        })
    );
    assert_eq!(strike.phase(), Phase::Exit);
    // The jink inside 10,000 ft: climbing at maximum speed.
    let exit = strike.step(&world.context(61)).steer.unwrap();
    assert_eq!(
        (exit.pitch_deg, exit.speed),
        (JINK_CLIMB_DEG, SpeedRequest::Maximum)
    );
    // Out past 30,000 ft it turns back...
    world.own.position = [0., 6_000., 24_000. + 31_000.];
    world.inputs.solution = Some(solution(TARGET_AT, aim_point, false));
    strike.step(&world.context(4_000));
    assert_eq!(strike.phase(), Phase::TurnBack);
    // ...and once pointing at the target attacks again, a new pass with a
    // new draw.
    world.own.heading_deg = 180.;
    strike.step(&world.context(4_100));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass_number(), 2);
    assert_same_aim(strike.aim().0, aim_draw(9, OWN, 2, circle).offset_ft);
}

#[test]
fn a_dead_target_or_no_store_sends_it_home() {
    let mut world = quiet_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    // The target falls: the pass ends; at the turn back nothing is left.
    let mut dead = solution(TARGET_AT, [0., 0., 0.], false);
    dead.target_alive = false;
    world.inputs.solution = Some(dead);
    strike.step(&world.context(10));
    assert_eq!(strike.phase(), Phase::Exit);
    let mut fallen = ground(TARGET, TARGET_AT, 0x200, None);
    fallen.object.destroyed = true;
    world.picture = picture(&[fallen], Vec::new());
    world.own.position = [0., 6_000., 60_000.];
    strike.step(&world.context(20));
    world.own.heading_deg = 180.;
    strike.step(&world.context(30));
    assert_eq!(strike.phase(), Phase::Home);
    assert!(strike.home() && strike.selection().is_none());
    // No usable store: home at the first choice.
    let mut world = quiet_world();
    world.inputs.stores = vec![bomb(0)];
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    assert!(strike.home());
}

#[test]
fn the_floor_the_zone_and_a_warning_hold_the_release_back() {
    let mut world = quiet_world();
    let mut strike = Strike::new(9);
    strike.force(Some(Pass::Level), None);
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Level));
    let (aim, _) = strike.aim();
    let aim_point = [aim[0], 0., 24_000. + aim[1]];
    // Below the 3,000 ft floor a level release waits.
    world.own.position = [0., 2_500., 15_000.];
    world.own.agl_ft = 2_500.;
    world.inputs.solution = Some(solution(TARGET_AT, aim_point, true));
    assert_eq!(strike.step(&world.context(10)).release, None);
    // Above it, but the cue past the record's zone 1 maximum.
    world.own.position = [0., 4_000., 15_000.];
    world.own.agl_ft = 4_000.;
    world.inputs.stores[0].maximum_range_ft = 5_000.;
    assert_eq!(strike.step(&world.context(20)).release, None);
    // In the zone with the sight warning LOW or FRAG.
    world.inputs.stores[0].maximum_range_ft = 20_000.;
    let mut warned = solution(TARGET_AT, aim_point, true);
    warned.warning = true;
    world.inputs.solution = Some(warned);
    assert_eq!(strike.step(&world.context(30)).release, None);
    // Clear of all three it goes.
    world.inputs.solution = Some(solution(TARGET_AT, aim_point, true));
    assert!(strike.step(&world.context(40)).release.is_some());
}

#[test]
fn a_defence_hands_over_and_the_striker_resumes_only_when_safe() {
    let mut world = quiet_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    let mut cx = world.context(10);
    cx.defending = true;
    let order = strike.step(&cx);
    assert_eq!((strike.phase(), order.steer), (Phase::Defend, None));
    // A warning keeps it there for 10 s.
    let mut cx = world.context(20);
    cx.warnings.surface_to_air = true;
    strike.step(&cx);
    assert_eq!(
        strike.step(&world.context(20 + WARNING_TICKS - 1)).steer,
        None
    );
    // Quiet and safe: back to the attack.
    strike.step(&world.context(20 + WARNING_TICKS));
    assert_eq!(strike.phase(), Phase::Attack);
    // Quiet but a SAM covers the target: it egresses, and with the SAM
    // still there at the exit distance it goes home.
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(0x20, [0., 0., 26_000.], 0x1000, site(30_000., 0., true)),
        ],
        vec![0x20],
    );
    let mut cx = world.context(5_000);
    cx.defending = true;
    strike.step(&cx);
    strike.step(&world.context(5_001));
    assert_eq!(strike.phase(), Phase::Exit);
    world.own.position = [0., 6_000., -10_000.];
    strike.step(&world.context(6_000));
    assert_eq!(strike.phase(), Phase::Home);
}

#[test]
fn a_strike_round_trips_through_a_checkpoint_and_steps_on_identically() {
    let mut world = quiet_world();
    let mut strike = Strike::new(42);
    strike.force(Some(Pass::Dive), Some(StationId(1)));
    for tick in 0..200 {
        world.own.position[2] += 5.;
        strike.step(&world.context(tick));
    }
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let mut twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, strike);
    for tick in 200..400 {
        world.own.position[2] += 5.;
        let a = strike.step(&world.context(tick));
        let b = twin.step(&world.context(tick));
        assert_eq!(a, b, "tick {tick}");
    }
    assert_eq!(twin, strike);
    // The host's inputs never outlive a step.
    strike.set_inputs(inputs(vec![bomb(1)], None));
    let coded = to_bytes(&strike, &models).unwrap();
    let restored: Strike = from_bytes(&coded, &models).unwrap();
    assert!(restored.inputs().is_none());
}

// AI lasing (slice ST-L): the hold, the duty, the wing's lase, the spot
// assurance.

use crate::combat::laser::Mask;
use crate::combat::sight::Gimbal;

const FEET_PER_NM: f64 = FEET_PER_NAUTICAL_MILE;

fn laser_store() -> Store {
    store(2, StoreKind::Laser)
}

/// A hemisphere pod 10 nm of reach, as the F-16C's Pave Spike.
fn pod() -> LaseSensor {
    LaseSensor {
        gimbal: Gimbal::Hemisphere,
        reach_ft: 60_760.,
    }
}

/// A built-in sensor: the cone about the nose, 60 degrees to search and 45 to
/// track.
fn cone() -> LaseSensor {
    LaseSensor {
        gimbal: Gimbal::Cone {
            search: 60f64.to_radians(),
            track: 45f64.to_radians(),
        },
        reach_ft: 60_760.,
    }
}

/// A designator striker over a still target: laser store, no solution.
fn lasing_world(sensor: Option<LaseSensor>) -> World {
    let mut world = quiet_world();
    world.inputs = inputs(vec![laser_store()], None);
    world.inputs.designator = sensor.is_some();
    world.inputs.lase = LaseInputs {
        sensor,
        target: Some((TARGET_AT, true)),
        ..LaseInputs::default()
    };
    world
}

fn context_at<'a>(world: &'a World, tick: u64, velocity: Vector) -> Context<'a> {
    Context {
        velocity,
        ..world.context(tick)
    }
}

#[test]
fn the_orbit_is_three_nautical_miles_or_one_point_two_turn_radii() {
    let three = 3. * FEET_PER_NM;
    assert!((LASE_RADIUS_MIN_FT - three).abs() < 1e-9);
    // A fighter's turn radius is far inside three miles.
    assert!((lase_radius_ft(600., 7., 60_760.) - three).abs() < 1e-9);
    // A heavy turning at 2 G at 1,200 ft/s needs more: 1.2 turn radii.
    let heavy = lase_radius_ft(1_200., 2., 1.0e6);
    assert!((heavy - 1.2 * turn_radius_ft(1_200., 2.)).abs() < 1e-9);
    assert!(heavy > three);
    // Never beyond 80 percent of the laser's reach.
    assert!((lase_radius_ft(1_200., 2., 20_000.) - 16_000.).abs() < 1e-9);
}

#[test]
fn a_pods_gimbal_limits_the_bank_by_the_targets_depression() {
    // A target 45 degrees down abeam: tan(bank) <= tan(45) / sin(90), less
    // the 5 degree margin.
    assert!((bank_cap_deg(10_000., 10_000., 90.) - 40.).abs() < 1e-6);
    // Nearer the nose the bank may be steeper.
    assert!((bank_cap_deg(10_000., 10_000., 30.) - (2f64.atan().to_degrees() - 5.)).abs() < 1e-6);
    // Dead ahead: only the 80 degree stop.
    assert!((bank_cap_deg(10_000., 10_000., 0.) - 80.).abs() < 1e-6);
    // A target on the other side of the turn does not bind.
    assert!((bank_cap_deg(10_000., 10_000., -90.) - 80.).abs() < 1e-6);
    // Not below the aircraft: no bank toward it.
    assert_eq!(bank_cap_deg(-100., 10_000., 90.), 0.);
    // The speed a level orbit needs for that bank: v^2 = g r tan(depression).
    let v = orbit_speed_fps(10_000., 10_000., 1_000.);
    let bank = (v * v / (32.174 * 10_000.)).atan().to_degrees();
    assert!((bank - 40.).abs() < 1e-6, "{bank}");
    // Capped at the corner speed, and unlimited when the target is not below.
    assert_eq!(orbit_speed_fps(10_000., 10_000., 100.), 100.);
    assert_eq!(orbit_speed_fps(0., 10_000., 600.), 600.);
    // A one second horizon: the heading rate that gives a bank at a speed.
    let rate = turn_rate_deg(40., 600.);
    assert!((600. * rate.to_radians() / 32.174).atan().to_degrees() - 40. < 1e-6);
}

/// Flies `strike` for `seconds` with a point-mass aircraft that follows its
/// steering as the input adapter does (a heading error turns into a bank of
/// `atan(v * rate / g)` within 75 degrees, rolled at 120 degrees a second,
/// the speed request met at 20 ft/s^2) and returns, for every tick, the
/// target's elevation in the aircraft's body frame (positive above the wing
/// plane, degrees), the range and the bank.
fn fly(
    strike: &mut Strike,
    world: &mut World,
    seconds: u64,
    mut each: impl FnMut(u64, &mut Strike, &mut World),
) -> Vec<(f64, f64, f64)> {
    let g = 32.174;
    let mut bank: f64 = 0.;
    let mut log = Vec::new();
    for tick in 0..seconds * 120 {
        let own = &world.own;
        let heading = own.heading_deg.to_radians();
        let speed = own.speed.0;
        let velocity = [heading.sin() * speed, 0., heading.cos() * speed];
        each(tick, strike, world);
        let order = {
            let cx = context_at(world, tick, velocity);
            strike.step(&cx)
        };
        let Some(steer) = order.steer else {
            break;
        };
        // The adapter: bank from the heading error over one second.
        let error = angle_difference(world.own.heading_deg, steer.heading_deg).to_radians();
        let wanted = (speed.max(125.) * error / g)
            .atan()
            .to_degrees()
            .clamp(-75., 75.);
        bank += (wanted - bank).clamp(-1., 1.);
        let rate = g * bank.to_radians().tan() / speed.max(1.);
        let own = &mut world.own;
        own.heading_deg = (own.heading_deg + rate.to_degrees() / 120.).rem_euclid(360.);
        let heading = own.heading_deg.to_radians();
        own.position[0] += heading.sin() * speed / 120.;
        own.position[2] += heading.cos() * speed / 120.;
        let wanted_speed = match steer.speed {
            SpeedRequest::Explicit(s) => s.0.max(own.limits.minimum.0),
            SpeedRequest::Corner => own.limits.corner.0,
            SpeedRequest::Maximum => own.limits.maximum.0,
        };
        own.speed.0 += (wanted_speed - speed).clamp(-20. / 120., 20. / 120.);
        own.bank_deg = bank;
        // Where the target is in the body frame.
        let d: Vector = std::array::from_fn(|i| TARGET_AT[i] - own.position[i]);
        let range = d[0].hypot(d[2]);
        let up = {
            let right = [heading.cos(), 0., -heading.sin()];
            let b = bank.to_radians();
            [b.sin() * right[0], b.cos(), b.sin() * right[2]]
        };
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let elevation = ((d[0] * up[0] + d[1] * up[1] + d[2] * up[2]) / length)
            .asin()
            .to_degrees();
        log.push((elevation, range, bank));
    }
    log
}

#[test]
fn a_pod_holds_the_target_under_its_wings_through_a_bombs_fall() {
    // Released 17,000 ft from the target at 12,000 ft and 700 ft/s (the
    // reach of a GBU-10): 50 s of fall.
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 12_000., TARGET_AT[2] - 17_000.], 700.);
    world.own.limits.corner = ScalarSpeed(700.);
    let mut strike = Strike::new(1);
    strike.set_inputs(world.inputs.clone());
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    let log = fly(&mut strike, &mut world, 60, |_, _, _| {});
    assert_eq!(log.len(), 60 * 120, "the hold never gave the flying up");
    // The target stays under the wing plane (a little slack while rolling in
    // and for the one-second bank law) and inside the laser's reach.
    let worst = log.iter().map(|l| l.0).fold(f64::MIN, f64::max);
    assert!(
        worst < 2.,
        "the target rose {worst} degrees above the wings"
    );
    let far = log.iter().map(|l| l.1).fold(0., f64::max);
    assert!(
        far < 60_760.,
        "the target left the laser's reach at {far} ft"
    );
    // It is a turn, not a run: the aircraft ends up having turned about the
    // target.
    let banked = log.iter().filter(|l| l.2.abs() > 10.).count();
    assert!(banked > log.len() / 2, "mostly flying a turn");
    assert!(world.own.position[1] > 11_999.);
}

#[test]
fn the_hold_slows_a_fast_aircraft_so_the_orbit_fits_the_gimbal() {
    // 12,000 ft over the target, an orbit of 3 nm: 700 ft/s needs a 56
    // degree bank (too steep: the target is 33 degrees down); the hold asks
    // for the speed that fits.
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 12_000., TARGET_AT[2] - 17_000.], 700.);
    let mut strike = Strike::new(1);
    strike.set_inputs(world.inputs.clone());
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    let order = strike.step(&world.context(0));
    let steer = order.steer.expect("the hold flies");
    let SpeedRequest::Explicit(speed) = steer.speed else {
        panic!("{:?}", steer.speed);
    };
    let radius = lase_radius_ft(700., 7., 60_760.);
    let bank = (speed.0 * speed.0 / (32.174 * radius)).atan().to_degrees();
    let depression = (12_000f64 / radius).atan().to_degrees();
    assert!(bank < depression, "{bank} against {depression}");
    assert!(speed.0 < 700.);
    assert_eq!(order.activity, Some(Activity::Attacking));
}

#[test]
fn a_cone_holds_its_nose_on_the_target_and_a_duty_turns_away_inside_the_exit() {
    let mut world = lasing_world(Some(cone()));
    let mut strike = Strike::new(1);
    strike.set_inputs(world.inputs.clone());
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    let steer = strike.step(&world.context(0)).steer.unwrap();
    assert_eq!(steer.heading_deg, 0.);
    assert_eq!(steer.speed, SpeedRequest::Corner);
    // Off to the side it points at the target.
    world.own.position = [6_000., 5_000., 0.];
    let steer = strike.step(&world.context(1)).steer.unwrap();
    assert!((steer.heading_deg - bearing_deg(world.own.position, TARGET_AT)).abs() < 1e-9);
    // Inside the exit distance it cannot follow a target it is flying
    // over: the duty exits, and turns back to hold again.
    world.own.position = [0., 5_000., TARGET_AT[2] - 1_500.];
    let order = strike.step(&world.context(2));
    assert_eq!(strike.phase(), Phase::Exit);
    assert!(order.steer.is_some());
    assert_eq!(strike.lasing(), Some(TARGET), "the duty stands");
    world.own.position = [0., 6_000., TARGET_AT[2] + 31_000.];
    strike.step(&world.context(3));
    assert_eq!(strike.phase(), Phase::TurnBack);
    strike.step(&world.context(4));
    assert_eq!(strike.phase(), Phase::Lase);
}

#[test]
fn a_laser_release_with_its_own_designator_holds_the_spot_until_the_bomb_lands() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 8_000., 6_000.], 600.);
    let mut strike = Strike::new(9);
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        ..solution(TARGET_AT, [0., 0., 0.], false)
    });
    // The pass: a level run with the laser wanted.
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.laser_target(), Some(TARGET));
    assert!(strike.selection().unwrap().laser);
    // Due: the bomb goes, and the hold begins instead of the exit.
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        due: true,
        ..solution(TARGET_AT, [0., 0., 0.], true)
    });
    let order = strike.step(&world.context(30));
    assert!(order.release.is_some());
    assert_eq!(strike.phase(), Phase::Lase);
    assert_eq!(strike.laser_target(), Some(TARGET));
    // The bomb flies: still holding, a long way on.
    world.inputs.lase.own_flight = true;
    for tick in 31..120 * 40 {
        let order = strike.step(&world.context(tick));
        assert!(order.steer.is_some() && order.release.is_none(), "{tick}");
        assert_eq!(strike.phase(), Phase::Lase, "{tick}");
    }
    // The bomb lands: the exit.
    world.inputs.lase.own_flight = false;
    let order = strike.step(&world.context(120 * 40));
    assert_eq!(strike.phase(), Phase::Exit);
    assert!(order.steer.is_some());
    assert_eq!(strike.laser_target(), None, "the laser is let go");
    assert!(!strike.selection().unwrap().laser);
}

#[test]
fn the_hold_is_not_judged_finished_in_its_first_two_seconds() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 8_000., 6_000.], 600.);
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        due: true,
        ..solution(TARGET_AT, [0., 0., 0.], true)
    });
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Lase);
    // The bomb is not in the list yet: still holding.
    world.inputs.solution = None;
    strike.step(&world.context(2));
    assert_eq!(strike.phase(), Phase::Lase);
}

#[test]
fn a_carrier_without_a_designator_exits_after_its_release() {
    let mut world = lasing_world(None);
    world.inputs.lase.spot_targets = vec![TARGET];
    world.own = own([0., 8_000., 6_000.], 600.);
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        due: true,
        ..solution(TARGET_AT, [0., 0., 0.], true)
    });
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Exit);
    assert_eq!(strike.laser_target(), None);
}

#[test]
fn a_duty_ends_by_time_by_impact_by_the_target_dying_or_by_a_threat() {
    let ready = |until| {
        let mut world = lasing_world(Some(pod()));
        world.own = own([0., 12_000., TARGET_AT[2] - 20_000.], 600.);
        let mut strike = Strike::new(1);
        strike.set_inputs(world.inputs.clone());
        strike.lase_for(TARGET, until);
        (world, strike)
    };
    // Seconds: from the first step.
    let (world, mut strike) = ready(LaseUntil::Seconds(10.));
    strike.step(&world.context(500));
    assert_eq!(strike.phase(), Phase::Lase);
    strike.step(&world.context(500 + 10 * 120 - 1));
    assert_eq!(strike.phase(), Phase::Lase);
    strike.step(&world.context(500 + 10 * 120));
    assert_ne!(strike.phase(), Phase::Lase);
    assert_eq!(strike.lasing(), None);

    // Impact: after a weapon was seen guiding on the spot and has landed.
    let (mut world, mut strike) = ready(LaseUntil::Impact);
    strike.step(&world.context(0));
    world.inputs.lase.on_spot = true;
    strike.step(&world.context(100));
    world.inputs.lase.on_spot = false;
    strike.step(&world.context(101));
    assert_eq!(strike.phase(), Phase::Lase, "two seconds at least");
    strike.step(&world.context(LASE_MIN_TICKS));
    assert_ne!(strike.phase(), Phase::Lase);
    // With no weapon ever seen it waits two minutes.
    let (world, mut strike) = ready(LaseUntil::Impact);
    strike.step(&world.context(0));
    strike.step(&world.context(120 * 119));
    assert_eq!(strike.phase(), Phase::Lase);
    strike.step(&world.context(120 * 120));
    assert_ne!(strike.phase(), Phase::Lase);

    // The target dying ends any of them.
    let (mut world, mut strike) = ready(LaseUntil::TargetDies);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Lase);
    world.inputs.lase.target = Some((TARGET_AT, false));
    strike.step(&world.context(100));
    assert_ne!(strike.phase(), Phase::Lase);

    // A threat the aircraft has to defend against ends it too.
    let (world, mut strike) = ready(LaseUntil::TargetDies);
    strike.step(&world.context(0));
    let mut cx = world.context(10);
    cx.defending = true;
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Defend);
    assert_eq!(strike.lasing(), None);

    // Cancelled by the caller.
    let (world, mut strike) = ready(LaseUntil::TargetDies);
    strike.step(&world.context(0));
    assert!(strike.stop_lase());
    assert_eq!(strike.lasing(), None);
    assert!(!strike.stop_lase());
    strike.step(&world.context(1));
    assert_ne!(strike.phase(), Phase::Lase);

    // An aircraft with no designator cannot lase.
    let (mut world, mut strike) = ready(LaseUntil::TargetDies);
    world.inputs.lase.sensor = None;
    strike.step(&world.context(0));
    assert_ne!(strike.phase(), Phase::Lase);
}

#[test]
fn a_duty_wants_the_laser_on_its_target_with_no_store() {
    let mut world = lasing_world(Some(pod()));
    world.inputs.stores = Vec::new();
    let mut strike = Strike::new(1);
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    assert_eq!(
        strike.laser_target(),
        Some(TARGET),
        "wanted from the order on"
    );
    strike.step(&world.context(0));
    assert_eq!(strike.laser_target(), Some(TARGET));
    assert_eq!(strike.selection(), None, "no store: nothing to select");
}

#[test]
fn a_designator_with_nothing_to_drop_lases_for_its_wings_carriers() {
    // A designator aircraft, no ground store, a carrier in its wing that has
    // chosen the target.
    let mut world = lasing_world(Some(pod()));
    world.inputs.stores = Vec::new();
    world.own = own([0., 8_000., TARGET_AT[2] - 30_000.], 600.);
    let mut strike = Strike::new(1);
    let mut cx = world.context(0);
    cx.wing = WingFacts {
        designator: false,
        carriers: true,
        escort: None,
    };
    // At the choice there is nothing to strike with: it waits for the
    // carriers rather than going home.
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Wait);
    // A carrier chose the target: the designator lases it.
    let mut cx = world.context(300);
    cx.wing = WingFacts {
        designator: false,
        carriers: true,
        escort: Some(TARGET),
    };
    let order = strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Lase);
    assert_eq!(strike.laser_target(), Some(TARGET));
    assert!(order.steer.is_some());
    // The carriers still want the spot, then their bombs are on it, then it
    // is done.
    let mut cx = world.context(600);
    cx.wing.escort = Some(TARGET);
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Lase);
    world.inputs.lase.on_spot = true;
    let mut cx = world.context(1_000);
    cx.wing.escort = None;
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Lase, "bombs still on its spot");
    world.inputs.lase.on_spot = false;
    let mut cx = world.context(1_200);
    cx.wing.escort = None;
    strike.step(&cx);
    assert_ne!(strike.phase(), Phase::Lase);
    // With no carriers left it goes home instead of waiting.
    let mut cx = world.context(1_400);
    cx.wing = WingFacts::default();
    for tick in [1_400, 1_600, 1_800] {
        cx.tick = tick;
        strike.step(&cx);
    }
    assert_eq!(strike.phase(), Phase::Home);
}

#[test]
fn a_designator_that_has_stores_of_its_own_strikes_with_them() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 8_000., TARGET_AT[2] - 30_000.], 600.);
    let mut strike = Strike::new(1);
    let mut cx = world.context(0);
    cx.wing = WingFacts {
        designator: false,
        carriers: true,
        escort: Some(TARGET),
    };
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.lasing(), None);
}

#[test]
fn a_laser_store_is_chosen_only_with_a_spot_assured() {
    // No designator, nothing else to drop: with no spot it waits while
    // there is room to close on the target, then gives up.
    let mut world = lasing_world(None);
    world.own = own([0., 8_000., 0.], 600.);
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Wait);
    // Waiting, it flies at the target.
    let order = strike.step(&world.context(10));
    assert!(order.steer.is_some());
    // Inside 1.5 fire ranges: nothing to drop, so it goes home.
    world.own.position[2] = TARGET_AT[2] - 12_000.;
    for tick in [300, 600, 900] {
        strike.step(&world.context(tick));
    }
    assert_eq!(strike.phase(), Phase::Home);
    // A laser missile cannot fall unguided: it says so and goes home.
    let call = strike.no_laze().expect("a no-laze decision");
    assert_eq!((call.kind, call.station), (NoLaze::Rtb, StationId(2)));
    assert_eq!(call.target, Some(TARGET));

    // A same-side spot on the target assures it.
    let mut world = lasing_world(None);
    world.own = own([0., 8_000., 0.], 600.);
    world.inputs.lase.spot_targets = vec![TARGET];
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.kind, Some(StoreKind::Laser));
    // A spot on some other object does not.
    let mut world = lasing_world(None);
    world.own = own([0., 8_000., 0.], 600.);
    world.inputs.lase.spot_targets = vec![0x6000_0099];
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Wait);

    // A designator in its wing assures it.
    let mut world = lasing_world(None);
    world.own = own([0., 8_000., 0.], 600.);
    let mut strike = Strike::new(1);
    let mut cx = world.context(0);
    cx.wing.designator = true;
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Attack);
    // A bomb as well as the laser store: bombs first, with no spot.
    let mut world = lasing_world(None);
    world.inputs.stores = vec![laser_store(), bomb(2)];
    world.own = own([0., 8_000., 0.], 600.);
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
}

/// Found at the integration (ST-L x ST-T2): a fighter's gun is no store to
/// strike with, so a fighter with a laser bomb and a gun and no designator
/// is a laser-only carrier: it waits for a spot instead of going home, and
/// drops on one. An attack jet's gun strafes, so it takes the gun.
#[test]
fn a_fighters_gun_does_not_stop_it_waiting_for_a_spot() {
    let gun = Store {
        station: StationId(0),
        kind: StoreKind::Gun,
        rounds: 500,
        cadence_ticks: 4,
        maximum_range_ft: 7_000.,
        ..bomb(500)
    };
    let fighter = |spots: Vec<u32>| {
        let mut world = lasing_world(None);
        world.inputs.class = StrikeClass::Fighter;
        world.inputs.stores = vec![laser_store(), gun];
        world.inputs.lase.spot_targets = spots;
        world.own = own([0., 8_000., 0.], 600.);
        world
    };
    let world = fighter(Vec::new());
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Wait);
    let world = fighter(vec![TARGET]);
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.kind, Some(StoreKind::Laser));
    let mut world = fighter(Vec::new());
    world.inputs.class = StrikeClass::Attack;
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.kind, Some(StoreKind::Gun));
}

#[test]
fn a_glide_bomb_that_cannot_reach_is_approached_in_a_descent_to_its_floor() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 15_000., TARGET_AT[2] - 36_000.], 700.);
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        reach_ok: false,
        ..solution(TARGET_AT, [0., 0., 0.], false)
    });
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    let held = strike.hold_ft.expect("a hold");
    strike.step(&world.context(120));
    assert!(strike.hold_ft.unwrap() < held, "descending");
    assert!((held - strike.hold_ft.unwrap() - GLIDE_DESCENT_FPS / 120.).abs() < 1.);
    // It settles at the release floor over the ground, not below.
    for tick in 120..120 * 60 {
        strike.step(&world.context(tick));
    }
    let floor = strike.hold_ft.unwrap();
    assert!((floor - (15_000. - 15_000. + release_floor_ft(ThreatLevel::Clear))).abs() < 1.);
    // With a reach it holds its height.
    world.inputs.solution = Some(Solution {
        station: StationId(2),
        ..solution(TARGET_AT, [0., 0., 0.], false)
    });
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    let held = strike.hold_ft;
    strike.step(&world.context(120));
    assert_eq!(strike.hold_ft, held);
}

#[test]
fn a_lase_in_progress_round_trips_through_a_checkpoint() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 12_000., TARGET_AT[2] - 20_000.], 600.);
    let mut strike = Strike::new(7);
    strike.lase_for(TARGET, LaseUntil::Seconds(30.));
    for tick in 0..300 {
        strike.step(&world.context(tick));
    }
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let mut twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, strike);
    assert_eq!(twin.phase(), Phase::Lase);
    for tick in 300..600 {
        let a = strike.step(&world.context(tick));
        let b = twin.step(&world.context(tick));
        assert_eq!(a, b, "tick {tick}");
    }
    assert_eq!(twin, strike);
}

#[test]
fn a_gimbal_mask_levels_the_wings_and_the_range_is_kept_inside_the_reach() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 12_000., TARGET_AT[2] - 12_000.], 600.);
    let mut strike = Strike::new(1);
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    strike.step(&world.context(0));
    // Masked by the gimbal: no turn toward the target.
    world.inputs.lase.mask = Some(Mask::Gimbal);
    world.own.heading_deg = 90.;
    let steer = strike.step(&world.context(60)).steer.unwrap();
    let turn = angle_difference(90., steer.heading_deg);
    let side = strike.lase.unwrap().side;
    assert!(turn * side <= 0., "{turn}");
}

#[test]
fn the_wing_facts_a_mate_offers_are_its_designator_its_laser_rounds_and_its_target() {
    // A carrier that has chosen a laser store: it needs a spot on its target.
    let mut world = lasing_world(None);
    world.own = own([0., 8_000., 0.], 600.);
    world.inputs.lase.spot_targets = vec![TARGET];
    let mut carrier = Strike::new(1);
    assert!(!carrier.is_carrier() && !carrier.is_designator());
    carrier.step(&world.context(0));
    assert_eq!(carrier.needs_spot(), Some(TARGET));
    assert!(carrier.is_carrier());
    assert!(!carrier.is_designator());
    // A designator is not a carrier, and needs no spot.
    let world = lasing_world(Some(pod()));
    let mut designator = Strike::new(2);
    designator.step(&world.context(0));
    assert_eq!(designator.needs_spot(), None);
    assert!(designator.is_designator() && !designator.is_carrier());
    // A carrier gone home, or defending, is neither.
    carrier.step(&{
        let mut cx = world.context(10);
        cx.recovering = true;
        cx
    });
    assert!(!carrier.is_carrier());
    assert_eq!(carrier.needs_spot(), None);
}

#[test]
fn a_duty_given_in_the_middle_of_a_defence_resumes_when_the_threat_has_gone() {
    let mut world = lasing_world(Some(pod()));
    world.own = own([0., 12_000., TARGET_AT[2] - 20_000.], 600.);
    let mut strike = Strike::new(1);
    strike.set_inputs(world.inputs.clone());
    let mut cx = world.context(0);
    cx.defending = true;
    strike.step(&cx);
    assert_eq!(strike.phase(), Phase::Defend);
    // The order arrives while it defends: it waits, then holds.
    strike.lase_for(TARGET, LaseUntil::TargetDies);
    assert_eq!(strike.phase(), Phase::Defend);
    strike.step(&world.context(WARNING_TICKS + 1));
    assert_eq!(strike.phase(), Phase::Lase);
    assert_eq!(strike.lasing(), Some(TARGET));
}

#[test]
fn the_buddy_spot_rules_bind_laser_carriers_only() {
    // An anti-radiation carrier needs no spot to take a radar, and a
    // carrier with a laser bomb and a HARM is not held to the spots either.
    for stores in [
        vec![store(0, StoreKind::AntiRadiation)],
        vec![store(0, StoreKind::AntiRadiation), laser_store()],
    ] {
        let world = World {
            own: own([0., 15_000., 0.], 700.),
            picture: picture(
                &[ground(TARGET, TARGET_AT, 0x1000, site(30_000., 0., true))],
                vec![TARGET],
            ),
            inputs: inputs(stores, None),
        };
        let mut strike = Strike::new(1);
        strike.step(&world.context(0));
        assert_eq!(strike.phase(), Phase::Attack);
        assert_eq!(strike.kind, Some(StoreKind::AntiRadiation));
    }
}

#[test]
fn a_drag_bomb_lays_down_clear_of_its_fuze_and_its_fragments() {
    // A Snakeye: it leaves at 66% of the speed, its fuze arms in 1 s, its
    // fragments reach 300 ft. At 700 ft/s it is 34% behind the aircraft,
    // so the floor (500 ft) is safe: 5.6 s of fall, 1,330 ft behind.
    let snakeye = Store {
        release_speed_share: 0.66,
        bomb_body: crate::combat::bomb_flight::Body {
            retarder: 2.4,
            ..Default::default()
        },
        ..bomb(4)
    };
    assert!(snakeye.retarded());
    assert_eq!(laydown_height_ft(&snakeye, 700.), LAYDOWN_MIN_FT);
    // A slick bomb lands under the aircraft: only height clears the 300 ft
    // fragments (with the margin, 330 ft); the floor is higher anyway.
    let slick = bomb(4);
    assert!(!slick.retarded());
    assert_eq!(laydown_height_ft(&slick, 700.), LAYDOWN_MIN_FT);
    // Fragments of 600 ft reach over a slick bomb's 500 ft: 700 ft is the
    // first 50 ft step past 660 ft.
    let big = Store {
        frag_radius_ft: 600.,
        falls_free: false,
        flight: None,
        ..bomb(4)
    };
    assert_eq!(laydown_height_ft(&big, 700.), 700.);
    // A slow arming fuze (3 s): the fall must last 3.6 s, 207 ft... and a
    // 6 s fuze 7.2 s, 830 ft: 850 ft.
    let slow = Store {
        fuze_arm_s: 6.,
        release_speed_share: 0.66,
        bomb_body: crate::combat::bomb_flight::Body {
            retarder: 2.4,
            ..Default::default()
        },
        ..bomb(4)
    };
    assert_eq!(laydown_height_ft(&slow, 700.), 700.);
}

/// The cue's miss across the aircraft's track, feet (positive: the aim lies
/// right), for a bomb thrown `throw_ft` ahead along the heading and blown
/// `drift_ft` to the right on the way down.
fn cue_across(world: &World, aim_point: Vector, throw_ft: f64, drift_ft: f64) -> (Vector, f64) {
    let heading = world.own.heading_deg.to_radians();
    let (sin, cos) = heading.sin_cos();
    let cue = [
        world.own.position[0] + sin * throw_ft + cos * drift_ft,
        0.,
        world.own.position[2] + cos * throw_ft - sin * drift_ft,
    ];
    let across = (aim_point[0] - cue[0]) * cos - (aim_point[2] - cue[2]) * sin;
    (cue, across)
}

/// A fast-high or level bomb run's cue lies 13,500 ft ahead, so 400 ft of
/// offset at the start and a 110 ft drift in the wind must be laid on the aim
/// well before the release: from the fifth second the cue's miss across stays
/// inside the release gate (nr-bombs, 2026-10-13). The slow trim this replaced
/// left it 35 ft off at the fifth second here and swung it 180 ft either
/// side in the battery's run, meeting the aim 44 ft off at the release: outside the gate and, with
/// the drawn aim offset, outside the circle, so the first pass of the
/// battery's `ai-strike-bombs` fast-high run released nothing, turned back
/// and bombed after the run had ended.
#[test]
fn a_level_like_run_lays_its_far_cue_on_the_aim_before_the_release() {
    for pass in [Pass::FastHigh, Pass::Level] {
        let mut world = quiet_world();
        world.own = own([-400., 5_000., 0.], 700.);
        world.own.limits.corner = ScalarSpeed(700.);
        let mut strike = Strike::new(9);
        strike.force(Some(pass), None);
        let gate = (GATE_SHARE * release_circle_ft(Experience::Average)).max(GATE_FLOOR_FT);
        let mut worst = 0_f64;
        let mut first = f64::NAN;
        fly(&mut strike, &mut world, 12, |tick, strike, world| {
            let (aim, _) = strike.aim();
            let aim_point = [aim[0], 0., TARGET_AT[2] + aim[1]];
            let (cue, across) = cue_across(world, aim_point, 13_500., 110.);
            // Far short of the aim the whole time: nothing is due.
            world.inputs.solution = Some(solution(TARGET_AT, cue, false));
            if tick == 1 {
                first = across;
            }
            // From the fifth second on the cue is on the aim.
            if tick >= 600 {
                worst = worst.max(across.abs());
            }
        });
        assert!(first.abs() > 100., "{pass:?} starts {first} ft off");
        assert!(
            worst < gate,
            "{pass:?}: the cue was {worst} ft across the aim late in the run, gate {gate} ft"
        );
    }
}

#[test]
fn a_dive_pulls_out_on_its_plan_and_the_plan_sets_the_floor() {
    // 800 ft/s at 6 G: the pull-out radius is 800² / (32.174 x 5), 3,978 ft.
    let loss = pull_out_loss_ft(800., 6., -60.);
    assert!((loss - (3_978.4 * 0.5 + 800. * 0.866)).abs() < 2., "{loss}");
    assert_eq!(pull_out_loss_ft(800., 6., 10.), 0.);
    // Where the dive pulls out: never below retail's 2,000 ft.
    let mut diving = own([0., 9_000., 0.], 800.);
    diving.g_limit = 6.;
    diving.flight_path_pitch_deg = -60.;
    assert!((dive_pull_out_ft(&diving) - (loss + PULL_OUT_CLEARANCE_FT)).abs() < 1e-9);
    let mut slow = own([0., 9_000., 0.], 300.);
    slow.flight_path_pitch_deg = -10.;
    assert_eq!(dive_pull_out_ft(&slow), DIVE_PULL_OUT_MIN_FT);
    // The planned floor: high up, the steepest dive; near the ground, only
    // a shallow one; below the clearance, a climb.
    diving.terrain_climb_deg = -60.;
    assert_eq!(planned_pitch_floor_deg(&diving), -MAX_DIVE_DEG);
    diving.altitude_msl_ft = 3_000.;
    let low = planned_pitch_floor_deg(&diving);
    assert_eq!(low, -45., "1,731 ft of pull-out fits in 2,000 ft");
    diving.altitude_msl_ft = 900.;
    assert!(planned_pitch_floor_deg(&diving) > 0.);
    // Rising ground's climb is kept; the glide-slope term is not.
    diving.altitude_msl_ft = 9_000.;
    diving.terrain_climb_deg = 12.;
    assert_eq!(planned_pitch_floor_deg(&diving), 12.);
}

#[test]
fn each_gun_burst_has_its_own_seeded_draw() {
    let r = release_circle_ft(Experience::Average);
    let first = burst_draw(7, OWN, 1, 1, r);
    assert_eq!(first, burst_draw(7, OWN, 1, 1, r), "deterministic");
    assert_ne!(first.offset_ft, burst_draw(7, OWN, 1, 2, r).offset_ft);
    assert_ne!(first.offset_ft, burst_draw(7, OWN, 2, 1, r).offset_ft);
    assert_ne!(first.offset_ft, aim_draw(7, OWN, 1, r).offset_ft);
    // The circle and ring hold for bursts as for passes.
    let mut inside = 0;
    for burst in 1..=2_000 {
        let d = burst_draw(11, OWN, 1, burst, r);
        let distance = d.offset_ft[0].hypot(d.offset_ft[1]);
        if d.inside {
            inside += 1;
            assert!(distance <= r + 1e-9);
        } else {
            assert!(distance >= r - 1e-9 && distance <= 2. * r + 1e-9);
        }
    }
    assert!((1_700..=1_900).contains(&inside), "{inside}");
}

#[test]
fn a_gun_run_fires_bursts_on_the_cue_recovers_and_draws_again() {
    let mut world = quiet_world();
    let gun = Store {
        station: StationId(0),
        kind: StoreKind::Gun,
        rounds: 500,
        cadence_ticks: 4,
        maximum_range_ft: 7_000.,
        ..bomb(500)
    };
    world.inputs.class = StrikeClass::Attack;
    world.inputs.stores = vec![gun];
    let mut strike = Strike::new(3);
    strike.force(Some(Pass::Strafe), Some(StationId(0)));
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Strafe));
    assert_eq!(strike.burst(), 1);
    // In the dive, 5,000 ft out, the cue on the target (the run is flown
    // onto the target; the burst's walk is laid on the rounds).
    world.own.position = [0., 1_500., TARGET_AT[2] - 5_000.];
    world.own.flight_path_pitch_deg = -STRAFE_DIVE_DEG;
    let aim = [TARGET_AT[0], 0., TARGET_AT[2]];
    world.inputs.solution = Some(Solution {
        station: StationId(0),
        impact: Some(aim),
        impact_seconds: Some(1.5),
        ..solution(TARGET_AT, aim, false)
    });
    let mut fired = Vec::new();
    for tick in 1..=200 {
        if strike.step(&world.context(tick)).release.is_some() {
            fired.push(tick);
        }
    }
    assert!(!fired.is_empty(), "a burst on the cue");
    // Rounds at the cadence within a burst, at most a 0.5 s burst, then
    // 0.5 s without fire, then the next burst with its own draw.
    let first = fired[0];
    let burst: Vec<u64> = fired
        .iter()
        .copied()
        .take_while(|t| *t < first + 60)
        .collect();
    assert!(burst.windows(2).all(|w| w[1] - w[0] == 4), "{burst:?}");
    assert!(burst.len() <= 15);
    let next = fired.iter().copied().find(|t| *t >= first + 60);
    if let Some(next) = next {
        assert!(next >= first + 120, "recovery: {fired:?}");
        assert!(strike.burst() >= 2);
    }
}

/// A gun run in the dive with the cue held on the burst's aim: the
/// `(tick, offset)` of every round the striker fires over `ticks`.
/// The rounds of a gun run (tick and walked offset) and its bursts' start
/// offsets.
type GunRun = (Vec<(u64, [f64; 2])>, Vec<[f64; 2]>);

fn gun_run(seed: u64, level: Experience, ticks: u64) -> GunRun {
    let mut world = quiet_world();
    let gun = Store {
        station: StationId(0),
        kind: StoreKind::Gun,
        rounds: 500,
        cadence_ticks: 4,
        maximum_range_ft: 7_000.,
        ..bomb(500)
    };
    world.inputs.class = StrikeClass::Attack;
    world.inputs.stores = vec![gun];
    let mut strike = Strike::new(seed);
    strike.force(Some(Pass::Strafe), Some(StationId(0)));
    let step = |world: &World, strike: &mut Strike, tick| {
        let mut cx = world.context(tick);
        cx.experience = level;
        strike.step(&cx)
    };
    step(&world, &mut strike, 0);
    world.own.position = [0., 1_500., TARGET_AT[2] - 5_000.];
    world.own.flight_path_pitch_deg = -STRAFE_DIVE_DEG;
    let mut starts = Vec::new();
    let mut rounds = Vec::new();
    for tick in 1..=ticks {
        let aim = [TARGET_AT[0], 0., TARGET_AT[2]];
        world.inputs.solution = Some(Solution {
            station: StationId(0),
            impact: Some(aim),
            impact_seconds: Some(1.5),
            ..solution(TARGET_AT, aim, false)
        });
        if step(&world, &mut strike, tick).release.is_some() {
            rounds.push((tick, strike.burst_aim(tick, level)));
        }
        if rounds.last().is_some_and(|(t, _)| *t == tick) && starts.len() < strike.burst() as usize
        {
            starts.push(strike.aim().0);
        }
    }
    (rounds, starts)
}

#[test]
fn a_gun_burst_opens_on_its_offset_and_walks_onto_the_target_at_the_skills_pace() {
    // John, 2026-10-12: the burst starts at an offset point and its rounds
    // walk onto the target over the burst; the skill sets the time.
    assert!(gun_walk_in_s(Experience::Ace) < gun_walk_in_s(Experience::Experienced));
    assert!(gun_walk_in_s(Experience::Experienced) < gun_walk_in_s(Experience::Average));
    assert!(gun_walk_in_s(Experience::Average) < gun_walk_in_s(Experience::Novice));
    assert!(
        gun_walk_in_s(Experience::Novice) < 0.5,
        "inside a 0.5 s burst"
    );
    for level in [
        Experience::Ace,
        Experience::Experienced,
        Experience::Average,
        Experience::Novice,
    ] {
        let (rounds, _) = gun_run(5, level, 300);
        let first_burst: Vec<_> = rounds
            .iter()
            .filter(|(t, _)| *t < rounds[0].0 + 60)
            .collect();
        assert!(first_burst.len() >= 3, "{level:?}: {rounds:?}");
        // The first round leaves from the drawn offset (inside the ring)...
        let start = first_burst[0].1;
        let r = release_circle_ft(level);
        assert!(start[0].hypot(start[1]) <= 2. * r + 1e-9, "{level:?}");
        // ...the offset only shrinks along the burst, and is nothing at
        // the end of the walk-in.
        let ticks = (gun_walk_in_s(level) * 120.).round() as u64;
        for pair in first_burst.windows(2) {
            let (a, b) = (pair[0].1, pair[1].1);
            assert!(b[0].hypot(b[1]) <= a[0].hypot(a[1]) + 1e-9, "{level:?}");
        }
        let opened = first_burst[0].0;
        for (tick, offset) in &first_burst {
            if *tick >= opened + ticks {
                assert_eq!(
                    *offset, [0.; 2],
                    "{level:?} on the target after the walk-in"
                );
            }
        }
    }
    // The Ace is on the target sooner than the Novice: more of a burst's
    // rounds leave on it.
    let on_target = |level| {
        let (rounds, _) = gun_run(5, level, 300);
        let first = rounds[0].0;
        rounds
            .iter()
            .filter(|(t, o)| *t < first + 60 && *o == [0.; 2])
            .count()
    };
    assert!(on_target(Experience::Ace) > on_target(Experience::Novice));
}

#[test]
fn a_gun_burst_walks_in_from_short_of_the_target_and_is_deterministic() {
    // The start offset never lies past the target along the track (here
    // +z): a burst closing on the target walks in from short of it.
    for seed in 0..40 {
        let (_, starts) = gun_run(seed, Experience::Novice, 400);
        assert!(!starts.is_empty());
        for offset in starts {
            assert!(offset[1] <= 1e-9, "seed {seed}: {offset:?}");
        }
    }
    assert_eq!(
        gun_run(9, Experience::Average, 300),
        gun_run(9, Experience::Average, 300),
        "the same seed walks the same bursts"
    );
}

/// A laser glide bomb: it falls on the bomb equation when let go with no spot.
fn glide_store() -> Store {
    Store {
        falls_free: true,
        flight: None,
        ..laser_store()
    }
}

/// A laser carrier with no designator, no spot and no other store, 12,000 ft
/// from the target (inside 1.5 fire ranges): it has given up waiting.
fn no_laze_world(store: Store) -> World {
    let mut world = lasing_world(None);
    world.inputs.stores = vec![store];
    world.own = own([0., 8_000., TARGET_AT[2] - 12_000.], 600.);
    world
}

#[test]
fn a_laser_bomb_with_no_laze_support_is_dropped_by_hand_as_an_unguided_bomb() {
    // John, 2026-10-12: "Can laser bombs be fired like a dumb bomb? If so
    // have AI do a manual bomb drop."
    let world = no_laze_world(glide_store());
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    let call = *strike.no_laze().expect("a no-laze decision");
    assert_eq!(call.kind, NoLaze::Manual);
    assert_eq!(call.kind.reason(), "no_laze_manual");
    assert_eq!(call.kind.words(), "No laze support, dropping manual");
    assert_eq!((call.station, call.target), (StationId(2), Some(TARGET)));
    assert!(!call.told);
    // It attacks the target with the laser store as an unguided bomb: a
    // bomb pass with the skill circle's aim and no laser asked for.
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.target(), Some(TARGET));
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
    let selection = strike.selection().expect("a selection");
    assert_eq!(
        (selection.kind, selection.station, selection.laser),
        (StoreKind::Bomb, StationId(2), false)
    );
    assert!(selection.kind.unguided());
    let (aim, _) = strike.aim();
    assert_eq!(
        selection.aim_ft,
        [aim[0] + strike.lead()[0], aim[1] + strike.lead()[1]]
    );
    // The radio says it once.
    assert!(strike.told_no_laze());
    assert!(!strike.told_no_laze());
    assert!(strike.no_laze().unwrap().told);
    // Later passes stay manual.
    for tick in 1..=120 {
        strike.step(&world.context(tick));
    }
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
    assert_eq!(strike.no_laze().map(|c| c.kind), Some(NoLaze::Manual));
}

#[test]
fn a_laser_store_that_cannot_fall_unguided_goes_home_saying_so() {
    // "Otherwise just have it RTB but say it has no laze support."
    let world = no_laze_world(laser_store());
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    let call = *strike.no_laze().expect("a no-laze decision");
    assert_eq!(call.kind, NoLaze::Rtb);
    assert_eq!(call.kind.reason(), "no_laze_rtb");
    assert_eq!(call.kind.words(), "No laze support, RTB");
    assert_eq!((call.station, call.target), (StationId(2), Some(TARGET)));
    assert!(strike.home());
    assert_eq!(strike.target(), None);
    assert!(strike.told_no_laze());
    assert!(!strike.told_no_laze());
    // A mixed load drops the bomb by hand first, then the missile goes home.
    let mut world = no_laze_world(glide_store());
    let missile = store(3, StoreKind::Laser);
    world.inputs.stores = vec![missile, glide_store()];
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(
        strike.no_laze().map(|c| (c.kind, c.station)),
        Some((NoLaze::Manual, StationId(2)))
    );
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
    assert_eq!(
        strike.station,
        Some(StationId(2)),
        "the bomb, not the missile"
    );
    // The bomb is gone: only the missile is left, which cannot fall free.
    world.inputs.stores = vec![missile];
    strike.enter(Phase::TurnBack, 400);
    strike.step(&world.context(500));
    assert_eq!(strike.no_laze().map(|c| c.kind), Some(NoLaze::Rtb));
    assert!(strike.home());
}

#[test]
fn a_spot_ends_the_manual_drop_and_the_sight_guides_again() {
    let world = no_laze_world(glide_store());
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
    // The pass ends and the next choice finds a buddy's spot on the target.
    let mut guided = no_laze_world(glide_store());
    guided.inputs.lase.spot_targets = vec![TARGET];
    strike.enter(Phase::TurnBack, 400);
    strike.step(&guided.context(500));
    assert!(strike.no_laze().is_none(), "the decision is over");
    assert_eq!(strike.kind, Some(StoreKind::Laser), "guided on the spot");
}

#[test]
fn only_a_carrier_with_nothing_else_to_drop_decides_no_laze() {
    // With a bomb as well, it simply takes the bomb: nothing to announce.
    let mut world = no_laze_world(glide_store());
    world.inputs.stores = vec![glide_store(), bomb(2)];
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.kind, Some(StoreKind::Bomb));
    assert!(strike.no_laze().is_none());
    // With its own designator the laser store is guided as before.
    let mut world = lasing_world(Some(pod()));
    world.inputs.stores = vec![glide_store()];
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.kind, Some(StoreKind::Laser));
    assert!(strike.no_laze().is_none());
    // Far from the target it still waits for a spot (no decision yet).
    let mut world = no_laze_world(glide_store());
    world.own = own([0., 8_000., 0.], 600.);
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Wait);
    assert!(strike.no_laze().is_none());
}

#[test]
fn a_no_laze_decision_round_trips_through_a_checkpoint() {
    let world = no_laze_world(glide_store());
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let back: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(back, strike);
    assert_eq!(back.no_laze().map(|c| c.kind), Some(NoLaze::Manual));
}

// The cue passing its closest point to the aim (slice nr-cue, John's answer 12
// of 2026-10-13): the release check solves the closest approach inside the
// tick, so the answer does not depend on the tick rate or on where the ticks
// fall on the cue's path.

/// A cue flying straight past an aim at `cross_ft` to the side, at
/// `speed_fps`, sampled `rate_hz` times a second from `phase` of a tick before
/// the closest point's tick: where it lies from the aim (x across, z along),
/// with the exact time (s) of the closest point, 1.5 s in.
fn cue_path(cross_ft: f64, speed_fps: f64, rate_hz: f64, phase: f64) -> (Vec<[f64; 2]>, f64) {
    let along = |t: f64| speed_fps * (1.5 - t);
    let dt = 1. / rate_hz;
    let first = 1.4 + phase * dt;
    let samples = (0..40)
        .map(|k| [cross_ft, along(first + k as f64 * dt)])
        .collect();
    (samples, 1.5)
}

#[test]
fn the_closest_approach_is_found_once_and_exactly_at_any_tick_rate() {
    for rate in [30., 60., 120., 240.] {
        for phase in [0., 0.17, 0.5, 0.83, 0.999] {
            let (samples, exact_s) = cue_path(126.4, 790., rate, phase);
            let first_s = 1.4 + phase / rate;
            let mut closing = true;
            let mut found = Vec::new();
            for (k, pair) in samples.windows(2).enumerate() {
                let (answer, still) = closest_approach(pair[0], pair[1], closing);
                closing = still;
                if let Some(c) = answer {
                    // The tick's start in seconds, and the closest point's.
                    let at_s = first_s + (k as f64 + c.fraction) / rate;
                    found.push((k, c, at_s));
                }
            }
            assert_eq!(found.len(), 1, "{rate} Hz, phase {phase}: {found:?}");
            let (_, c, at_s) = found[0];
            assert!((c.distance - 126.4).abs() < 1e-9, "{rate} Hz: {c:?}");
            assert!((at_s - exact_s).abs() < 1e-9, "{rate} Hz: {at_s} s");
            assert!(c.fraction > 0. && c.fraction <= 1.);
        }
    }
}

#[test]
fn a_bend_at_a_sample_and_a_cue_that_starts_opening_count_once() {
    // Closing for two ticks, then the path bends at the second sample and
    // opens: the closest point is that sample, found by the tick after it.
    let a = [40., 100.];
    let b = [40., 60.];
    let c = [60., 70.];
    let (first, closing) = closest_approach(a, b, true);
    assert_eq!(first, None);
    assert!(closing, "still closing at the end of the first tick");
    let (second, closing) = closest_approach(b, c, closing);
    let found = second.expect("the bend at b is the closest point");
    assert_eq!(found.fraction, 0.);
    assert_eq!(found.at, b);
    assert!((found.distance - 40_f64.hypot(60.)).abs() < 1e-12);
    assert!(!closing);
    // Nothing more once it is opening.
    assert_eq!(closest_approach(c, [80., 80.], closing).0, None);
    // The first sample may be the closest (the cue appears and opens), a
    // cue that has not moved has none.
    let (start, _) = closest_approach([50., 0.], [60., 0.], true);
    assert_eq!(start.map(|c| c.distance), Some(50.));
    assert_eq!(closest_approach([50., 0.], [50., 0.], true), (None, false));
    // A path that crosses the aim itself is zero away.
    let (through, _) = closest_approach([0., 30.], [0., -10.], true);
    let through = through.unwrap();
    assert!(through.distance.abs() < 1e-12);
    assert!((through.fraction - 0.75).abs() < 1e-12);
}

#[test]
fn the_cue_passing_answer_does_not_depend_on_the_tick_rate() {
    let world = quiet_world();
    let circle = release_circle_ft(Experience::Average);
    // The drawn aim lies 90 ft from the target along the track, so the cue's
    // offset from the led target is not minimal where it is closest to the aim.
    let aim_ft = [20., 88.];
    let decide = |samples: &[[f64; 2]], strike: &mut Strike| {
        let aim = [0., 0., 24_000.];
        let cx = world.context(0);
        samples.iter().any(|r| {
            let impact = [aim[0] + r[0], 0., aim[2] + r[1]];
            strike.cue_passing(&cx, Some(impact), aim)
        })
    };
    for inside_by in [-1.5, 1.5] {
        // The cue passes the aim at `cross` such that, at its closest point,
        // it lies `inside_by` ft inside (positive) or outside the ring.
        let radius = circle + (GATE_SHARE * circle).max(GATE_FLOOR_FT);
        // r = (cross, 0) at the closest point; q = |r + aim_ft|.
        let cross = |c: f64| (c + aim_ft[0]).hypot(aim_ft[1]);
        let (mut low, mut high) = (0., circle);
        for _ in 0..80 {
            let mid = (low + high) / 2.;
            if cross(mid) > radius - inside_by {
                high = mid;
            } else {
                low = mid;
            }
        }
        let want = inside_by > 0.;
        for rate in [30., 60., 120., 240.] {
            for phase in [0., 0.2, 0.4, 0.6, 0.8] {
                let (samples, _) = cue_path(low, 790., rate, phase);
                let mut strike = Strike::new(1);
                strike.aim_ft = aim_ft;
                strike.inside = true;
                strike.gate_ft = (GATE_SHARE * circle).max(GATE_FLOOR_FT);
                assert_eq!(
                    decide(&samples, &mut strike),
                    want,
                    "{rate} Hz, phase {phase}, {inside_by} ft inside the ring"
                );
            }
        }
    }
}

#[test]
fn a_cue_strike_state_with_a_last_sample_round_trips_through_a_checkpoint() {
    let world = quiet_world();
    let mut strike = Strike::new(3);
    strike.aim_ft = [10., -20.];
    strike.inside = true;
    let cx = world.context(5);
    assert!(!strike.cue_passing(&cx, Some([100., 0., 24_300.]), [0., 0., 24_000.]));
    assert!(strike.cue_last.is_some());
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let back: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(back, strike);
    // No cue, no last sample.
    let mut cleared = back;
    assert!(!cleared.cue_passing(&cx, None, [0., 0., 24_000.]));
    assert_eq!(cleared.cue_last, None);
}

#[test]
fn anti_radiation_only_cannot_choose_a_non_radar_vehicle() {
    let subjects = [ground(TARGET, [0., 0., 20_000.], 0x200, None)];
    let world = World {
        own: own([0., 10_000., 0.], 600.),
        picture: picture(&subjects, Vec::new()),
        inputs: inputs(vec![store(1, StoreKind::AntiRadiation)], None),
    };
    let mut strike = Strike::new(1);
    assert_eq!(strike.pick_target(&world.context(0)), None);
    strike.step(&world.context(0));
    assert!(strike.home(), "no usable target ends the assignment");
    for kind in [
        StoreKind::Bomb,
        StoreKind::Rocket,
        StoreKind::Seeker,
        StoreKind::Gun,
    ] {
        let world = World {
            own: own([0., 10_000., 0.], 600.),
            picture: picture(&subjects, Vec::new()),
            inputs: inputs(vec![store(1, kind)], None),
        };
        assert_eq!(
            Strike::new(1).pick_target(&world.context(0)),
            Some(TARGET),
            "{kind:?}"
        );
    }
}

#[test]
fn a_class_order_with_no_usable_store_goes_home_instead_of_waiting_forever() {
    let subjects = [ground(TARGET, [0., 0., 20_000.], 0x200, None)];
    let mut picture = picture(&subjects, Vec::new());
    picture.set_ordered(None);
    picture.set_class_order(vec![TARGET]);
    picture.rebuild(0, Vec::new(), &Subjects::new(&subjects), Vec::new());
    let world = World {
        own: own([0., 10_000., 0.], 600.),
        picture,
        inputs: inputs(vec![store(1, StoreKind::AntiRadiation)], None),
    };
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert!(strike.home());
}

#[test]
fn ordered_wingmen_use_seekers_then_a_level_bomb_pass_without_changing_strike_wings() {
    let subjects = [ground(TARGET, [0., 0., 30_000.], 0x200, None)];
    for role in [StrikeRole::Cap, StrikeRole::Strike] {
        for guided in [true, false] {
            let mut picture = picture(&subjects, Vec::new());
            picture.set_role(role);
            let mut stores = vec![bomb(2)];
            if guided {
                stores.push(store(2, StoreKind::Seeker));
            }
            let world = World {
                own: own([0., 7_800., 0.], 600.),
                picture,
                inputs: inputs(stores, None),
            };
            let mut strike = Strike::new(1);
            strike.step(&world.context(0));
            if role == StrikeRole::Cap {
                assert_eq!(
                    strike.kind,
                    Some(if guided {
                        StoreKind::Seeker
                    } else {
                        StoreKind::Bomb
                    })
                );
                assert_eq!(
                    strike.pass(),
                    Some(if guided { Pass::FastHigh } else { Pass::Level })
                );
            } else {
                assert_eq!(strike.kind, Some(StoreKind::Bomb));
                assert_eq!(strike.pass(), Some(Pass::Straight));
            }
        }
    }
}

#[test]
fn ordered_slick_bomb_releases_from_level_pass_when_shared_solution_is_due() {
    let target = [0., 0., 20_000.];
    let subjects = [ground(TARGET, target, 0x200, None)];
    let mut picture = picture(&subjects, Vec::new());
    picture.set_role(StrikeRole::Cap);
    let mut world = World {
        own: own([0., 7_800., 0.], 600.),
        picture,
        inputs: inputs(vec![bomb(2)], None),
    };
    let mut strike = Strike::new(1);
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Level));
    world.inputs.solution = Some(solution(target, target, true));
    assert!(
        strike.step(&world.context(1)).release.is_none(),
        "the synthetic 20,000 ft weapon zone still applies"
    );
    world.own.position[2] = 5_000.;
    let order = strike.step(&world.context(2));
    assert_eq!(
        order.release.map(|r| (r.station, r.target)),
        Some((StationId(1), TARGET))
    );
}

#[test]
fn bomb_cue_steering_uses_ground_track_and_corrects_impact_error() {
    let mut world = quiet_world();
    world.own.heading_deg = 10.;
    let cx = world.context(0);
    for pass in [Pass::Dive, Pass::Level, Pass::FastHigh, Pass::Laydown] {
        let mut strike = Strike::new(9);
        strike.kind = Some(StoreKind::Bomb);
        strike.pass = Some(pass);
        let aim = [0., 0., 10_000.];
        let aligned = strike.line_heading(&cx, aim, aim, false);
        assert!(
            aligned.abs() < 1e-9,
            "aligned velocity cue must not follow nose offset"
        );
        let corrected = strike.line_heading(&cx, aim, [-50., 0., 10_000.], false);
        assert!((0.0..1.).contains(&corrected), "{pass:?}: {corrected}");
    }
}

// Early retarget and the committed-to-release window (T.O.R.E, John,
// 2026-10-28).

const OTHER: u32 = 0x5000_0002;
const OTHER_AT: Vector = [2_000., 0., 26_000.];

/// A striker briefed on two trucks, `TARGET` nearer, nothing around.
fn two_target_world() -> World {
    two_target_world_at(OTHER_AT)
}

fn two_target_world_at(other_at: Vector) -> World {
    let subjects = [
        ground(TARGET, TARGET_AT, 0x200, None),
        ground(OTHER, other_at, 0x200, None),
    ];
    let mut briefed = GroundPicture::new(StrikeRole::Strike);
    briefed.set_objective(StrikeObjective {
        targets: vec![TARGET, OTHER],
        defenses: Vec::new(),
        point: Some(TARGET_AT),
    });
    briefed.rebuild(0, Vec::new(), &Subjects::new(&subjects), Vec::new());
    World {
        own: own([0., 5_000., 0.], 700.),
        picture: briefed,
        inputs: inputs(vec![bomb(4)], None),
    }
}

/// [`two_target_world`] once its picture knows `TARGET` has fallen.
fn two_target_world_dead() -> World {
    let mut fallen = ground(TARGET, TARGET_AT, 0x200, None);
    fallen.object.destroyed = true;
    let subjects = [fallen, ground(OTHER, OTHER_AT, 0x200, None)];
    let mut world = two_target_world();
    world
        .picture
        .rebuild(0, Vec::new(), &Subjects::new(&subjects), Vec::new());
    world
}

/// The host's word that `TARGET` has fallen (the picture may not know yet).
fn target_dead(world: &mut World) {
    let mut dead = solution(TARGET_AT, [0., 0., 0.], false);
    dead.target_alive = false;
    world.inputs.solution = Some(dead);
}

#[test]
fn a_target_killed_before_the_release_is_followed_by_the_next_at_once() {
    let mut world = two_target_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::Attack, Some(TARGET))
    );
    // A wingman kills it 20,000 ft out, the pass not yet released: at its
    // cruise height the striker attacks the next truck, 22,000 ft ahead, at
    // once, without flying the exit (the picture still lists the dead truck
    // standing).
    world.own.position = [0., 5_000., 4_000.];
    target_dead(&mut world);
    let order = strike.step(&world.context(120));
    assert_eq!(
        (strike.phase(), strike.target(), strike.pass_number()),
        (Phase::Attack, Some(OTHER), 1)
    );
    assert_eq!(order.activity, Some(Activity::Attacking));
    assert_eq!(strike.selection().map(|s| s.target), Some(OTHER));

    // Down at 3,500 ft from the old run: it turns toward the next truck now,
    // climbing back to its cruise height, and chooses there.
    let mut world = two_target_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    world.own.position = [0., 3_500., 4_000.];
    world.own.agl_ft = 3_500.;
    target_dead(&mut world);
    let order = strike.step(&world.context(120));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::TurnBack, Some(OTHER))
    );
    assert!(order.steer.is_some_and(|s| s.pitch_deg > 0.), "it climbs");
    // By then the picture knows the first truck has fallen.
    world.picture = two_target_world_dead().picture;
    world.inputs.solution = None;
    world.own.position = [0., 4_800., 8_000.];
    world.own.agl_ft = 4_800.;
    strike.step(&world.context(1_200));
    assert_eq!(
        (strike.phase(), strike.target(), strike.pass_number()),
        (Phase::Attack, Some(OTHER), 1)
    );

    // Killed when the next truck is too near to set a pass up: the exit on
    // the heading, turning back at half the exit distance from it, not the
    // full 30,000 ft.
    let mut world = two_target_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    world.own.position = [0., 4_000., 18_000.];
    target_dead(&mut world);
    strike.step(&world.context(120));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::Exit, Some(OTHER))
    );
    world.inputs.solution = None;
    world.own.position = [0., 6_000., OTHER_AT[2] + 16_000.];
    strike.step(&world.context(2_000));
    assert_eq!(strike.phase(), Phase::TurnBack);
    world.own.heading_deg = 180.;
    strike.step(&world.context(2_100));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::Attack, Some(OTHER))
    );

    // Far enough but behind: it turns back toward it at once.
    let mut world = two_target_world_at([2_000., 0., -26_000.]);
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    assert_eq!(strike.target(), Some(TARGET));
    world.own.position = [0., 4_000., 4_000.];
    target_dead(&mut world);
    strike.step(&world.context(120));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::TurnBack, Some(OTHER))
    );
}

#[test]
fn a_striker_past_its_release_point_or_with_no_target_left_keeps_the_exit() {
    // The cue already beyond the aim when the target dies: the normal exit,
    // on to the full exit distance.
    let mut world = two_target_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    let aim = strike.aim().0;
    world.own.position = [0., 4_000., 14_000.];
    world.inputs.solution = Some(solution(
        TARGET_AT,
        [aim[0], 0., TARGET_AT[2] + aim[1] + 500.],
        false,
    ));
    strike.step(&world.context(10));
    assert!(strike.release_in_s().is_some_and(|t| t < 0.));
    target_dead(&mut world);
    strike.step(&world.context(20));
    assert_eq!(
        (strike.phase(), strike.target()),
        (Phase::Exit, Some(TARGET))
    );
    world.inputs.solution = None;
    world.own.position = [0., 6_000., TARGET_AT[2] + 20_000.];
    strike.step(&world.context(2_000));
    assert_eq!(strike.phase(), Phase::Exit, "the full exit distance");

    // With no other target the striker exits as before
    // (`a_dead_target_or_no_store_sends_it_home` follows it home).
    let mut world = quiet_world();
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    target_dead(&mut world);
    strike.step(&world.context(10));
    assert_eq!(strike.phase(), Phase::Exit);
}

/// A straight Mk 82 run whose cue closes on the aim at the ground speed,
/// 700 ft/s, from `seconds` short: steps every 0.1 s until the cue is
/// `until` seconds short, the host saying due only at the aim.
fn close_cue(
    strike: &mut Strike,
    world: &mut World,
    tick: &mut u64,
    seconds: f64,
    until: f64,
    defending: bool,
) -> Order {
    let aim = strike.aim().0;
    let mut short = seconds;
    let mut order = None;
    while short > until - 1e-9 {
        let along = short * 700.;
        let impact = [aim[0], 0., TARGET_AT[2] + aim[1] - along];
        world.inputs.solution = Some(solution(TARGET_AT, impact, along <= 0.));
        let mut cx = world.context(*tick);
        cx.defending = defending;
        order = Some(strike.step(&cx));
        *tick += 12;
        short -= 0.1;
    }
    order.expect("at least one step")
}

#[test]
fn the_last_five_seconds_of_a_run_hold_through_a_defence_and_then_it_defends() {
    let mut world = quiet_world();
    world.own.position = [0., 3_000., 8_000.];
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Straight));
    let mut tick = 12;
    // Six seconds out the run is not committed: a defence breaks it.
    close_cue(&mut strike, &mut world, &mut tick, 8., 6., false);
    assert!(!strike.holds_run(tick));
    let mut early = strike.clone();
    let mut cx = world.context(tick);
    cx.defending = true;
    early.step(&cx);
    assert_eq!(early.phase(), Phase::Defend);
    // Five seconds out it is.
    close_cue(&mut strike, &mut world, &mut tick, 5.9, 4.9, false);
    assert!(strike.holds_run(tick));
    assert!(
        strike
            .release_in_s()
            .is_some_and(|t| (4.5..=5.).contains(&t))
    );
    // A defence trigger now does not break it: it flies on and releases.
    let order = close_cue(&mut strike, &mut world, &mut tick, 4.8, 0.1, true);
    assert_eq!(strike.phase(), Phase::Attack);
    assert!(order.steer.is_some() && order.release.is_none());
    let order = close_cue(&mut strike, &mut world, &mut tick, 0., 0., true);
    assert_eq!(
        order.release,
        Some(Release {
            station: StationId(1),
            target: TARGET
        })
    );
    assert!(!strike.holds_run(tick));
    // Right after the release it reacts.
    let mut cx = world.context(tick);
    cx.defending = true;
    let order = strike.step(&cx);
    assert_eq!((strike.phase(), order.steer), (Phase::Defend, None));
}

#[test]
fn the_window_holds_no_longer_than_its_limit() {
    let mut world = quiet_world();
    world.own.position = [0., 3_000., 8_000.];
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    let mut tick = 12;
    close_cue(&mut strike, &mut world, &mut tick, 6., 3., false);
    let opened = strike.committed.expect("the window is open");
    assert!(strike.holds_run(tick));
    // The cue stalls 3 s short: the run holds until the limit, then the
    // defence breaks it.
    while strike.phase() == Phase::Attack && tick < opened + 2 * COMMIT_HOLD_TICKS {
        let mut cx = world.context(tick);
        cx.defending = true;
        strike.step(&cx);
        tick += 12;
    }
    assert!(tick - 12 > opened + COMMIT_HOLD_TICKS);
    assert!(tick - 12 <= opened + COMMIT_HOLD_TICKS + 12);
    assert_eq!(strike.phase(), Phase::Defend);
}

#[test]
fn a_committed_run_round_trips_through_a_checkpoint() {
    let mut world = quiet_world();
    world.own.position = [0., 3_000., 8_000.];
    let mut strike = Strike::new(9);
    strike.step(&world.context(0));
    let mut tick = 12;
    close_cue(&mut strike, &mut world, &mut tick, 6., 4., false);
    assert!(strike.holds_run(tick));
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, strike);
    assert!(twin.holds_run(tick));
}
