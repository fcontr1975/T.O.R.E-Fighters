//! The helicopter family (docs/spec/ai-strike.md, "Helicopters"): the pass
//! and store choice, the pop-up without the speed bleed, the AT-2 held to
//! impact, the stand-off attack under SAMs, the rocket burst and
//! determinism. Synthetic pictures and solutions only.
use super::super::tests::{TARGET, TARGET_AT, World, ground, inputs, own, picture, site, solution};
use super::super::{Order, Phase, Segment, Store, StoreKind, Strike};
use super::*;
use crate::ai::weapon_service::StationId;
use crate::checkpoint::{Models, from_bytes, to_bytes};
use crate::models::strike_class::StrikeClass;

fn at2(rounds: u32) -> Store {
    Store {
        station: StationId(1),
        kind: StoreKind::Command,
        rounds,
        cadence_ticks: 30,
        fire_range_ft: 10_000.,
        minimum_range_ft: 500.,
        maximum_range_ft: 18_000.,
        round_speed_fps: None,
        release_speed_share: 1.,
        bomb_body: Default::default(),
        fuze_arm_s: 0.,
        frag_radius_ft: 0.,
        falls_free: false,
        flight: None,
    }
}

fn rockets(rounds: u32) -> Store {
    Store {
        kind: StoreKind::Rocket,
        maximum_range_ft: 12_000.,
        minimum_range_ft: 50.,
        round_speed_fps: Some(2_000.),
        release_speed_share: 1.,
        bomb_body: Default::default(),
        fuze_arm_s: 0.,
        frag_radius_ft: 0.,
        falls_free: false,
        flight: None,
        ..at2(rounds)
    }
}

/// A Mi-24 at 1,500 ft, 24,000 ft south of the truck, heading north.
fn heli_world(store: Store) -> World {
    let mut inputs = inputs(vec![store], None);
    inputs.class = StrikeClass::Helicopter;
    World {
        own: own([0., 1_500., 0.], 200.),
        picture: picture(&[ground(TARGET, TARGET_AT, 0x200, None)], Vec::new()),
        inputs,
    }
}

/// The strike speed every leg asks for (slice HF1).
fn strike_speed(world: &World) -> SpeedRequest {
    SpeedRequest::Explicit(crate::ai::helicopter_defense::strike_speed(
        &world.own.limits,
    ))
}

fn steer(order: &Order) -> super::super::Steer {
    order.steer.expect("the helicopter steers")
}

#[test]
fn the_pass_and_the_store_are_fixed_functions_of_store_and_threat() {
    use crate::ai::awareness::ground::ThreatLevel as T;
    assert_eq!(choose_pass(StoreKind::Command, T::Missiles), Pass::StandOff);
    assert_eq!(choose_pass(StoreKind::Command, T::Guns), Pass::PopUp);
    assert_eq!(choose_pass(StoreKind::Command, T::Clear), Pass::PopUp);
    assert_eq!(choose_pass(StoreKind::Rocket, T::Missiles), Pass::PopUp);
    let both = [rockets(38), at2(4)];
    assert_eq!(choose_store(&both, false).unwrap().kind, StoreKind::Command);
    assert_eq!(
        choose_store(&[rockets(38), at2(0)], false).unwrap().kind,
        StoreKind::Rocket
    );
    let laser = Store {
        kind: StoreKind::Laser,
        ..at2(2)
    };
    assert!(choose_store(&[laser], false).is_none());
    assert_eq!(choose_store(&[laser], true).unwrap().kind, StoreKind::Laser);
}

#[test]
fn the_at2_popup_keeps_its_speed_and_holds_the_line_to_impact() {
    let mut world = heli_world(at2(4));
    let mut strike = Strike::new(3);
    // The choice: the pop-up, toward 1,500 ft over the target.
    let order = strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::PopUp));
    assert_eq!(strike.segment(), Segment::Approach);
    assert_eq!(steer(&order).speed, strike_speed(&world));
    // Inside 10,000 ft: the pull, 30 degrees over the line of sight, at
    // its speed (no retail bleed).
    world.own.position = [0., 1_500., TARGET_AT[2] - 9_000.];
    let order = strike.step(&world.context(2));
    assert_eq!(strike.segment(), Segment::Pull);
    let pull = steer(&order);
    assert_eq!(pull.speed, strike_speed(&world));
    let line = -(1_500_f64).atan2(9_000.).to_degrees();
    assert!(
        (pull.pitch_deg - (line + 30.)).abs() < 1e-6,
        "{}",
        pull.pitch_deg
    );
    // The shared release test passes: the AT-2 leaves.
    world.inputs.solution = Some(solution(TARGET_AT, TARGET_AT, true));
    let order = strike.step(&world.context(3));
    let release = order.release.expect("the AT-2 leaves");
    assert_eq!((release.station, release.target), (StationId(1), TARGET));
    assert_eq!(strike.segment(), Segment::Guide);
    // While it flies the nose stays on the target, and nothing else leaves.
    world.own.position = [400., 1_500., TARGET_AT[2] - 8_000.];
    for tick in 4..400 {
        world.inputs.solution = Some(Solution {
            guided_in_flight: true,
            ..solution(TARGET_AT, TARGET_AT, true)
        });
        let order = strike.step(&world.context(tick));
        assert!(order.release.is_none());
        assert_eq!(strike.segment(), Segment::Guide);
        let bearing = bearing_deg(world.own.position, TARGET_AT);
        assert!((steer(&order).heading_deg - bearing).abs() < 1e-9);
    }
    // Gone: the exit, at its strike speed, turning away from the target
    // and down to the masking height (slice HF1), not on over it.
    world.inputs.solution = Some(solution(TARGET_AT, TARGET_AT, false));
    strike.step(&world.context(400));
    let order = strike.step(&world.context(401));
    assert_eq!(strike.phase(), Phase::Exit);
    let exit = steer(&order);
    assert_eq!(exit.speed, strike_speed(&world));
    let away = bearing_deg(TARGET_AT, world.own.position);
    assert!(
        angle_difference(exit.heading_deg, away).abs() < 1e-6,
        "{} vs {away}",
        exit.heading_deg
    );
    assert!(exit.pitch_deg < 0., "masking: {}", exit.pitch_deg);
}

#[test]
fn under_a_sam_it_attacks_from_low_at_the_weapons_range() {
    // A short-range SAM by the target (its ring, 5,000 ft plus the 2,000 ft
    // margin, covers the target but not the firing point 10,800 ft out:
    // slice H4 lets the stand-off through).
    let mut world = heli_world(at2(4));
    const SAM: u32 = 0x5000_0009;
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(
                SAM,
                [3_000., 0., TARGET_AT[2]],
                0x1000,
                site(5_000., 0., true),
            ),
        ],
        vec![SAM],
    );
    let mut strike = Strike::new(3);
    let order = strike.step(&world.context(1));
    assert_eq!(strike.pass(), Some(Pass::StandOff));
    // Masked: down toward 200 ft over the ground.
    assert!(steer(&order).pitch_deg < 0.);
    // At 60% of the AT-2's 18,000 ft zone it unmasks, climbing, at speed.
    world.own.position = [0., 300., TARGET_AT[2] - 10_500.];
    world.own.agl_ft = 300.;
    let order = strike.step(&world.context(2));
    assert_eq!(strike.segment(), Segment::Pull);
    let unmask = steer(&order);
    assert!(unmask.pitch_deg > 0.);
    assert_eq!(unmask.speed, strike_speed(&world));
    // It fires there and holds the line.
    world.inputs.solution = Some(solution(TARGET_AT, TARGET_AT, true));
    assert!(strike.step(&world.context(3)).release.is_some());
    assert_eq!(strike.segment(), Segment::Guide);
    // Unmasked too long with no shot: it masks again, turning away.
    let mut world = heli_world(at2(4));
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(
                SAM,
                [3_000., 0., TARGET_AT[2]],
                0x1000,
                site(5_000., 0., true),
            ),
        ],
        vec![SAM],
    );
    let mut strike = Strike::new(3);
    strike.step(&world.context(1));
    world.own.position = [0., 300., TARGET_AT[2] - 10_500.];
    strike.step(&world.context(2));
    strike.step(&world.context(2 + UNMASK_TICKS));
    assert_eq!(strike.phase(), Phase::Exit);
    let order = strike.step(&world.context(3 + UNMASK_TICKS));
    assert!(
        (steer(&order).heading_deg - 180.).abs() < 1.,
        "away from the target"
    );
}

#[test]
fn under_threat_it_holds_course_at_speed_and_settles_to_the_masking_height() {
    // The controller flies a defence (a missile), then it ends; for the ten
    // quiet seconds after it the controller's own air logic would fly corner
    // speed, which sinks a helicopter: the family flies the time (slice ST-H).
    let mut world = heli_world(at2(4));
    world.own.heading_deg = 20.;
    world.own.agl_ft = 1_200.;
    world.own.terrain_ahead_ft = 300.;
    world.own.position = [0., 1_500., 0.];
    let mut strike = Strike::new(3);
    // While the controller flies the defence, the family stands aside for it.
    let defending = Context {
        defending: true,
        warnings: crate::ai::awareness::ground::RecentWarnings {
            air_to_air: false,
            surface_to_air: true,
        },
        ..world.context(1)
    };
    let order = strike.step(&defending);
    assert_eq!(strike.phase(), Phase::Defend);
    assert!(order.steer.is_none());
    // The defence is over but the ten quiet seconds are not: it holds course at
    // full speed, down toward 200 ft over the highest ground ahead.
    let order = strike.step(&world.context(2));
    assert_eq!(strike.phase(), Phase::Defend);
    let hold = steer(&order);
    assert_eq!(hold.heading_deg, 20., "course held");
    assert_eq!(hold.speed, strike_speed(&world));
    assert!(
        hold.pitch_deg < 0. && hold.pitch_deg >= -10.,
        "{}",
        hold.pitch_deg
    );
    assert_eq!(order.activity, Some(Activity::Defending));
    // Already at that height it flies level.
    world.own.position[1] = 500.;
    assert_eq!(steer(&strike.step(&world.context(3))).pitch_deg, 0.);
}

#[test]
fn rockets_leave_in_a_burst_while_the_cue_stays_in_the_circle() {
    let mut world = heli_world(rockets(38));
    let mut strike = Strike::new(3);
    strike.step(&world.context(1));
    assert_eq!(strike.pass(), Some(Pass::PopUp));
    world.own.position = [0., 1_500., TARGET_AT[2] - 9_000.];
    strike.step(&world.context(2));
    assert_eq!(strike.segment(), Segment::Pull);
    let (offset, _) = strike.aim();
    let aim = [
        TARGET_AT[0] + offset[0],
        TARGET_AT[1],
        TARGET_AT[2] + offset[1],
    ];
    world.own.position = [0., 1_400., TARGET_AT[2] - 7_000.];
    let mut fired = 0;
    for tick in 3..400 {
        world.inputs.solution = Some(solution(TARGET_AT, aim, fired == 0));
        if strike.step(&world.context(tick)).release.is_some() {
            fired += 1;
        }
    }
    assert_eq!(fired, ROCKETS_PER_PASS);
}

#[test]
fn the_helicopter_is_deterministic_and_round_trips_through_a_checkpoint() {
    let mut world = heli_world(at2(4));
    let mut a = Strike::new(5);
    let mut b = Strike::new(5);
    for tick in 0..300 {
        world.own.position[2] += 2.;
        assert_eq!(a.step(&world.context(tick)), b.step(&world.context(tick)));
    }
    let models = Models::default();
    let coded = to_bytes(&a, &models).unwrap();
    let mut twin: Strike = from_bytes(&coded, &models).unwrap();
    assert_eq!(twin, a);
    for tick in 300..600 {
        world.own.position[2] += 2.;
        assert_eq!(
            a.step(&world.context(tick)),
            twin.step(&world.context(tick))
        );
    }
}

// Slice H4: John's threat rule (2026-10-12), docs/spec/ai-strike.md,
// "Helicopter threat awareness".

const SAM: u32 = 0x5000_0009;
const SAM_AT: Vector = [3_000., 0., 24_000.];

/// The target, and a SAM beside it that out-ranges the AT-2 (its ring
/// covers the target and every firing point), standing or destroyed.
fn sam_picture(standing: bool) -> crate::ai::awareness::ground::GroundPicture {
    let mut sam = ground(SAM, SAM_AT, 0x1000, site(30_000., 0., true));
    sam.object.destroyed = !standing;
    picture(&[ground(TARGET, TARGET_AT, 0x200, None), sam], vec![SAM])
}

fn defended_world() -> World {
    let mut world = heli_world(at2(4));
    world.picture = sam_picture(true);
    world
}

fn call(strike: &Strike) -> Option<crate::ai::helicopter_threat::Decision> {
    strike.threat_call().map(|c| c.decision)
}

#[test]
fn aware_of_a_ring_it_holds_off_outside_it_and_resumes_once_sead_kills_the_site() {
    use crate::ai::helicopter_threat::{Decision, HOLD_CALL_TICKS, HOLD_RADIUS_FT};
    let mut world = defended_world();
    let mut strike = Strike::new(3);
    let order = strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::HoldOff);
    assert_eq!(order.release, None);
    // Its circle lies outside the SAM's ring (30,000 ft plus the margin).
    let (centre, since) = strike.hold_off().expect("holding off");
    assert_eq!(since, 1);
    assert!(horizontal(SAM_AT, centre) >= 32_000. + HOLD_RADIUS_FT - 1.);
    let hold = steer(&order);
    assert_eq!(hold.speed, strike_speed(&world));
    assert!((hold.heading_deg - bearing_deg(world.own.position, centre)).abs() < 1e-9);
    // No call for a moment, then "holding off", naming the SAM.
    assert_eq!(call(&strike), None);
    strike.step(&world.context(1 + HOLD_CALL_TICKS));
    let held = *strike.threat_call().expect("called");
    assert_eq!(held.decision, Decision::HoldOff);
    assert_eq!((held.target, held.ring), (Some(TARGET), Some(SAM)));
    // SEAD kills the SAM: the ring is gone and it attacks again.
    world.picture = sam_picture(false);
    strike.step(&world.context(2_000));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(call(&strike), Some(Decision::Resume));
    assert!(strike.hold_off().is_none());
}

#[test]
fn after_three_minutes_with_nothing_outside_the_rings_it_goes_home() {
    use crate::ai::helicopter_threat::{Decision, HOLD_WAIT_TICKS};
    let world = defended_world();
    let mut strike = Strike::new(3);
    strike.step(&world.context(1));
    strike.step(&world.context(HOLD_WAIT_TICKS));
    assert_eq!(strike.phase(), Phase::HoldOff, "not quite three minutes");
    strike.step(&world.context(1 + HOLD_WAIT_TICKS));
    assert_eq!(strike.phase(), Phase::Home);
    assert_eq!(call(&strike), Some(Decision::Home));
}

#[test]
fn after_three_minutes_it_takes_a_target_outside_every_ring() {
    use crate::ai::awareness::ground::{GroundPicture, Subjects};
    use crate::ai::engagement::{StrikeObjective, StrikeRole};
    use crate::ai::helicopter_threat::{Decision, HOLD_WAIT_TICKS};
    const OTHER: u32 = 0x5000_0002;
    let other_at = [-40_000., 0., -20_000.];
    let subjects = [
        ground(TARGET, TARGET_AT, 0x200, None),
        ground(OTHER, other_at, 0x200, None),
        ground(SAM, SAM_AT, 0x1000, site(30_000., 0., true)),
    ];
    let mut picture = GroundPicture::new(StrikeRole::Strike);
    picture.set_objective(StrikeObjective {
        targets: vec![TARGET, OTHER],
        defenses: vec![SAM],
        point: Some(TARGET_AT),
    });
    picture.rebuild(0, Vec::new(), &Subjects::new(&subjects), Vec::new());
    let mut world = heli_world(at2(4));
    world.picture = picture;
    let mut strike = Strike::new(3);
    strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::HoldOff);
    assert_eq!(strike.target(), Some(TARGET), "the nearest first");
    strike.step(&world.context(1 + HOLD_WAIT_TICKS));
    assert_eq!(strike.target(), Some(OTHER));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(call(&strike), Some(Decision::Retarget));
}

#[test]
fn ambushed_by_a_threat_it_did_not_know_it_breaks_leaves_the_ring_then_resumes() {
    use crate::ai::awareness::ground::RecentWarnings;
    use crate::ai::helicopter_threat::{AMBUSH_QUIET_TICKS, AMBUSH_RADIUS_FT, Decision};
    use crate::ai::strike::WARNING_TICKS;
    let mut world = heli_world(at2(4));
    world.own.position = [0., 1_500., 12_000.];
    let mut strike = Strike::new(3);
    strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Attack);
    // A launch from the north: the break flies (the controller's), and the
    // place it was engaged from becomes a ring.
    let engaged = Context {
        defending: true,
        threat_bearing_deg: Some(0.),
        warnings: RecentWarnings {
            air_to_air: false,
            surface_to_air: true,
        },
        ..world.context(2)
    };
    let order = strike.step(&engaged);
    assert_eq!(strike.phase(), Phase::Defend);
    assert!(order.steer.is_none());
    assert_eq!(call(&strike), Some(Decision::Leave));
    // Quiet for ten seconds: it leaves the ring, south, away from the threat.
    let order = strike.step(&world.context(2 + WARNING_TICKS));
    assert_eq!(strike.phase(), Phase::HoldOff);
    let (centre, _) = strike.hold_off().expect("leaving");
    let heading = steer(&order).heading_deg;
    assert!((heading - 180.).abs() < 1., "{heading}");
    assert!(horizontal(centre, world.own.position) > AMBUSH_RADIUS_FT - 6_100.);
    // Out of the ring and a minute quiet: it may resume.
    world.own.position = centre;
    strike.step(&world.context(2 + AMBUSH_QUIET_TICKS - 1));
    assert_eq!(strike.phase(), Phase::HoldOff, "not yet a minute quiet");
    strike.step(&world.context(2 + AMBUSH_QUIET_TICKS));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(call(&strike), Some(Decision::Resume));
}

#[test]
fn a_shot_from_a_site_it_knew_is_no_ambush_and_one_from_a_site_it_did_not_is() {
    use crate::ai::awareness::ground::RecentWarnings;
    use crate::ai::helicopter_threat::{AMBUSH_ID, Decision};
    // A small site it knows 8,000 ft east, its ring (6,000 ft and the
    // margin) clear of the target and the way in.
    const KNOWN: u32 = 0x5000_0011;
    let mut world = heli_world(at2(4));
    world.own.position = [0., 1_500., 12_000.];
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(KNOWN, [8_000., 0., 12_000.], 0x1000, site(6_000., 0., true)),
        ],
        vec![KNOWN],
    );
    fn engaged(world: &World, tick: u64, bearing: f64) -> Context<'_> {
        Context {
            defending: true,
            threat_bearing_deg: Some(bearing),
            warnings: RecentWarnings {
                air_to_air: false,
                surface_to_air: true,
            },
            ..world.context(tick)
        }
    }
    // Engaged from the known site's bearing, just outside its ring: before
    // 0.2.2 any engagement outside every ring it knew was an ambush; now it
    // is that site's, and no ambush.
    let mut known = Strike::new(3);
    known.step(&world.context(1));
    assert_eq!(known.phase(), Phase::Attack);
    known.step(&engaged(&world, 2, 90.));
    assert_eq!(known.phase(), Phase::Defend);
    assert_ne!(call(&known), Some(Decision::Leave));
    assert!(known.heli.ambush.is_none());
    // A tick of the break without a bearing is no new evidence.
    let lost = Context {
        threat_bearing_deg: None,
        ..engaged(&world, 3, 90.)
    };
    known.step(&lost);
    assert!(known.heli.ambush.is_none());
    // Engaged from the west, where it knows no site: an ambush.
    let mut ambushed = Strike::new(3);
    ambushed.step(&world.context(1));
    ambushed.step(&engaged(&world, 2, 270.));
    assert_eq!(call(&ambushed), Some(Decision::Leave));
    let (ring, _) = ambushed.heli.ambush.expect("an ambush ring");
    assert_eq!(ring.id, AMBUSH_ID);
    assert!(ring.centre[0] < world.own.position[0]);
}

/// A hidden shoulder-fired team that its own launch gave away (T.O.R.E
/// opinionated, John 2026-10-29): known at once, so the plans keep out of
/// its ring, but the shot that gave it away is still from a site the
/// helicopter did not know, an ambush. Once it has been known for longer
/// than a round flies, a shot from it is a known site's.
#[test]
fn a_team_its_own_launch_gave_away_is_still_an_ambush_and_known_after() {
    use crate::ai::awareness::ground::{RecentWarnings, Subjects};
    use crate::ai::helicopter_threat::{Decision, UNMASK_SHOT_TICKS, known_rings};
    const TEAM: u32 = 0x5000_0012;
    let mut world = heli_world(at2(4));
    world.own.position = [0., 1_500., 12_000.];
    let mut team = ground(TEAM, [8_000., 0., 12_000.], 0x1000, site(6_000., 0., false));
    team.hidden = true;
    let subjects = [ground(TARGET, TARGET_AT, 0x200, None), team];
    world.picture = picture(&subjects, Vec::new());
    assert!(known_rings(&world.picture, crate::ai::targeting::Side(1)).is_empty());
    world.picture.unmask(1, &[TEAM], &Subjects::new(&subjects));
    assert!(
        known_rings(&world.picture, crate::ai::targeting::Side(1))
            .iter()
            .any(|r| r.id == TEAM)
    );
    fn engaged(world: &World, tick: u64) -> Context<'_> {
        Context {
            defending: true,
            threat_bearing_deg: Some(90.),
            warnings: RecentWarnings {
                air_to_air: false,
                surface_to_air: true,
            },
            ..world.context(tick)
        }
    }
    // The defence starts a second and a half after the launch.
    let mut first = Strike::new(3);
    first.step(&world.context(1));
    first.step(&engaged(&world, 180));
    assert_eq!(call(&first), Some(Decision::Leave));
    assert!(first.heli.ambush.is_some());
    // Long after, a shot from its bearing is the known team's.
    let late = 2 + UNMASK_SHOT_TICKS;
    let mut later = Strike::new(3);
    later.step(&world.context(late - 1));
    later.step(&engaged(&world, late));
    assert_ne!(call(&later), Some(Decision::Leave));
    assert!(later.heli.ambush.is_none());
}

#[test]
fn a_hold_off_is_deterministic_and_round_trips_through_a_checkpoint() {
    let mut world = defended_world();
    let mut a = Strike::new(5);
    for tick in 0..900 {
        world.own.position[2] -= 2.;
        a.step(&world.context(tick));
    }
    assert_eq!(a.phase(), Phase::HoldOff);
    let models = Models::default();
    let mut twin: Strike = from_bytes(&to_bytes(&a, &models).unwrap(), &models).unwrap();
    assert_eq!(twin, a);
    for tick in 900..1_500 {
        world.own.position[2] -= 2.;
        assert_eq!(
            a.step(&world.context(tick)),
            twin.step(&world.context(tick))
        );
    }
}

/// Slice H2 (the H4 follow-up): inside a ring it knows, after a break it
/// leaves by the shortest way out, straight away from the site, instead of
/// holding its course deeper in; outside every ring it holds its course.
#[test]
fn inside_a_known_ring_after_a_break_it_leaves_by_the_shortest_way_out() {
    let mut world = defended_world();
    world.own.heading_deg = 20.;
    let world = world;
    let mut strike = Strike::new(3);
    let defending = Context {
        defending: true,
        warnings: crate::ai::awareness::ground::RecentWarnings {
            air_to_air: false,
            surface_to_air: true,
        },
        ..world.context(1)
    };
    strike.step(&defending);
    assert_eq!(strike.phase(), Phase::Defend);
    let out = steer(&strike.step(&world.context(2)));
    let away = bearing_deg(SAM_AT, world.own.position);
    assert!(
        (out.heading_deg - away).abs() < 1e-9,
        "{} vs {away}",
        out.heading_deg
    );
    // The same after a break outside every ring: course held.
    let mut clear = heli_world(at2(4));
    clear.picture = sam_picture(false);
    clear.own.heading_deg = 20.;
    let mut strike = Strike::new(3);
    strike.step(&Context {
        defending: true,
        warnings: crate::ai::awareness::ground::RecentWarnings {
            air_to_air: false,
            surface_to_air: true,
        },
        ..clear.context(1)
    });
    assert_eq!(steer(&strike.step(&clear.context(2))).heading_deg, 20.);
}

/// Every shot hits, at 700 ft/s.
struct AllHit;

impl crate::ai::strike::ShotTable for AllHit {
    fn shot(&self, _: StationId, _: crate::ai::strike::ShotLaunch, range_ft: f64) -> Option<f64> {
        Some(range_ft / 700.)
    }
}

#[test]
fn under_heavy_aaa_on_the_target_it_fires_from_farther_out_with_the_hosts_answers() {
    // A gun 5,000 ft of reach on the target (a 7,000 ft ring), the Mi-24's
    // strike speed 120 kt: from the usual ranges the AT-2's flight carries
    // it into the ring, and before 2026-10-29 it held off. With the host's
    // answers it looks farther out (John, 2026-10-29) and fires from
    // 13,500 ft, the nearest range whose advance ends outside the ring.
    let mut store = at2(4);
    store.flight = Some(crate::ai::strike::FlightTimes {
        points: [
            (4_500., 7.7),
            (9_000., 14.6),
            (13_500., 24.7),
            (18_000., 34.8),
        ],
    });
    let mut world = heli_world(store);
    world.own.limits.maximum = crate::ai::ScalarSpeed(253.);
    world.own.limits.minimum = crate::ai::ScalarSpeed(60.);
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(SAM, TARGET_AT, 0x800, site(0., 5_000., true)),
        ],
        vec![SAM],
    );
    let mut blind = Strike::new(3);
    blind.step(&world.context(1));
    assert_eq!(blind.phase(), Phase::HoldOff, "no answers: as before");
    world.inputs.shots = crate::ai::strike::Shots(Some(std::sync::Arc::new(AllHit)));
    let mut strike = Strike::new(3);
    let order = strike.step(&world.context(1));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::StandOff));
    assert_eq!(strike.heli.standoff_ft, Some(13_500.));
    assert_eq!(order.release, None);
    // Its exit opens from that range.
    assert!(exit_distance(&strike, &world.context(2)) >= 13_500. + STANDOFF_EXIT_FT);
}

/// A tank driving east at 50 ft/s from `TARGET_AT` with a gun of 7,500 ft
/// reach standing 3,000 ft west of where it is now (T.O.R.E opinionated
/// change, John 2026-10-29: the move-ahead ambush).
fn moving_column_world() -> World {
    let mut store = at2(4);
    store.flight = Some(crate::ai::strike::FlightTimes {
        points: [
            (4_500., 8.2),
            (9_000., 16.4),
            (13_500., 24.5),
            (18_000., 32.7),
        ],
    });
    let mut world = heli_world(store);
    world.own.limits.maximum = crate::ai::ScalarSpeed(253.);
    world.own.limits.minimum = crate::ai::ScalarSpeed(60.);
    world.picture = picture(
        &[
            ground(TARGET, TARGET_AT, 0x200, None),
            ground(SAM, [-3_000., 0., 24_000.], 0x800, site(0., 7_500., true)),
        ],
        vec![SAM],
    );
    world.inputs.shots = crate::ai::strike::Shots(Some(std::sync::Arc::new(AllHit)));
    let mut moving = solution(TARGET_AT, TARGET_AT, false);
    moving.target_velocity = [50., 0., 0.];
    world.inputs.solution = Some(moving);
    world
}

#[test]
fn a_moving_column_under_a_gun_is_ambushed_from_ahead_of_it() {
    let world = moving_column_world();
    let mut strike = Strike::new(3);
    // The choice on a search tick (once a second) takes the move-ahead.
    strike.step(&world.context(120));
    assert_eq!(strike.phase(), Phase::Attack);
    assert_eq!(strike.pass(), Some(Pass::StandOff));
    let plan = *strike.move_ahead().expect("a move-ahead");
    assert_eq!(strike.heli.standoff_ft, Some(plan.range_ft));
    assert_eq!(strike.heli.approach, Some(plan.point));
    // Its aim lies east of the tank, where the tank will be.
    assert!(plan.aim[0] > TARGET_AT[0] + 1_000., "{plan:?}");
    // The firing point is outside the gun's ring.
    let gun = [-3_000., 0., 24_000.];
    assert!(horizontal(plan.point, gun) >= 7_500. + threat::RING_MARGIN_FT);
    // The same column standing still is attacked as before: no move-ahead.
    let mut parked = moving_column_world();
    if let Some(sol) = parked.inputs.solution.as_mut() {
        sol.target_velocity = [0.; 3];
    }
    let mut still = Strike::new(3);
    still.step(&parked.context(120));
    assert_eq!(still.move_ahead(), None);
}

#[test]
fn a_move_ahead_is_deterministic_and_round_trips_through_a_checkpoint() {
    let mut world = moving_column_world();
    let mut a = Strike::new(5);
    let mut b = Strike::new(5);
    let step = |world: &mut World, tick: u64| {
        world.own.position[2] += 1.5;
        if let Some(sol) = world.inputs.solution.as_mut() {
            sol.target_position[0] = TARGET_AT[0] + 50. * tick as f64 / 120.;
        }
    };
    for tick in 120..600 {
        step(&mut world, tick);
        assert_eq!(a.step(&world.context(tick)), b.step(&world.context(tick)));
    }
    assert!(a.move_ahead().is_some());
    let models = Models::default();
    let mut twin: Strike = from_bytes(&to_bytes(&a, &models).unwrap(), &models).unwrap();
    assert_eq!(twin, a);
    for tick in 600..900 {
        step(&mut world, tick);
        assert_eq!(
            a.step(&world.context(tick)),
            twin.step(&world.context(tick))
        );
    }
}
