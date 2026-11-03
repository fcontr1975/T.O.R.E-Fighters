//! The handoff between the AI and a human (docs/ARCHITECTURE.md, "Handoff
//! between the AI and a human"), on synthetic fixtures: what is kept across
//! it, what it refuses, who leads and follows, and that the mission goes on
//! stepping and does so the same way twice.

use super::tick_tests::mission;
use super::*;
use crate::{ai_wings::FRIENDLY_SIDE, combat::fixtures, seats::Pilot};
use tore_sim::{
    ai::{
        launch::Side,
        threat::{DispenserStore, SeekerClass},
        weapon_service::Rounds,
    },
    combat::live,
};

/// Friendly Wing 2 is planes 1 (lead) and 2, the enemy's Wing 1 planes 3
/// (lead) and 4; plane 0 is seat 0's, the lead of Friendly Wing 1.
const WING_LEAD: PlaneId = PlaneId(1);
const WINGMAN: PlaneId = PlaneId(2);
const ENEMY_LEAD: PlaneId = PlaneId(3);
const ENEMY_WINGMAN: PlaneId = PlaneId(4);

/// The mission with its AI armed the way a built mission arms it (a store per
/// configuration station, flares and chaff), sides on the rows, and the
/// aircraft types a take needs.
fn armed_mission() -> World {
    let mut world = mission();
    fixtures::set_types(&mut world.combat, fixtures::types());
    let config = world.combat.own().configuration().clone();
    for (id, side) in [(1, 1), (2, 1), (3, 2), (4, 2)] {
        world.combat.state.set_side(id, live::Side(side));
    }
    // The fixture's enemy pair flies into rising ground at its own altitude,
    // so the whole AI flies high, clear of the ground, with the range the
    // fixture gave it.
    for target in &mut world.combat.state.targets {
        if (1..=4).contains(&target.id) {
            target.position[1] = 10000.;
        }
    }
    let wings = world.ai_wings.as_mut().unwrap();
    for id in 1..=4 {
        let actor = wings.mission_mut().actor_mut(id).unwrap();
        actor.flight_mut().position[1] = 10000.;
        actor.set_stations(ai_wings::station_specs(&config, false));
        actor.set_dispensers(vec![
            DispenserStore {
                class: SeekerClass::Infrared,
                count: 24,
            },
            DispenserStore {
                class: SeekerClass::Radar,
                count: 40,
            },
        ]);
    }
    world
}

/// One tick with an input for each of `seats`, `held` of them with the
/// trigger down.
fn tick(
    world: &mut World,
    mission: &[MissionCommand],
    seats: &[u8],
    held: bool,
    out: &mut TickOutput,
) -> WorldResult<()> {
    let tick = world.tick();
    let inputs: Vec<_> = seats
        .iter()
        .map(|&seat| SeatInput {
            seat: SeatId(seat),
            tick,
            trigger: held && seat != 0,
            ..SeatInput::default()
        })
        .collect();
    world.step_with(mission, &inputs, out, |_, _| Ok(()))
}

fn fly(world: &mut World, seats: &[u8], ticks: usize) {
    let mut out = TickOutput::default();
    for _ in 0..ticks {
        tick(world, &[], seats, false, &mut out).unwrap();
    }
}

fn cockpit(world: &World, plane: PlaneId) -> &Cockpit {
    world
        .cockpits
        .iter()
        .find(|c| c.plane == plane)
        .expect("a cockpit")
}

fn row(world: &World, plane: PlaneId) -> &live::Target {
    world
        .combat
        .state
        .targets
        .iter()
        .find(|t| t.id == plane.0)
        .expect("a combat row")
}

fn ai_stations(world: &World, plane: PlaneId) -> Vec<(Rounds, bool)> {
    world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(plane.0)
        .expect("an AI actor")
        .stations()
        .iter()
        .map(|s| (s.store.rounds, s.store.inhibited))
        .collect()
}

/// The invariants of a mission at any moment: one cockpit and one ownship for
/// each human-flown plane, in plane id order, an AI actor and a combat row
/// for every other one, and the seats agreeing with the planes.
fn assert_consistent(world: &World) {
    let humans: Vec<u32> = world
        .roster
        .planes()
        .iter()
        .filter(|p| matches!(p.pilot, Pilot::Human(_)))
        .map(|p| p.id.0)
        .collect();
    let cockpits: Vec<u32> = world.cockpits.iter().map(|c| c.plane.0).collect();
    let ownships: Vec<u32> = world
        .combat
        .state
        .ownships()
        .iter()
        .map(|o| o.aircraft)
        .collect();
    assert_eq!(cockpits, humans, "a cockpit for each human-flown plane");
    assert_eq!(ownships, humans, "an ownship for each human-flown plane");
    let wings = world.ai_wings.as_ref().unwrap();
    for plane in world.roster.planes() {
        let has_actor = wings.mission().actor(plane.id.0).is_some();
        let has_row = world
            .combat
            .state
            .targets
            .iter()
            .any(|t| t.id == plane.id.0);
        match plane.pilot {
            Pilot::Ai => assert!(has_actor && has_row, "AI plane {} whole", plane.id.0),
            Pilot::Human(seat) => {
                assert!(
                    !has_actor && !has_row,
                    "human plane {} only human",
                    plane.id.0
                );
                assert_eq!(world.roster.seat(seat).unwrap().plane, Some(plane.id));
            }
            Pilot::Lost => assert!(!has_actor && !has_row, "lost plane {}", plane.id.0),
        }
    }
    for seat in world.roster.seats() {
        if let Some(plane) = seat.plane {
            assert_eq!(world.roster.seat_of(plane), Some(seat.id));
        }
    }
    let ids: Vec<u32> = world.combat.state.targets.iter().map(|t| t.id).collect();
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "rows in id order {ids:?}"
    );
    let slots: Vec<u32> = wings.slots().iter().map(|s| s.id).collect();
    assert!(slots.windows(2).all(|w| w[0] < w[1]), "actors in id order");
    assert!(
        world.comms.seats().count() >= world.roster.seats().len(),
        "a radio channel for each seat"
    );
}

/// What the tick after a handoff must reproduce: the human planes' flights,
/// the ownships, the AI's aircraft and their rows, and what combat reported.
fn digest(world: &World, out: &TickOutput) -> String {
    let mut text = String::new();
    for c in &world.cockpits {
        let f = &c.flight;
        text += &format!(
            "cockpit {} {:?} {:?} {:?} {} {:?}\n",
            c.plane.0,
            f.position,
            f.velocity,
            [f.yaw, f.pitch, f.bank],
            f.fuel,
            f.damage_fraction
        );
    }
    for o in world.combat.state.ownships() {
        text += &format!(
            "ownship {} {} {:?} {} {} {:?}\n",
            o.aircraft, o.hp, o.ammo, o.chaff, o.flares, o.subsystem_counts
        );
    }
    for t in &world.combat.state.targets {
        text += &format!("row {} {:?} {:?} {}\n", t.id, t.position, t.velocity, t.hp);
    }
    for actor in world.ai_wings.as_ref().unwrap().mission().actors() {
        text += &format!(
            "actor {} {:?} {:?}\n",
            actor.id(),
            actor.flight().position,
            actor.flight().speed
        );
    }
    text += &format!("{:?}\n", out.events);
    text
}

/// Damage the AI's plane 1 the way a fight would: hit points, hit sections,
/// fault counts, some rounds spent, one station out of action.
fn hurt_plane_one(world: &mut World) {
    let target = world
        .combat
        .state
        .targets
        .iter_mut()
        .find(|t| t.id == 1)
        .unwrap();
    target.hp = 61;
    target.localized_damage.amounts = [3, 0, 40, 0, 7, 0];
    target.faults.counts[5] = 1;
    target.faults.counts[36] = 1;
    let wings = world.ai_wings.as_mut().unwrap();
    let actor = wings.mission_mut().actor_mut(1).unwrap();
    let stations = actor.stations_mut();
    stations[0].store.rounds = Rounds::Finite(7);
    stations[1].store.rounds = Rounds::Finite(3);
    stations[1].store.inhibited = true;
    actor.set_dispensers(vec![
        DispenserStore {
            class: SeekerClass::Infrared,
            count: 17,
        },
        DispenserStore {
            class: SeekerClass::Radar,
            count: 29,
        },
    ]);
}

fn feet(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// An aircraft that changes hands keeps its pose, speed, fuel and the rest of
/// its flight state exactly, and moves by one normal tick of flight in the
/// tick after.
#[test]
fn a_taken_plane_keeps_its_flight_stores_and_damage() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 60);
    hurt_plane_one(&mut world);
    let before = world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(1)
        .unwrap()
        .flight()
        .clone();
    let stations = ai_stations(&world, WING_LEAD);
    let hurt = row(&world, WING_LEAD).clone();

    world.take_plane(SeatId(1), WING_LEAD).unwrap();
    assert_consistent(&world);

    // The flight state: the whole of it, including fuel, systems, damage,
    // gear and flaps, and the flight model's internals and random state.
    let taken = cockpit(&world, WING_LEAD);
    assert_eq!(taken.flight, before);
    assert_eq!(taken.previous_flight, before);
    // The aircraft left the AI and its row became the ownship.
    let wings = world.ai_wings.as_ref().unwrap();
    assert!(wings.mission().actor(1).is_none() && wings.slot(1).is_none());
    assert!(world.combat.state.targets.iter().all(|t| t.id != 1));
    let own = world.combat.state.ownship(1).expect("an ownship");
    // Hit points keep their fraction to within one point.
    let capacity = own.configuration().damage_capacity;
    let expected = f64::from(hurt.hp) / f64::from(hurt.initial_hp) * f64::from(capacity);
    assert!(
        (f64::from(own.hp) - expected).abs() <= 1.,
        "{} of {capacity}, expected {expected}",
        own.hp
    );
    assert_eq!(own.subsystem_counts[5], 1);
    assert_eq!(own.subsystem_counts[36], 1);
    // Every station's rounds are the AI's, and the failed one stays failed.
    for (index, (rounds, failed)) in stations.iter().enumerate() {
        let Rounds::Finite(rounds) = rounds else {
            unreachable!()
        };
        assert_eq!(u32::from(own.rounds(index)), *rounds, "station {index}");
        assert_eq!(own.ammo[index] & 0x8000 != 0, *failed && *rounds > 0);
    }
    assert_eq!((own.flares, own.chaff), (17, 29));
    // A cockpit: the gun selected and armed, no autopilot, the seat's radio.
    assert!(own.armed);
    assert!(live::is_gun(
        &own.configuration().stations[own.selected].weapon
    ));
    assert_eq!(
        taken.flight.autopilot.mode(),
        tore_sim::autopilot::Mode::Off
    );
    assert!(!taken.airport_nav_mode);
    assert_eq!(world.roster.seat_of(WING_LEAD), Some(SeatId(1)));
    assert_eq!(world.roster.seat(SeatId(1)).unwrap().plane, Some(WING_LEAD));
    assert!(world.comms.seats().any(|seat| seat == SeatId(1)));
    let order: Vec<u32> = world.cockpits.iter().map(|c| c.plane.0).collect();
    assert_eq!(order, [0, 1]);

    // The next tick moves the aircraft by one normal tick of flight.
    let mut out = TickOutput::default();
    tick(&mut world, &[], &[0, 1], false, &mut out).unwrap();
    let after = &cockpit(&world, WING_LEAD).flight;
    let moved = feet(after.position, before.position);
    assert!(
        moved <= before.speed * flight::DT * 1.5 + 1.,
        "moved {moved} ft in a tick at {} ft/s",
        before.speed
    );
    assert!((after.speed - before.speed).abs() <= 5.);
    assert_consistent(&world);
}

/// Refitting the same number of stores while a human holds an AI plane
/// changes its picture when it goes back, even if its configuration reuses
/// the old map entry's address. The render cache must not keep the old shape.
#[test]
fn a_refitted_plane_given_back_draws_its_new_stores() {
    let mut world = armed_mission();
    for shape in ["SYNTH-A.SH", "SYNTH-B.SH"] {
        world.take_plane(SeatId(1), WING_LEAD).unwrap();
        let own = world.combat.state.ownship(1).unwrap();
        let mut config = own.configuration().clone();
        let side = own.side;
        config.stations[0].weapon.source = "SYNTH.JT".into();
        config.stations[0].weapon.shape = Some(shape.into());
        config.stations[0].count = 1;
        config.stations[0].rack.location = 4;
        config.hardpoint_slots = vec![Some(0)];
        world.combat.state.remove_ownship(1);
        world
            .combat
            .state
            .add_ownship(live::Ownship::new(1, side, config, true).unwrap())
            .unwrap();
        world.give_back_plane(SeatId(1)).unwrap();
        fly(&mut world, &[0], 1);
        let drawn = world
            .combat
            .render_snapshot()
            .targets
            .iter()
            .find(|p| p.id == 1)
            .unwrap();
        assert_eq!(drawn.stores[0].shape.as_deref(), Some(shape));
    }
}

/// A plane a human gives back keeps the same, and the AI flies it on.
#[test]
fn a_plane_given_back_keeps_its_flight_stores_and_damage() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 60);
    hurt_plane_one(&mut world);
    world.take_plane(SeatId(1), WING_LEAD).unwrap();
    let mut out = TickOutput::default();
    for _ in 0..90 {
        tick(&mut world, &[], &[0, 1], false, &mut out).unwrap();
    }
    // The human changed some things the AI must inherit: rounds and chaff.
    {
        let own = world.combat.state.ownship_mut(1).unwrap();
        own.ammo[0] = 5;
        own.chaff = 21;
        own.hp = 12;
    }
    let before = cockpit(&world, WING_LEAD).flight.clone();
    let own = world.combat.state.ownship(1).unwrap().clone();

    world.give_back_plane(SeatId(1)).unwrap();
    assert_consistent(&world);

    let wings = world.ai_wings.as_ref().unwrap();
    let actor = wings.mission().actor(1).expect("the AI flies it again");
    assert_eq!(actor.flight(), &before);
    assert!(world.cockpits.iter().all(|c| c.plane != WING_LEAD));
    assert!(world.combat.state.ownship(1).is_none());
    // Rounds, countermeasures and the failed station.
    let stations = ai_stations(&world, WING_LEAD);
    assert_eq!(stations[0].0, Rounds::Finite(5));
    assert_eq!(stations[1], (Rounds::Finite(3), true));
    let dispensers: Vec<_> = actor
        .dispensers()
        .iter()
        .map(|d| (d.class, d.count))
        .collect();
    assert_eq!(
        dispensers,
        [(SeekerClass::Infrared, 17), (SeekerClass::Radar, 21)]
    );
    // Hit points keep their fraction under the AI's rule.
    let back = row(&world, WING_LEAD);
    let expected = f64::from(own.hp) / f64::from(own.configuration().damage_capacity)
        * f64::from(back.initial_hp);
    assert!(
        (f64::from(back.hp) - expected).abs() <= 1.,
        "{} of {}, expected {expected}",
        back.hp,
        back.initial_hp
    );
    assert_eq!(back.faults.counts[5], 1);
    assert_eq!(back.position, before.position);
    // The seat waits; the plane is the AI's.
    assert_eq!(world.roster.seat(SeatId(1)).unwrap().plane, None);
    assert_eq!(world.roster.plane(WING_LEAD).unwrap().pilot, Pilot::Ai);
    // The AI's tick after moves it by one normal tick.
    tick(&mut world, &[], &[0], false, &mut out).unwrap();
    let after = world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(1)
        .unwrap()
        .flight();
    let moved = feet(after.position, before.position);
    assert!(
        moved <= before.speed * flight::DT * 1.5 + 1.,
        "moved {moved} ft in a tick at {} ft/s",
        before.speed
    );
    assert!((after.speed - before.speed).abs() <= 5.);
    assert_consistent(&world);
}

/// Everything the handoff refuses leaves the mission exactly as it was.
#[test]
fn a_refused_handoff_changes_nothing() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 10);
    let refuse = |world: &mut World, seat: u8, plane: u32| {
        let before = digest(world, &TickOutput::default());
        let planes = world.roster.planes().to_vec();
        assert!(world.can_take(SeatId(seat), PlaneId(plane)).is_err());
        assert!(world.take_plane(SeatId(seat), PlaneId(plane)).is_err());
        assert_eq!(digest(world, &TickOutput::default()), before);
        assert_eq!(world.roster.planes(), planes);
        assert_consistent(world);
    };
    // A plane a human flies, one that is not in the mission, and a seat
    // that already flies.
    refuse(&mut world, 1, 0);
    refuse(&mut world, 1, 99);
    refuse(&mut world, 0, 1);
    // Destroyed, crashed, or the pilot gone.
    world
        .combat
        .state
        .targets
        .iter_mut()
        .find(|t| t.id == 2)
        .unwrap()
        .hp = 0;
    refuse(&mut world, 1, 2);
    {
        let wings = world.ai_wings.as_mut().unwrap();
        wings
            .mission_mut()
            .actor_mut(3)
            .unwrap()
            .flight_mut()
            .crashed = true;
    }
    refuse(&mut world, 1, 3);
    {
        let wings = world.ai_wings.as_mut().unwrap();
        wings
            .mission_mut()
            .actor_mut(4)
            .unwrap()
            .flight_mut()
            .systems
            .pilot
            .ejected = true;
    }
    refuse(&mut world, 1, 4);
    // Giving back: an unknown seat, a seat with no plane, a dead plane.
    assert!(world.can_give_back(SeatId(7)).is_err());
    assert!(world.give_back_plane(SeatId(7)).is_err());
    world.take_plane(SeatId(1), WING_LEAD).unwrap();
    world.give_back_plane(SeatId(1)).unwrap();
    assert!(world.can_give_back(SeatId(1)).is_err());
    assert!(world.give_back_plane(SeatId(1)).is_err());
    world.take_plane(SeatId(1), WING_LEAD).unwrap();
    world.combat.state.ownship_mut(1).unwrap().hp = 0;
    let before = digest(&world, &TickOutput::default());
    assert!(world.give_back_plane(SeatId(1)).is_err());
    assert_eq!(digest(&world, &TickOutput::default()), before);
    assert_consistent(&world);
}

/// The command path: applied first in the tick, before any seat's commands,
/// and the seats' inputs are checked against the planes it leaves.
#[test]
fn handoff_commands_apply_first_in_the_tick() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 5);
    let mut out = TickOutput::default();
    let take = [MissionCommand::Take {
        seat: SeatId(1),
        plane: WING_LEAD,
    }];
    // The seat that takes a plane sends input for that tick; without it the
    // tick is refused.
    assert!(tick(&mut world, &take, &[0], false, &mut out).is_err());
    let tick_before = world.tick();
    tick(&mut world, &take, &[0, 1], false, &mut out).unwrap();
    assert_eq!(world.tick(), tick_before + 1);
    assert_eq!(world.roster.seat_of(WING_LEAD), Some(SeatId(1)));
    assert_consistent(&world);
    // A seat that gave its plane back sends none.
    let give = [MissionCommand::GiveBack { seat: SeatId(1) }];
    assert!(tick(&mut world, &give, &[0, 1], false, &mut out).is_err());
    tick(&mut world, &give, &[0], false, &mut out).unwrap();
    assert!(world.roster.seat_of(WING_LEAD).is_none());
    assert_consistent(&world);
    // A refused handoff is an error the host sees.
    assert!(tick(&mut world, &give, &[0], false, &mut out).is_err());
}

/// A wing leader a human takes leads its wing, and its AI wingman flies
/// formation on it; given back, the AI leads again.
#[test]
fn a_taken_wing_leader_leads_and_the_wingman_flies_on_it() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 10);
    let leader = |world: &World| {
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .wing_leader(FRIENDLY_SIDE, 1)
    };
    let formating = |world: &World, id: u32| {
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .actor(id)
            .unwrap()
            .controller()
            .formation_trace()
            .is_some()
    };
    assert_eq!(leader(&world), Some(1));
    world.take_plane(SeatId(1), WING_LEAD).unwrap();
    fly(&mut world, &[0, 1], 30);
    // A human leads the wing: no actor is its leader now, and the wingman
    // flies on it.
    assert_eq!(leader(&world), Some(1));
    assert!(
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .actor(1)
            .is_none()
    );
    assert!(formating(&world, 2), "the wingman has a leader to fly on");
    // The human flies straight on and the wingman stays with it.
    fly(&mut world, &[0, 1], 600);
    let lead = cockpit(&world, WING_LEAD).flight.position;
    let wingman = world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(2)
        .unwrap()
        .flight()
        .position;
    assert!(
        feet(lead, wingman) < 6000.,
        "the wingman is {} ft from its leader",
        feet(lead, wingman)
    );
    // Given back, the AI leads again.
    world.give_back_plane(SeatId(1)).unwrap();
    fly(&mut world, &[0], 30);
    assert_eq!(leader(&world), Some(1));
    let wings = world.ai_wings.as_ref().unwrap();
    assert!(wings.mission().actor(1).unwrap().identity().is_leader());
    assert!(formating(&world, 2));
}

/// The bug bash's start-up rule goes with a handoff: a taken plane whose gun
/// is empty starts on its first loaded station, or on NAV under Guns only,
/// with the NAV mode to match.
#[test]
fn a_handoff_keeps_the_start_up_rule() {
    for guns_only in [false, true] {
        let mut world = armed_mission();
        world.combat.state.cheats.guns_only = guns_only;
        let config = world.combat.own().configuration().clone();
        let gun = config
            .stations
            .iter()
            .position(|s| live::is_gun(&s.weapon))
            .expect("a gun station");
        let mut stations = ai_wings::station_specs(&config, false);
        stations[gun].store.rounds = Rounds::Finite(0);
        let wings = world.ai_wings.as_mut().unwrap();
        wings
            .mission_mut()
            .actor_mut(WINGMAN.0)
            .unwrap()
            .set_stations(stations);
        fly(&mut world, &[0], 10);
        world.take_plane(SeatId(1), WINGMAN).unwrap();
        let own = world.combat.state.ownship(WINGMAN.0).unwrap();
        assert_eq!(own.rounds(gun), 0);
        if guns_only {
            assert!(!own.armed, "the gun is empty and nothing else is allowed");
        } else {
            assert!(own.armed && own.selected != gun && own.rounds(own.selected) > 0);
        }
        assert_eq!(cockpit(&world, WINGMAN).airport_nav_mode, !own.armed);
    }
}

/// A wingman a human gives back rejoins its leader.
#[test]
fn a_given_back_wingman_rejoins_its_leader() {
    let mut world = armed_mission();
    fly(&mut world, &[0], 10);
    world.take_plane(SeatId(1), WINGMAN).unwrap();
    fly(&mut world, &[0, 1], 60);
    world.give_back_plane(SeatId(1)).unwrap();
    assert_consistent(&world);
    fly(&mut world, &[0], 30);
    let wings = world.ai_wings.as_ref().unwrap();
    let wingman = wings.mission().actor(2).expect("the AI flies it");
    assert!(!wingman.identity().is_leader());
    assert_eq!(wingman.identity().member, 1);
    assert!(wingman.controller().formation_trace().is_some());
    assert_eq!(
        wings.mission().wing_leader(FRIENDLY_SIDE, 1),
        Some(1),
        "its leader still leads"
    );
    // The wing's skill came with it.
    assert_eq!(
        wingman.experience(),
        wings.mission().actor(1).unwrap().experience()
    );
}

/// Two seats in two wings take aircraft, fight, give them back and take
/// others; the mission keeps stepping and stays consistent all the way.
#[test]
fn two_seats_in_two_wings_hand_planes_through_a_fight() {
    let mut world = armed_mission();
    let mut out = TickOutput::default();
    let mut seats = vec![0u8];
    let mut fired = 0usize;
    let mut handoffs = 0usize;
    let apply = |world: &mut World, seats: &mut Vec<u8>, command: MissionCommand| match command {
        MissionCommand::Take { seat, plane } if world.can_take(seat, plane).is_ok() => {
            seats.push(seat.0);
            seats.sort_unstable();
            Some(command)
        }
        MissionCommand::GiveBack { seat } if world.can_give_back(seat).is_ok() => {
            seats.retain(|s| *s != seat.0);
            Some(command)
        }
        _ => None,
    };
    for step in 0..1500usize {
        let mut commands = Vec::new();
        let scheduled = match step {
            0 => Some(MissionCommand::Take {
                seat: SeatId(1),
                plane: WING_LEAD,
            }),
            1 => Some(MissionCommand::Take {
                seat: SeatId(2),
                plane: ENEMY_LEAD,
            }),
            400 => Some(MissionCommand::GiveBack { seat: SeatId(1) }),
            500 => Some(MissionCommand::Take {
                seat: SeatId(1),
                plane: WINGMAN,
            }),
            700 => Some(MissionCommand::GiveBack { seat: SeatId(2) }),
            800 => Some(MissionCommand::Take {
                seat: SeatId(2),
                plane: ENEMY_WINGMAN,
            }),
            1100 => Some(MissionCommand::GiveBack { seat: SeatId(1) }),
            1200 => Some(MissionCommand::GiveBack { seat: SeatId(2) }),
            _ => None,
        };
        if let Some(command) = scheduled
            && let Some(command) = apply(&mut world, &mut seats, command)
        {
            commands.push(command);
            handoffs += 1;
        }
        tick(&mut world, &commands, &seats, step % 240 < 120, &mut out).unwrap();
        fired += out
            .events
            .iter()
            .filter(|e| matches!(e, live::Event::Fired { aircraft, .. } if *aircraft != 0))
            .count();
        if step % 50 == 0 || !commands.is_empty() {
            assert_consistent(&world);
        }
    }
    assert_consistent(&world);
    assert!(fired > 0, "the humans and the AI fought");
    // Every handoff of the schedule went through, each way, on both sides.
    assert_eq!(handoffs, 8);
    assert_eq!(world.roster.seats().len(), 3);
    for side in [Side::Friendly, Side::Enemy] {
        assert!(
            world
                .roster
                .planes()
                .iter()
                .any(|p| p.slot.wing.side == side)
        );
    }
}

/// The tick after a handoff is deterministic: two worlds given the same
/// handoff step the same way, in each direction.
#[test]
fn the_tick_after_a_handoff_is_deterministic() {
    let run = || {
        let mut world = armed_mission();
        let mut out = TickOutput::default();
        let mut digests = Vec::new();
        for step in 0..240usize {
            let mut commands = Vec::new();
            let seats: &[u8] = match step {
                100..180 => &[0, 1],
                _ => &[0],
            };
            if step == 100 {
                hurt_plane_one(&mut world);
                commands.push(MissionCommand::Take {
                    seat: SeatId(1),
                    plane: WING_LEAD,
                });
            }
            if step == 180 {
                commands.push(MissionCommand::GiveBack { seat: SeatId(1) });
            }
            tick(&mut world, &commands, seats, step >= 100, &mut out).unwrap();
            if (99..104).contains(&step) || (179..184).contains(&step) {
                digests.push(digest(&world, &out));
            }
        }
        digests
    };
    let first = run();
    assert_eq!(first.len(), 10);
    assert_eq!(first, run());
}

/// Retail multiplies every loaded station's count by `projsInPod` at mission
/// start (`HARDPodHack`): an AI plane's or wing member's rocket pods hold the
/// same rockets as the player's, so the plane a human takes carries what the
/// loadout says, and the pod weighs the same whoever flies it.
#[test]
fn ai_rocket_pods_hold_their_rockets_like_the_players() {
    let world = armed_mission();
    let mut config = world.combat.own().configuration().clone();
    let pod = 0;
    config.stations[pod].internal = false;
    config.stations[pod].weapon.flags &= !0x80;
    config.stations[pod].weapon.weight = 190;
    config.stations[pod].weapon.burst.projectiles_in_pod = 19;
    config.stations[pod].count = 2;
    let ammo = config.ammunition(&[2; 8][..config.stations.len()]).unwrap();
    assert_eq!(ammo[pod], 38);
    let specs = ai_wings::station_specs(&config, false);
    assert_eq!(specs[pod].store.rounds, Rounds::Finite(38));
    assert!((specs[pod].external_round_lbs * 19. - 190.).abs() < 1e-9);
    // A part-loaded station counts the pods it carries.
    let loaded = ai_wings::station_specs_loaded(&config, &[1; 8][..config.stations.len()], false);
    assert_eq!(loaded[pod].store.rounds, Rounds::Finite(19));
    // An ordinary store is still one round a unit.
    for (index, station) in config.stations.iter().enumerate() {
        if index != pod && live::pod_rounds(&station.weapon) == 1 && !live::is_gun(&station.weapon)
        {
            assert_eq!(
                specs[index].store.rounds,
                Rounds::Finite(u32::from(station.count))
            );
        }
    }
}
