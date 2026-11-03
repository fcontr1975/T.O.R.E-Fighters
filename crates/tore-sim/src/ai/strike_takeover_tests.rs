//! Takeover (docs/spec/ai-strike.md, "Takeover and handback"; slice ST-K):
//! the kept store and bomb sight mode, the safe test before every choice
//! and the stand-off, the guiding hold, the end of a takeover and the
//! checkpoint. Synthetic pictures and solutions only.
use super::super::*;
use super::*;
use crate::ai::awareness::ground::{
    DefenseFacts, GroundPicture, GroundSubject, RecentWarnings, Subjects,
};
use crate::ai::engagement::{StrikeObjective, StrikeRole};
use crate::ai::{ScalarSpeed, SpeedLimits};
use crate::checkpoint::{Models, from_bytes, to_bytes};
use crate::sensors::GroundObject;

const TARGET: u32 = 0x5000_0001;
const SAM: u32 = 0x20;
const OWN: u32 = 1;
const TARGET_AT: Vector = [0., 0., 24_000.];

fn own(position: Vector) -> OwnState {
    OwnState {
        position,
        heading_deg: 0.,
        flight_path_pitch_deg: 0.,
        body_pitch_offset_deg: 2.,
        bank_deg: 0.,
        speed: ScalarSpeed(700.),
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

fn ground(id: u32, position: Vector, class: u16, defense: Option<DefenseFacts>) -> GroundSubject {
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

/// A SAM site with a 30,000 ft missile reach 2,000 ft beyond the target.
fn sam() -> GroundSubject {
    ground(
        SAM,
        [0., 0., 26_000.],
        CLASS_SAM,
        Some(DefenseFacts {
            missile_reach_ft: 30_000.,
            rotor_reach_ft: 30_000.,
            gun_reach_ft: 0.,
            radar: true,
            working: true,
            ship: false,
            heading_deg: 0.,
        }),
    )
}

/// A picture ordered onto `TARGET` (the takeover's order), briefed on the
/// defences among `subjects`.
fn picture(subjects: &[GroundSubject]) -> GroundPicture {
    let mut picture = GroundPicture::new(StrikeRole::Strike);
    picture.set_ordered(Some(TARGET));
    picture.set_objective(StrikeObjective {
        targets: Vec::new(),
        defenses: subjects
            .iter()
            .filter(|s| s.defense.is_some())
            .map(|s| s.object.id)
            .collect(),
        point: None,
    });
    picture.rebuild(0, Vec::new(), &Subjects::new(subjects), Vec::new());
    picture
}

fn store(station: u8, kind: StoreKind, rounds: u32) -> Store {
    Store {
        station: StationId(station),
        kind,
        rounds,
        cadence_ticks: 30,
        fire_range_ft: 10_000.,
        minimum_range_ft: 0.,
        maximum_range_ft: 40_000.,
        round_speed_fps: None,
        release_speed_share: 1.,
        bomb_body: Default::default(),
        fuze_arm_s: 0.,
        frag_radius_ft: 0.,
        falls_free: false,
        flight: None,
    }
}

fn solution(station: u8, impact: Option<Vector>, due: bool) -> Solution {
    Solution {
        station: StationId(station),
        target: TARGET,
        target_position: TARGET_AT,
        target_velocity: [0.; 3],
        target_alive: true,
        impact,
        impact_seconds: Some(20.),
        due,
        warning: false,
        guided_in_flight: false,
        reach_ok: true,
        release_seconds: None,
        reaches: false,
        bar_ft: None,
    }
}

struct World {
    own: OwnState,
    picture: GroundPicture,
    inputs: Inputs,
}

impl World {
    /// A laser bomber at 10,000 ft, 4 nm south of the target and heading
    /// at it: Mk 82s on station 1, GBU-10s on station 2, its own pod.
    fn new() -> Self {
        let mut own = own([0., 10_000., 0.]);
        own.agl_ft = 10_000.;
        Self {
            own,
            picture: picture(&[ground(TARGET, TARGET_AT, 0x200, None)]),
            inputs: Inputs {
                class: StrikeClass::Fighter,
                stores: vec![store(1, StoreKind::Bomb, 6), store(2, StoreKind::Laser, 2)],
                designator: true,
                mission_height_ft: None,
                solution: None,
                lase: LaseInputs::default(),
                shots: Shots::default(),
            },
        }
    }
    fn context(&self, tick: u64) -> Context<'_> {
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

fn takeover(station: Option<u8>, mode: DeliveryMode) -> Strike {
    Strike::taken_over(9, Takeover::new(TARGET, station.map(StationId), mode))
}

#[test]
fn a_takeover_keeps_the_human_s_store_and_its_lvl_flies_a_level_run() {
    let world = World::new();
    // The laser bomb in LVL: a level run with it, at the target, laser on.
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    assert_eq!(strike.target(), Some(TARGET), "aimed from the first step");
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::Level));
    let selection = strike.selection().unwrap();
    assert_eq!(
        (selection.target, selection.station, selection.kind),
        (TARGET, StationId(2), StoreKind::Laser)
    );
    assert!(selection.laser);
    // The Mk 82 in LVL: a level run too, where the table would pick a dive
    // or a straight run.
    let mut strike = takeover(Some(1), DeliveryMode::Level);
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Level));
    assert_eq!(strike.selection().unwrap().mode, DeliveryMode::Level);
    // In CCIP the table chooses (Clear, no dive gate from 10,000 ft at
    // 4 nm: a dive).
    let mut strike = takeover(Some(1), DeliveryMode::Ccip);
    strike.step(&world.context(0));
    let table = choose_pass(PassInputs {
        class: StrikeClass::Fighter,
        kind: StoreKind::Bomb,
        threat: ThreatLevel::Clear,
        energy_good: energy_good(&world.own),
        dive_gate: dive_gate(10_000., 24_000., turn_radius_ft(700., 7.)),
        retarded: false,
    });
    assert_eq!(PassChoice::Pass(strike.pass().unwrap()), table);
    assert_eq!(strike.selection().unwrap().station, StationId(1));
}

#[test]
fn an_emptied_or_unusable_kept_station_leaves_the_store_to_the_ai() {
    let mut world = World::new();
    // The kept laser bombs are gone: the AI's own choice (bombs first).
    world.inputs.stores[1].rounds = 0;
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    strike.step(&world.context(0));
    assert_eq!(strike.selection().unwrap().station, StationId(1));
    // Laser bombs with no designator of its own are not the AI's to drop
    // (decision 9) unless a spot is assured: it waits for one while it
    // closes to 1.5 fire ranges (slice ST-L), then has nothing usable left.
    let mut world = World::new();
    world.inputs.designator = false;
    world.inputs.stores[0].rounds = 0;
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    strike.step(&world.context(0));
    assert_eq!(strike.phase(), Phase::Wait, "waits for a buddy's spot");
    assert!(strike.selection().is_none());
    world.own.position = [0., 10_000., TARGET_AT[2] - 12_000.];
    strike.step(&world.context(100_000));
    assert!(strike.home(), "nothing usable left");
    assert!(strike.takeover_done());
}

#[test]
fn an_unsafe_takeover_stands_off_and_resumes_once_safe() {
    let mut world = World::new();
    world.picture = picture(&[ground(TARGET, TARGET_AT, 0x200, None), sam()]);
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    let order = strike.step(&world.context(0));
    // The SAM covers the target: no pass; it turns away and stands off
    // outside the SAM's reach plus a mile.
    assert_eq!(strike.phase(), Phase::Exit);
    assert_eq!(strike.pass(), None);
    let radius = 2_000. + 30_000. + STAND_OFF_MARGIN_FT;
    assert_eq!(strike.takeover().unwrap().stand_off_ft(), Some(radius));
    let steer = order.steer.unwrap();
    assert!((steer.heading_deg - 180.).abs() < 1e-9, "{steer:?}");
    // Out on the circle: a left turn about the target (tangent, the target
    // on the left), still not attacking.
    world.own.position = [0., 10_000., 24_000. - radius];
    let steer = strike.step(&world.context(600)).steer.unwrap();
    assert!((steer.heading_deg - 90.).abs() < 1e-9, "{steer:?}");
    assert_eq!(steer.speed, SpeedRequest::Corner);
    assert_eq!(strike.phase(), Phase::Exit);
    // Farther out it heads in at 45 degrees off the bearing.
    world.own.position = [0., 10_000., 24_000. - 2. * radius];
    let steer = strike.step(&world.context(700)).steer.unwrap();
    assert!((steer.heading_deg - 45.).abs() < 1e-9, "{steer:?}");
    // The SAM falls: safe, it turns back and attacks with the kept store
    // and mode.
    let mut fallen = sam();
    fallen.object.destroyed = true;
    world.picture = picture(&[ground(TARGET, TARGET_AT, 0x200, None), fallen]);
    world.own.position = [0., 10_000., 24_000. - radius];
    strike.step(&world.context(800));
    assert_eq!(strike.phase(), Phase::TurnBack);
    assert_eq!(strike.takeover().unwrap().stand_off_ft(), None);
    strike.step(&world.context(801));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::Level));
    assert_eq!(strike.selection().unwrap().station, StationId(2));
}

#[test]
fn an_ordinary_striker_still_goes_home_where_a_takeover_stands_off() {
    // The same defence and SAM as ST-T's own test: a striker that is not
    // a takeover goes home from the exit distance.
    for taken in [false, true] {
        let mut world = World::new();
        let mut strike = if taken {
            takeover(Some(2), DeliveryMode::Level)
        } else {
            Strike::new(9)
        };
        strike.step(&world.context(0));
        assert_eq!(strike.phase(), Phase::Attack);
        world.picture = picture(&[ground(TARGET, TARGET_AT, 0x200, None), sam()]);
        let mut cx = world.context(10);
        cx.defending = true;
        strike.step(&cx);
        strike.step(&world.context(11));
        assert_eq!(strike.phase(), Phase::Exit);
        world.own.position = [0., 10_000., -10_000.];
        strike.step(&world.context(1_000));
        if taken {
            assert_eq!(strike.phase(), Phase::Exit);
            assert!(strike.takeover().unwrap().stand_off_ft().is_some());
        } else {
            assert_eq!(strike.phase(), Phase::Home);
        }
    }
}

#[test]
fn a_guiding_laser_weapon_holds_a_level_line_and_no_second_leaves() {
    let mut world = World::new();
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    strike.step(&world.context(0));
    assert_eq!(strike.pass(), Some(Pass::Level));
    // The human's bomb is already guiding: due, but no second bomb.
    let mut guiding = solution(2, None, true);
    guiding.guided_in_flight = true;
    world.inputs.solution = Some(guiding);
    world.own.position = [0., 10_000., 6_000.];
    assert_eq!(strike.step(&world.context(10)).release, None);
    // With nothing in flight the bomb leaves and the run exits.
    world.inputs.solution = Some(solution(2, None, true));
    let order = strike.step(&world.context(20));
    assert_eq!(
        order.release,
        Some(Release {
            station: StationId(2),
            target: TARGET
        })
    );
    assert_eq!(strike.phase(), Phase::Exit);
    // While it guides the exit holds the line level at corner speed, no
    // jink, no climb.
    world.inputs.solution = Some(guiding);
    world.own.position = [0., 10_000., 20_000.];
    for tick in [21, 21 + JINK_LEG_TICKS, 21 + 2 * JINK_LEG_TICKS] {
        let steer = strike.step(&world.context(tick)).steer.unwrap();
        assert_eq!(
            (steer.heading_deg, steer.pitch_deg, steer.speed),
            (0., 0., SpeedRequest::Corner),
            "tick {tick}"
        );
    }
    // Once it has ended, the ordinary exit: a climbing jink.
    world.inputs.solution = Some(solution(2, None, false));
    let steer = strike.step(&world.context(1_000)).steer.unwrap();
    assert_eq!(
        (steer.pitch_deg, steer.speed),
        (JINK_CLIMB_DEG, SpeedRequest::Maximum)
    );
}

#[test]
fn a_takeover_round_trips_through_a_checkpoint_and_steps_on_identically() {
    let mut world = World::new();
    world.picture = picture(&[ground(TARGET, TARGET_AT, 0x200, None), sam()]);
    let mut strike = takeover(Some(2), DeliveryMode::Level);
    for tick in 0..200 {
        world.own.position[2] -= 50.;
        strike.step(&world.context(tick));
    }
    assert!(strike.takeover().unwrap().stand_off_ft().is_some());
    let models = Models::default();
    let coded = to_bytes(&strike, &models).unwrap();
    let mut twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, strike);
    for tick in 200..400 {
        world.own.position[2] -= 50.;
        assert_eq!(
            strike.step(&world.context(tick)),
            twin.step(&world.context(tick)),
            "tick {tick}"
        );
    }
    assert_eq!(twin, strike);
}
