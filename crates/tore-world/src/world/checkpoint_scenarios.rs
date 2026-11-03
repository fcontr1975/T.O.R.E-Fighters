//! The equivalence scenarios of docs/formats/checkpoint.md: missions built
//! from synthetic fixtures, each with the inputs and fixture adjustments of
//! every tick, the tick N to checkpoint at, the M ticks to step on, and what
//! the world must hold at tick N.
//!
//! Stage H0 built the first three (the single-player tick mission, the crowd
//! fixture's fight and an open mission with handoffs). Slice H8 added the
//! rest and the state each scenario asserts at tick N (`expect`), so the
//! whole-world test is known to exercise the state it is meant to: a
//! restore that drops a field nothing reads would otherwise pass. Every
//! scenario is built from synthetic fixtures; none reads retail data. The
//! lobby pass's slice R1 added the AI respawns, with one pending at N, and
//! slice R2 a lead held for a lost human, given back after N.

use super::{World, crowd, tick_tests};
use crate::{
    checkpoint::Section,
    comms::{Call, Hearer, Kind, Phrase},
    mission::{MissionSpec, Skill, Start},
    seats::{PlaneId, SeatCommand, SeatId, SeatInput},
    test_support::resources::{AIRPORT_AT, AIRPORT_RUNWAY, THEATER, airport_resources, resources},
    world::{AirportInput, MissionCommand, Seating, TickOutput},
};
use tore_formats::aircraft::AircraftId;
use tore_input::{PilotCommand, PilotInput, Switch};
use tore_sim::{
    ai::{airfield::Phase, wing::PlayerOrder},
    airport::Command as AirportCommand,
    combat::live::{self, Command},
};

/// One scenario. `drive` gets the world before step `step` (counted from the
/// build) and returns that step's mission commands and seat inputs; it may
/// adjust the world first, as the fixtures' own tests do. It must depend only
/// on the world and the step, so the original and the restored copy are
/// driven alike.
/// One step's mission commands and seat inputs.
pub(super) type Step = (Vec<MissionCommand>, Vec<SeatInput>);

pub(super) struct Scenario {
    pub name: &'static str,
    pub build: fn() -> World,
    pub drive: fn(&mut World, u64) -> Step,
    /// The step count at which the checkpoint is taken.
    pub at: u64,
    /// How many steps both copies then take.
    pub then: u64,
    /// Asserts what the world holds after `at` steps, which is what the
    /// scenario exists to exercise, and returns a note of it for `after`.
    pub expect: fn(&World) -> String,
    /// Asserts what the world holds after `at + then` steps, given the note
    /// `expect` made.
    pub after: Option<fn(&World, &str)>,
}

impl Scenario {
    /// A world built and stepped `steps` times.
    pub fn flown(&self, steps: u64) -> World {
        let mut world = (self.build)();
        let mut out = TickOutput::default();
        for step in 0..steps {
            let (commands, inputs) = (self.drive)(&mut world, step);
            world
                .step_with(&commands, &inputs, &mut out, |_, _| Ok(()))
                .unwrap_or_else(|error| panic!("{}: step {step}: {error}", self.name));
        }
        world
    }
}

/// Every scenario, in a fixed order.
pub(super) fn all() -> Vec<Scenario> {
    vec![
        single_player(),
        crowd_fight(),
        open_handoffs(),
        damaged_aircraft(),
        crowd_handoffs(),
        missile_duel(),
        radio_pending(),
        ai_landing(),
        ground_start(),
        changing_weather(),
        revivals(),
        lead_order(),
        crew_ejection(),
        ai_respawns(),
        held_lead(),
    ]
}

/// The full-tick fingerprint mission: a low-flying player with turbulence,
/// the airport service and a tower conversation, two AI aircraft a side and
/// two drones placed on the way.
pub(super) fn single_player() -> Scenario {
    fn expect(world: &World) -> String {
        let cockpit = &world.cockpits[0];
        // Turbulence is in use: its event state moved off the default.
        assert_ne!(
            cockpit.turbulence,
            tore_sim::turbulence::Turbulence::default(),
            "no turbulence event began"
        );
        // The player picked the airport and the tower answered.
        assert_eq!(cockpit.airport_service.selected(), Some(7));
        assert!(
            cockpit.airport_service.last_reply().is_some(),
            "the tower has not answered"
        );
        // The drone placed before the first burst was hit, and the player
        // flew on (the gear, the flaps and the throttle moved).
        assert!(cockpit.flight.speed > 100.);
        // The data link published its pictures and names every plane (the
        // drones are no members: they are not on the roster).
        assert_link_published(world, world.roster.planes().len());
        String::new()
    }
    Scenario {
        name: "single player",
        build: tick_tests::mission,
        drive: single_player_drive,
        at: 600,
        then: 600,
        expect,
        after: None,
    }
}

/// The fingerprint mission's own script: the drones moved into the sights
/// before each burst, and the scripted stick, throttle and tower calls.
fn single_player_drive(world: &mut World, step: u64) -> Step {
    match step {
        195 => tick_tests::place_drone(world, tick_tests::DRONES[0]),
        695 => tick_tests::place_drone(world, tick_tests::DRONES[1]),
        _ => {}
    }
    let mut input = tick_tests::script(step as usize);
    input.tick = world.tick();
    (Vec::new(), vec![input])
}

/// The crowd fixture: four against four at 10,000 feet, two humans a side,
/// radars on, turning into each other with the guns in bursts while the AI
/// fights on its own.
pub(super) fn crowd_fight() -> Scenario {
    fn drive(world: &mut World, step: u64) -> Step {
        (
            Vec::new(),
            crowd::inputs(world, |seat| crowd_pilot(step, seat, false)),
        )
    }
    fn expect(world: &World) -> String {
        let state = &world.combat.state;
        // The humans' guns have rounds in the air.
        let rounds = gun_rounds(state);
        assert!(rounds > 0, "no gun round in flight");
        // Every ownship holds radar contacts.
        assert!(
            state
                .ownships()
                .iter()
                .all(|own| !own.sensors.contacts().is_empty()),
            "an ownship sees nothing"
        );
        // Both wings' pictures were published and hold radar tracks of the
        // other side.
        let tracks = assert_link_published(world, 8);
        assert!(
            world
                .datalink
                .pictures()
                .iter()
                .all(|picture| !picture.tracks.is_empty()),
            "a flight holds no track"
        );
        format!("{rounds} rounds, {tracks} tracks")
    }
    Scenario {
        name: "crowd fight",
        build: crowd::crowded_mission,
        drive,
        at: 600,
        then: 600,
        expect,
        after: None,
    }
}

/// What the data link section must hold at a checkpoint (stage H slice H10):
/// every flight's picture was published on the last publishing tick, and the
/// members are the roster's. Returns the number of tracks the pictures hold.
fn assert_link_published(world: &World, members: usize) -> usize {
    let link = &world.datalink;
    assert_eq!(link.members().len(), members, "members");
    assert_eq!(link.tick(), world.tick(), "the link saw the last step");
    let pictures = link.pictures();
    assert!(!pictures.is_empty(), "no picture was published");
    for picture in pictures {
        assert!(picture.tick > 0 && picture.tick % crate::datalink::PUBLISH_TICKS == 0);
        assert!(world.tick() - picture.tick <= crate::datalink::PUBLISH_TICKS);
        assert!(!picture.status.is_empty() || picture.tracks.is_empty());
    }
    pictures.iter().map(|picture| picture.tracks.len()).sum()
}

/// The gun rounds in flight: an AI round carries its weapon, a human's is the
/// weapon of the station that fired it.
fn gun_rounds(state: &live::State) -> usize {
    state
        .projectiles
        .iter()
        .filter(|p| {
            let station = state
                .ownship(p.owner)
                .and_then(|own| own.configuration().stations.get(p.station))
                .map(|station| &station.weapon);
            p.weapon.as_ref().or(station).is_some_and(live::is_gun)
        })
        .count()
}

/// What one of the crowd fixture's four humans does at `step`: the radar on,
/// a radar contact designated by each leader if `designate` (the plain fight
/// leaves it out, as H0 built it), turns toward each other and the guns in
/// bursts.
/// (The fixture's second station holds no seeker missile, so no human fires a
/// guided one here: the missile duel does.)
fn crowd_pilot(step: u64, seat: SeatId, designate: bool) -> SeatInput {
    let side = if seat.0 < 2 { 1. } else { -1. };
    let mut pilot = PilotInput::default();
    let mut commands = Vec::new();
    if step == 10 {
        pilot.commands.push(PilotCommand::Set(Switch::Radar, true));
    }
    if designate && step == 60 && seat.0.is_multiple_of(2) {
        commands.push(SeatCommand::Combat(Command::Designate));
    }
    match step {
        120..180 => pilot.roll = 0.4 * side,
        180..300 => pilot.pitch = 0.3,
        300..360 => pilot.roll = -0.4 * side,
        800..860 => pilot.pitch = -0.2,
        _ => {}
    }
    SeatInput {
        pilot,
        commands,
        trigger: (400..430).contains(&step) || (900..920).contains(&step),
        ..SeatInput::default()
    }
}

/// An open mission built by `World::new` from the synthetic import, three
/// against three: seats take planes, one gives its plane back and another
/// takes one later, so the checkpoint's cockpits, ownships and AI actors
/// differ from the fresh world's.
pub(super) fn open_handoffs() -> Scenario {
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 3;
        spec.wings[3].count = 3;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 5;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }
    fn drive(world: &mut World, step: u64) -> Step {
        let planes: Vec<PlaneId> = world.roster.planes().iter().map(|p| p.id).collect();
        let commands = match step {
            // Each seat's player is named before it is seated (the lobby
            // pass's follow-up F1), so the roster's callsigns are in the
            // checkpoint.
            60 => vec![
                MissionCommand::Callsign {
                    seat: SeatId(0),
                    callsign: "Viper".into(),
                },
                MissionCommand::Take {
                    seat: SeatId(0),
                    plane: planes[0],
                },
            ],
            61 => vec![
                MissionCommand::Callsign {
                    seat: SeatId(1),
                    callsign: "Hawk".into(),
                },
                MissionCommand::Take {
                    seat: SeatId(1),
                    plane: planes[planes.len() - 1],
                },
            ],
            500 => vec![MissionCommand::GiveBack { seat: SeatId(0) }],
            640 => vec![MissionCommand::Take {
                seat: SeatId(2),
                plane: planes[1],
            }],
            _ => Vec::new(),
        };
        let mut flying: Vec<SeatId> = world
            .roster
            .seats()
            .iter()
            .filter(|seat| seat.plane.is_some())
            .map(|seat| seat.id)
            .collect();
        for command in &commands {
            match *command {
                MissionCommand::Take { seat, .. }
                | MissionCommand::ReviveLost { seat, .. }
                | MissionCommand::ReplacePlane { seat, .. } => flying.push(seat),
                MissionCommand::GiveBack { seat } => flying.retain(|s| *s != seat),
                MissionCommand::Despawn { plane } => flying.retain(|s| {
                    world
                        .roster
                        .seat(*s)
                        .is_none_or(|entry| entry.plane != Some(plane))
                }),
                MissionCommand::Settings(_)
                | MissionCommand::Abandon { .. }
                | MissionCommand::Revive { .. }
                | MissionCommand::Respawn { .. }
                | MissionCommand::LeadHold { .. }
                | MissionCommand::LeadLeft { .. }
                | MissionCommand::Callsign { .. } => {}
            }
        }
        let tick = world.tick();
        let inputs = flying
            .into_iter()
            .map(|seat| SeatInput {
                seat,
                tick,
                trigger: (300..320).contains(&step),
                pilot: PilotInput {
                    pitch: 0.2,
                    roll: if step % 480 < 240 { 0.3 } else { -0.3 },
                    ..PilotInput::default()
                },
                ..SeatInput::default()
            })
            .collect();
        (commands, inputs)
    }
    fn expect(world: &World) -> String {
        let plane = |seat| world.roster.seat(SeatId(seat)).and_then(|s| s.plane);
        // The players' names: plane 0 keeps Viper's though Viper left it.
        assert_eq!(world.roster.plane_callsign(PlaneId(0)), Some("Viper"));
        assert_eq!(world.roster.seat_callsign(SeatId(1)), Some("Hawk"));
        // Seat 0 gave its plane back at step 500; seats 1 and 2 hold the
        // planes they took at steps 61 and 640.
        assert_eq!(plane(0), None);
        assert!(plane(1).is_some() && plane(2).is_some());
        assert_ne!(plane(1), plane(2));
        let flown: Vec<PlaneId> = world.cockpits.iter().map(|c| c.plane).collect();
        assert_eq!(flown.len(), 2, "{flown:?}");
        // The plane seat 0 left is the AI's again, with an actor.
        let back = world.roster.plane(PlaneId(0)).unwrap();
        assert_eq!(back.pilot, crate::seats::Pilot::Ai);
        assert!(
            world
                .ai_wings
                .as_ref()
                .unwrap()
                .mission()
                .actor(0)
                .is_some()
        );
        format!("{flown:?}")
    }
    Scenario {
        name: "open mission with handoffs",
        build,
        drive,
        at: 700,
        then: 600,
        expect,
        after: None,
    }
}

// ---------------------------------------------------------------------------
// Slice H8's scenarios.

/// The seats that fly after `commands` take effect: those flying now, plus the
/// seats the commands give a plane, less the seats they take one from.
fn flying_after(world: &World, commands: &[MissionCommand]) -> Vec<SeatId> {
    let mut flying: Vec<SeatId> = world
        .roster
        .seats()
        .iter()
        .filter(|seat| seat.plane.is_some())
        .map(|seat| seat.id)
        .collect();
    for command in commands {
        match *command {
            MissionCommand::Take { seat, .. }
            | MissionCommand::ReviveLost { seat, .. }
            | MissionCommand::ReplacePlane { seat, .. } => flying.push(seat),
            MissionCommand::GiveBack { seat } => flying.retain(|s| *s != seat),
            MissionCommand::Despawn { plane } => flying.retain(|s| {
                world
                    .roster
                    .seat(*s)
                    .is_none_or(|entry| entry.plane != Some(plane))
            }),
            MissionCommand::Settings(_)
            | MissionCommand::Abandon { .. }
            | MissionCommand::Revive { .. }
            | MissionCommand::Respawn { .. }
            | MissionCommand::LeadHold { .. }
            | MissionCommand::LeadLeft { .. }
            | MissionCommand::Callsign { .. } => {}
        }
    }
    flying.sort();
    flying.dedup();
    flying
}

/// An input for each seat of `flying`, as `script` makes it for that seat.
fn inputs_for(
    world: &World,
    flying: Vec<SeatId>,
    mut script: impl FnMut(SeatId) -> SeatInput,
) -> Vec<SeatInput> {
    let tick = world.tick();
    flying
        .into_iter()
        .map(|seat| SeatInput {
            seat,
            tick,
            ..script(seat)
        })
        .collect()
}

/// The crowd fixture's fight where damage lands: a human aircraft takes hits
/// that cost hit points and fault systems, one AI aircraft is shot down with
/// its pilot ejected and under canopy while its wreck falls, and another is
/// hurt. (The fixture's AI fires nothing and its guns hit nothing, so the
/// damage is dealt the ways the fixtures' own tests deal it: the development
/// damage command, and the target rows and ejection set by hand.)
pub(super) fn damaged_aircraft() -> Scenario {
    fn build() -> World {
        let mut world = crowd::crowded_mission();
        // Realistic damage, so a hit faults systems as well as taking hit points.
        world.combat.state.cheats.damage = tore_sim::cheats::Damage::Realistic;
        // The host's scoring is on and nobody drains the facts, so the
        // recorder holds the kill and the facts waiting at the checkpoint.
        world.set_scoring(true);
        world
    }
    fn drive(world: &mut World, step: u64) -> Step {
        match step {
            // Seat 3's aircraft is hit three times, below.
            // AI 7 is shot down: its row loses every hit point, its pilot
            // ejects as `eject` does, and its wreck starts to fall.
            840 => {
                let wings = world.ai_wings.as_mut().unwrap();
                let flight = wings.mission_mut().actor_mut(7).unwrap().flight_mut();
                flight.escape = Some(tore_sim::ejection::Escape::new(
                    flight.position,
                    flight.velocity,
                    tore_sim::attitude::Basis::new(flight.yaw, flight.pitch, flight.bank),
                ));
                flight.systems.pilot.ejected = true;
                flight.crashed = true;
                let row = world.combat.state.targets.iter_mut().find(|t| t.id == 7);
                row.unwrap().hp = 0;
            }
            // AI 6 is hurt: half its hit points and a fault.
            850 => {
                let row = world.combat.state.targets.iter_mut().find(|t| t.id == 6);
                let row = row.unwrap();
                row.hp = row.initial_hp / 2;
                row.faults.counts[8] = 1;
            }
            _ => {}
        }
        let inputs = crowd::inputs(world, |seat| {
            let mut input = crowd_pilot(step, seat, true);
            if seat.0 == 3 && (800..803).contains(&step) {
                input
                    .commands
                    .push(SeatCommand::Combat(Command::DamagePlayer));
            }
            input
        });
        (Vec::new(), inputs)
    }
    fn expect(world: &World) -> String {
        let state = &world.combat.state;
        // An ownship lost hit points and has a system fault.
        let full = state.ownships().iter().map(|o| o.hp).max().unwrap();
        let hurt = state.ownships().iter().find(|o| o.hp < full);
        let hurt = hurt.expect("no ownship lost hit points");
        let faults: u32 = hurt.subsystem_counts.iter().map(|c| u32::from(*c)).sum();
        assert!(faults > 0, "the ownship has no system fault");
        // A wreck is falling, and its pilot is under canopy.
        let wreck = state.targets.iter().find(|t| t.id == 7).unwrap();
        assert!(wreck.hp <= 0 && wreck.airborne && wreck.wreck.is_some());
        let escapees: Vec<u32> = world
            .ai_wings
            .as_ref()
            .unwrap()
            .escapees()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(escapees, [7]);
        // The other AI aircraft is hurt but flying.
        let hurt_ai = state.targets.iter().find(|t| t.id == 6).unwrap();
        assert!(
            hurt_ai.hp > 0 && hurt_ai.hp < hurt_ai.initial_hp,
            "{} of {}",
            hurt_ai.hp,
            hurt_ai.initial_hp
        );
        // The score recorder holds the lost aircraft as recorded, with its
        // kill waiting to be drained; the data link saw it die.
        let recorder = world.score.as_ref().expect("scoring is on");
        assert_eq!(recorder.recorded().collect::<Vec<_>>(), [7]);
        let facts = recorder.clone().take();
        assert!(
            facts.facts.iter().any(|fact| matches!(
                fact,
                crate::score::Fact::Kill { victim, .. } if victim.target == 7
            )),
            "the kill of 7 is not waiting"
        );
        assert!(!world.datalink.member(7).unwrap().alive);
        assert_link_published(world, 8);
        format!("{} faults, wreck {}", faults, wreck.id)
    }
    Scenario {
        name: "damaged aircraft",
        build,
        drive,
        at: 900,
        then: 600,
        expect,
        after: None,
    }
}

/// The crowd fixture's fight with handoffs after it began: seat 1 gives its
/// plane back to the AI and then takes another AI plane.
pub(super) fn crowd_handoffs() -> Scenario {
    fn drive(world: &mut World, step: u64) -> Step {
        let commands = match step {
            500 => vec![MissionCommand::GiveBack { seat: SeatId(1) }],
            560 => vec![MissionCommand::Take {
                seat: SeatId(1),
                plane: crowd::F_AI[0],
            }],
            _ => Vec::new(),
        };
        let flying = flying_after(world, &commands);
        let inputs = inputs_for(world, flying, |seat| crowd_pilot(step, seat, true));
        (commands, inputs)
    }
    fn expect(world: &World) -> String {
        let plane = |seat| world.roster.seat(SeatId(seat)).and_then(|s| s.plane);
        // Seat 1 flies the plane it took, not the one it gave back; the others
        // kept theirs.
        assert_eq!(plane(0), Some(crowd::F_LEAD));
        assert_eq!(plane(1), Some(crowd::F_AI[0]));
        assert_eq!(plane(2), Some(crowd::E_LEAD));
        assert_eq!(plane(3), Some(crowd::E_HUMAN));
        let back = world.roster.plane(crowd::F_HUMAN).unwrap();
        assert_eq!(back.pilot, crate::seats::Pilot::Ai);
        let taken = world.roster.plane(crowd::F_AI[0]).unwrap();
        assert_eq!(taken.pilot, crate::seats::Pilot::Human(SeatId(1)));
        assert_eq!(world.cockpits.len(), 4);
        // The AI flies the plane given back, and no longer the one taken.
        let mission = world.ai_wings.as_ref().unwrap().mission();
        assert!(mission.actor(crowd::F_HUMAN.0).is_some());
        assert!(mission.actor(crowd::F_AI[0].0).is_none());
        // The fight was already on when the handoffs came, and the leaders
        // still hold the radar contacts they designated: a lock.
        assert!(world.tick() > 560);
        let state = &world.combat.state;
        let locks = state.ownships().iter().filter(|o| o.designated().is_some());
        assert!(locks.count() >= 2, "no lock held");
        // The data link holds those locks and the engagements that follow
        // them, from the ticks they were first held.
        let link = &world.datalink;
        assert!(link.locks().len() >= 2, "the link holds no lock");
        assert!(link.locks().values().all(|lock| lock.since > 0));
        assert_eq!(link.engagements().len(), link.locks().len());
        assert_link_published(world, 8);
        String::new()
    }
    Scenario {
        name: "crowd fight with handoffs",
        build: crowd::crowded_mission,
        drive,
        at: 700,
        then: 600,
        expect,
        after: None,
    }
}

/// One against one over an open mission, nose to nose at 10,000 feet: seat 1
/// fires its guns, and seat 0 fires a guided missile when in range, so a
/// missile is in flight, a missile warning is raised on both ownships and gun
/// rounds are in the air at the checkpoint, and the two aircraft meet soon
/// after it. (The fixtures' AI fires nothing, so the humans fire.)
pub(super) fn missile_duel() -> Scenario {
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 1;
        spec.wings[3].count = 1;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 5;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }
    fn drive(world: &mut World, step: u64) -> Step {
        let commands = if step == 5 {
            vec![
                MissionCommand::Take {
                    seat: SeatId(0),
                    plane: PlaneId(0),
                },
                MissionCommand::Take {
                    seat: SeatId(1),
                    plane: PlaneId(1),
                },
            ]
        } else {
            Vec::new()
        };
        let range = match &world.cockpits[..] {
            [a, b] => {
                let d: Vec<f64> = (0..3)
                    .map(|i| a.flight.position[i] - b.flight.position[i])
                    .collect();
                d.iter().map(|v| v * v).sum::<f64>().sqrt()
            }
            _ => f64::INFINITY,
        };
        let flying = flying_after(world, &commands);
        let inputs = inputs_for(world, flying, |seat| {
            let mut input = SeatInput::default();
            if step == 20 {
                input
                    .pilot
                    .commands
                    .push(PilotCommand::Set(Switch::Radar, true));
            }
            if seat.0 == 0 {
                // The missile station, and the trigger once it is in range.
                if step == 90 {
                    input
                        .commands
                        .push(SeatCommand::CycleWeapon { forward: true });
                }
                input.trigger = (6_000. ..9_000.).contains(&range);
            } else {
                input.trigger = (1400..1430).contains(&step);
            }
            input
        });
        (commands, inputs)
    }
    fn expect(world: &World) -> String {
        let state = &world.combat.state;
        // A guided missile is in flight.
        let missiles = state.projectiles.iter().filter(|p| p.guidance.is_some());
        assert!(missiles.count() > 0, "no guided missile in flight");
        // A missile warning is raised: an ownship's threat service holds a
        // record of it.
        let warned = state
            .ownships()
            .iter()
            .filter(|own| own.missile_threats.records().count() > 0)
            .count();
        assert!(warned > 0, "no missile warning raised");
        // Gun rounds are in the air.
        let rounds = gun_rounds(state);
        assert!(rounds > 0, "no gun round in flight");
        // Both aircraft are still flying.
        assert!(state.ownships().iter().all(|own| own.hp > 0));
        format!("{warned} warned, {rounds} rounds")
    }
    Scenario {
        name: "missile duel",
        build,
        drive,
        at: 1_700,
        then: 1_100,
        expect,
        after: None,
    }
}

/// The tick mission with radio calls waiting in the player's channel at the
/// checkpoint (H7 found that no scenario had any): three calls with delays, a
/// shared cooldown and a seat's cooldown, put in just before it.
pub(super) fn radio_pending() -> Scenario {
    fn drive(world: &mut World, step: u64) -> Step {
        if step == 570 {
            let now = world.tick() as f64 / 120.;
            let hearers = [Hearer::seat(SeatId(0))];
            let line = |label: &str, text: &str, kind| {
                Call::new(label, Phrase::default().raw(text, Some("^CONTACT")), kind)
            };
            let comms = &mut world.comms;
            comms.send(
                now,
                line("Red two", "Radar contact", Kind::Chatter).after(5.),
                &hearers,
            );
            comms.send(
                now,
                line("Tower", "Cleared to land", Kind::Important).after(9.),
                &hearers,
            );
            comms.send(
                now,
                line("Red three", "Fox two", Kind::Important).after(14.),
                &hearers,
            );
            assert!(comms.cooldown("radio-gun", now, 30.));
            assert!(comms.seat_cooldown(SeatId(0), "crew-radar-warning", now, 30.));
        }
        single_player_drive(world, step)
    }
    fn expect(world: &World) -> String {
        let now = world.tick() as f64 / 120.;
        assert!(world.comms.cooling("radio-gun", now));
        assert!(
            world
                .comms
                .seat_remaining(SeatId(0), "crew-radar-warning", now)
                > 0.
        );
        // The channel holds calls the same mission without them does not.
        let plain = single_player().flown(world.tick());
        assert_ne!(
            world.checkpoint_sections(&[Section::Comms]).unwrap(),
            plain.checkpoint_sections(&[Section::Comms]).unwrap(),
            "no call is waiting in the channel"
        );
        String::new()
    }
    Scenario {
        name: "single player with radio calls pending",
        build: tick_tests::mission,
        drive,
        at: 600,
        then: 900,
        expect,
        after: None,
    }
}

/// A mission built from the synthetic import with an airport, airborne over
/// the field with the wing ordered to land there: one wingman rolls out on the
/// runway while the other holds, waiting its turn to approach. (The wingmen
/// start at the first gates of the approach, which the AI must still fly
/// through at its approach speed, so this is the longest scenario.)
pub(super) fn ai_landing() -> Scenario {
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 3;
        spec.wings[3].count = 1;
        spec.wings[3].skill = Skill::Average;
        // Keep the enemy distant at a separation that actually fits this map.
        spec.separation_nm = 150;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        let mut world = World::new(&spec, &airport_resources(), Seating::SinglePlayer).unwrap();
        // Wingman 1 starts 36,000 feet and wingman 2 52,000 feet short of
        // the field, heading for it at the approach speed.
        let wings = world.ai_wings.as_mut().unwrap();
        for (id, back) in [(1, 36_000.), (2, 52_000.)] {
            let flight = wings.mission_mut().actor_mut(id).unwrap().flight_mut();
            flight.position = [AIRPORT_AT, 2_500., AIRPORT_AT - back];
            flight.yaw = 0.;
            flight.speed = 340.;
            flight.velocity = [0., 0., flight.speed];
        }
        wings.mirror_pose_out(&mut world.combat.state.targets);
        world
    }
    fn drive(world: &mut World, step: u64) -> Step {
        let mut input = SeatInput {
            tick: world.tick(),
            ..SeatInput::default()
        };
        match step {
            10 => input
                .commands
                .push(SeatCommand::Airport(AirportInput::Command(
                    AirportCommand::SelectAirport(1),
                ))),
            20 => input
                .commands
                .push(SeatCommand::WingOrder(PlayerOrder::LandAtSelected)),
            _ => {}
        }
        (Vec::new(), vec![input])
    }
    fn expect(world: &World) -> String {
        let mission = world.ai_wings.as_ref().unwrap().mission();
        let phase = |id| mission.actor(id).unwrap().airfield_phase();
        // One wingman is rolling out on the runway; the other holds.
        assert_eq!(phase(1), Some(Phase::Rollout));
        assert!(
            matches!(phase(2), Some(Phase::Marshal | Phase::Approach)),
            "{:?}",
            phase(2)
        );
        let on_runway = {
            let at = mission.actor(1).unwrap().flight().position;
            world.terrain.airport_scene.runway_surface(at[0], at[2])
        };
        assert!(on_runway.is_some(), "wingman 1 is not on the runway");
        format!("{:?} {:?}", phase(1), phase(2))
    }
    fn after(world: &World, _: &str) {
        // The first wingman cleared the runway; the second began its approach.
        let mission = world.ai_wings.as_ref().unwrap().mission();
        let phase = |id| mission.actor(id).unwrap().airfield_phase();
        assert_eq!(phase(1), Some(Phase::TaxiClear));
        assert_eq!(phase(2), Some(Phase::Approach));
    }
    Scenario {
        name: "AI landing at an airport",
        build,
        drive,
        at: 12_500,
        then: 1_600,
        expect,
        after: Some(after),
    }
}

/// The synthetic import with an airport and a ground start: the player on the
/// takeoff spot, a three-aircraft wing behind it on the taxiway. The player
/// takes off at step 5; the wingmen wait their turn, then the first lines up.
pub(super) fn ground_start() -> Scenario {
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 4;
        spec.wings[3].count = 2;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 5;
        spec.start = Start::Ground {
            runway: AIRPORT_RUNWAY,
            altitude_ft: 10_000,
        };
        World::new(&spec, &airport_resources(), Seating::SinglePlayer).unwrap()
    }
    fn drive(world: &mut World, step: u64) -> Step {
        let mut input = SeatInput {
            tick: world.tick(),
            ..SeatInput::default()
        };
        if step == 5 {
            input.pilot.commands.push(PilotCommand::Throttle(1.));
            input
                .pilot
                .commands
                .push(PilotCommand::Set(Switch::Burner, true));
        }
        (Vec::new(), vec![input])
    }
    fn expect(world: &World) -> String {
        let mission = world.ai_wings.as_ref().unwrap().mission();
        let phases: Vec<Option<Phase>> = (1..=3)
            .map(|id| mission.actor(id).unwrap().airfield_phase())
            .collect();
        // Wingmen still parked, one moving up to line up for takeoff.
        assert!(phases.contains(&Some(Phase::Waiting)), "{phases:?}");
        assert!(
            phases
                .iter()
                .any(|p| matches!(p, Some(Phase::Taxi | Phase::LineUp))),
            "{phases:?}"
        );
        // The player took off on its roll: well past the takeoff spot, fast.
        let player = &world.cockpits[0].flight;
        assert!(player.speed > 100., "{}", player.speed);
        // The airborne enemy wing has no airfield sequence.
        assert_eq!(mission.actor(4).unwrap().airfield_phase(), None);
        format!("{phases:?}")
    }
    Scenario {
        name: "ground start at an airport",
        build,
        drive,
        at: 2_000,
        then: 1_400,
        expect,
        after: None,
    }
}

/// The tick mission with a weather configuration whose two layers both run
/// the fog callback, so the layers are reselected every second of mission
/// time, and whose first layer ends five seconds in, so the active layer
/// changes inside the run.
pub(super) fn changing_weather() -> Scenario {
    fn build() -> World {
        use tore_formats::weather::{Callback, Module, synthetic_module};
        let mut world = tick_tests::mission();
        let mut module = Module::parse(&synthetic_module(2)).unwrap();
        for layer in &mut module.layers {
            layer.callback = Callback::Fog;
        }
        // Launched at 00:59: the first layer ends at 00:59:05, the second
        // picks up the next second.
        module.layers[0].end_seconds = 3_545;
        module.layers[1].start_seconds = 3_546;
        let configuration =
            tore_sim::environment::Configuration::new(module, 0, 59, 0, None).unwrap();
        world.terrain.weather = tore_sim::environment::Environment::new(configuration);
        world
    }
    /// What the weather holds: the active layers' start and tint.
    fn held(world: &World) -> Vec<(i32, i32, Callback)> {
        let active = world.terrain.weather.active();
        active
            .iter()
            .map(|l| (l.start_seconds, l.tint_scalar, l.callback))
            .collect()
    }
    use tore_formats::weather::Callback;
    fn expect(world: &World) -> String {
        let layers = held(world);
        // The first layer is active, running the fog callback.
        assert_eq!(layers.len(), 1, "{layers:?}");
        assert_eq!(layers[0].0, 0);
        assert_eq!(layers[0].2, Callback::Fog);
        // Fog reselection has drawn from its stream: the tint left its start.
        assert_ne!(layers[0].1, 0);
        format!("{layers:?}")
    }
    fn after(world: &World, at_checkpoint: &str) {
        let layers = held(world);
        assert_eq!(layers.len(), 1, "{layers:?}");
        // The second layer took over, and its fog tint moved too.
        assert_eq!(layers[0].0, 3_546);
        assert_ne!(format!("{layers:?}"), at_checkpoint);
    }
    Scenario {
        name: "changing weather",
        build,
        drive: single_player_drive,
        at: 300,
        then: 1_200,
        expect,
        after: Some(after),
    }
}

// ---------------------------------------------------------------------------
// Slice F2-V's scenario.

/// An open mission, three against three, with stage F phase 2's revival: seat
/// 0's pilot is killed and it is revived in a new plane of its wing (step
/// 201); seat 1's plane crashes and is abandoned (301) and its wreck retired
/// (400, by hand: a real retirement waits 30 seconds of rest); seat 0's new
/// plane is lost and revived once more (501). The checkpoint's roster,
/// cockpits, combat rows, AI and revival book then hold planes a fresh build
/// lacks, one it has no longer, and two wrecks nobody flies.
pub(super) fn revivals() -> Scenario {
    use super::revive::RevivalWeapons;
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 3;
        spec.wings[3].count = 3;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 10;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }
    fn cockpit_of(world: &mut World, seat: u8) -> &mut super::Cockpit {
        let plane = world.roster.seat(SeatId(seat)).unwrap().plane.unwrap();
        world
            .cockpits
            .iter_mut()
            .find(|c| c.plane == plane)
            .unwrap()
    }
    fn revive(world: &World, seat: u8) -> MissionCommand {
        let side = tore_sim::ai::launch::Side::Friendly;
        let start = world.side_mean(side).unwrap();
        let spawn = world
            .revival_spawn(
                SeatId(seat),
                start,
                30_000.,
                None,
                RevivalWeapons::NoMissiles,
            )
            .unwrap();
        MissionCommand::Revive {
            seat: SeatId(seat),
            spawn: Box::new(spawn),
        }
    }
    fn drive(world: &mut World, step: u64) -> Step {
        match step {
            200 | 500 => cockpit_of(world, 0).flight.systems.pilot.dead = true,
            300 => cockpit_of(world, 1).flight.crashed = true,
            400 => world.retire_plane(PlaneId(5)).unwrap(),
            _ => {}
        }
        let commands = match step {
            60 => vec![
                MissionCommand::Take {
                    seat: SeatId(0),
                    plane: PlaneId(0),
                },
                MissionCommand::Take {
                    seat: SeatId(1),
                    plane: PlaneId(5),
                },
            ],
            201 | 501 => vec![revive(world, 0)],
            301 => vec![MissionCommand::Abandon { seat: SeatId(1) }],
            _ => Vec::new(),
        };
        let mut flying = flying_after(world, &commands);
        if commands
            .iter()
            .any(|c| matches!(c, MissionCommand::Abandon { .. }))
        {
            flying.retain(|seat| *seat != SeatId(1));
        }
        let inputs = inputs_for(world, flying, |_| SeatInput {
            pilot: PilotInput {
                pitch: 0.1,
                roll: if step % 480 < 240 { 0.2 } else { -0.2 },
                ..PilotInput::default()
            },
            ..SeatInput::default()
        });
        (commands, inputs)
    }
    fn expect(world: &World) -> String {
        use crate::seats::Pilot;
        let pilot = |plane| world.roster.plane(PlaneId(plane)).map(|p| p.pilot);
        // Planes 0 to 5 were built; 6 and 7 are revivals; 5 is retired.
        assert_eq!(pilot(0), Some(Pilot::Lost));
        assert_eq!(pilot(5), None);
        assert_eq!(pilot(6), Some(Pilot::Lost));
        assert_eq!(pilot(7), Some(Pilot::Human(SeatId(0))));
        assert_eq!(world.roster.seat(SeatId(1)).unwrap().plane, None);
        let book = &world.revival;
        let lost: Vec<u32> = book.lost().iter().map(|l| l.plane.0).collect();
        assert_eq!(lost, [0, 6]);
        assert_eq!(book.retired()[0].id, PlaneId(5));
        assert_eq!(book.added(), [PlaneId(6), PlaneId(7)]);
        let flown: Vec<u32> = world.cockpits.iter().map(|c| c.plane.0).collect();
        assert_eq!(flown, [0, 6, 7]);
        format!("{lost:?}")
    }
    Scenario {
        name: "revivals and wrecks",
        build,
        drive,
        at: 700,
        then: 600,
        expect,
        after: None,
    }
}

// ---------------------------------------------------------------------------
// Slice H9's scenario.

/// The data link's assignment tests' fight (`datalink_assign_tests`): the
/// friendly wing is a human lead (seat 0), a second human and two AI
/// wingmen, armed the way a real mission arms them. The lead turns its radar
/// on, designates an enemy AI aircraft and orders its wing to engage it, so
/// the link holds assignments at the checkpoint, one of them acknowledged by
/// the wingman that locked the target (H10 found no other scenario holds
/// any).
pub(super) fn lead_order() -> Scenario {
    fn build() -> World {
        let mut world = crowd::ai_mission();
        let wings = world.ai_wings.as_mut().unwrap();
        for id in 1..=7 {
            let stations = wings.mission_mut().actor_mut(id).unwrap().stations_mut();
            stations[0].guided = false;
            stations[0].capability = tore_sim::ai::weapon_service::StoreCapability::GUN;
            stations[1].guided = true;
            stations[1].capability =
                tore_sim::ai::weapon_service::StoreCapability::AIR_TO_AIR_MISSILE;
        }
        world.take_plane(SeatId(1), crowd::F_HUMAN).unwrap();
        world
    }
    fn drive(world: &mut World, step: u64) -> Step {
        // The target is shot down 10 steps after the checkpoint. (It used to
        // fall to a collision with the idle human-flown plane 1 at step 1077;
        // the AI's traffic avoidance now steers clear of it, B4b, and nothing
        // else in this fight fires.)
        if step == 1_010 {
            let row = world
                .combat
                .state
                .targets
                .iter_mut()
                .find(|t| t.id == crowd::E_AI[0].0)
                .expect("an AI row");
            row.hp = 0;
        }
        let inputs = crowd::inputs(world, |seat| {
            let mut input = SeatInput::default();
            if seat == SeatId(0) {
                match step {
                    10 => input
                        .pilot
                        .commands
                        .push(PilotCommand::Set(Switch::Radar, true)),
                    40 => input
                        .commands
                        .push(SeatCommand::Combat(Command::DesignateTarget(
                            crowd::E_AI[0].0,
                        ))),
                    60 => input
                        .commands
                        .push(SeatCommand::WingOrder(PlayerOrder::EngageMyTarget)),
                    _ => {}
                }
            }
            input
        });
        (Vec::new(), inputs)
    }
    fn expect(world: &World) -> String {
        let held = world.datalink.assignments();
        // The lead's order assigned its target to the three wingmen at step
        // 60; one has locked it and acknowledged, the others not yet.
        assert_eq!(held.len(), 3, "{held:?}");
        assert!(
            held.values().all(|a| a.target == crowd::E_AI[0].0
                && a.by == crowd::F_LEAD.0
                && a.order == PlayerOrder::EngageMyTarget),
            "{held:?}"
        );
        assert!(held.values().any(|a| a.acknowledged), "{held:?}");
        assert!(held.values().any(|a| !a.acknowledged), "{held:?}");
        assert!(world.datalink.member(crowd::E_AI[0].0).unwrap().alive);
        assert_link_published(world, 8);
        format!("{held:?}")
    }
    fn after(world: &World, _: &str) {
        // The target was shot down after the checkpoint, which ended the
        // assignments.
        assert!(!world.datalink.member(crowd::E_AI[0].0).unwrap().alive);
        assert!(world.datalink.assignments().is_empty());
    }
    Scenario {
        name: "human lead's order",
        build,
        drive,
        at: 1_000,
        then: 600,
        expect,
        after: Some(after),
    }
}

/// The crowd fight in two-seaters (slice B6): an AI two-seater ejects both
/// its crew (`flight::State::eject`, as the AI's escape monitor calls it)
/// and a human in another presses the handle twice, so at the checkpoint
/// four chutes fall, two of them the second crew members', which the wire's
/// exact state leaves out and the checkpoint must keep. They fall on through
/// the restore.
pub(super) fn crew_ejection() -> Scenario {
    /// The AI two-seater that ejects, and the seat that ejects.
    const AI: u32 = 6;
    const SEAT: SeatId = SeatId(3);
    fn build() -> World {
        crowd::two_seat_crowded_mission()
    }
    fn drive(world: &mut World, step: u64) -> Step {
        if step == 700 {
            // Shot down, as the damaged aircraft's AI 7 is, but the crew
            // ejected by the flight's own rule.
            let wings = world.ai_wings.as_mut().unwrap();
            let flight = wings.mission_mut().actor_mut(AI).unwrap().flight_mut();
            assert!(flight.eject(), "the AI two-seater ejects");
            flight.crashed = true;
            let row = world.combat.state.targets.iter_mut().find(|t| t.id == AI);
            row.unwrap().hp = 0;
        }
        let inputs = crowd::inputs(world, |seat| {
            let mut input = crowd_pilot(step, seat, false);
            if seat == SEAT && (step == 740 || step == 780) {
                input.pilot.commands.push(PilotCommand::Eject);
            }
            input
        });
        (Vec::new(), inputs)
    }
    /// The four chutes, each as (owner, crew, ticks).
    fn chutes(world: &World) -> Vec<(u32, bool, u64)> {
        let wings = world.ai_wings.as_ref().unwrap();
        let mut chutes: Vec<(u32, bool, u64)> = wings
            .escapees()
            .map(|(id, e)| (id, false, e.ticks))
            .chain(wings.crew_escapees().map(|(id, e)| (id, true, e.ticks)))
            .collect();
        let flight = &cockpit(world).flight;
        for (crew, escape) in [(false, &flight.escape), (true, &flight.crew_escape)] {
            let escape = escape.as_ref().expect("the seat's crew ejected");
            chutes.push((crowd::E_HUMAN.0, crew, escape.ticks));
        }
        chutes.sort_unstable();
        chutes
    }
    fn cockpit(world: &World) -> &super::Cockpit {
        let at = world
            .cockpits
            .iter()
            .position(|c| c.plane == crowd::E_HUMAN);
        &world.cockpits[at.expect("the ejecting seat's cockpit")]
    }
    fn expect(world: &World) -> String {
        let chutes = chutes(world);
        let owners: Vec<(u32, bool)> = chutes.iter().map(|c| (c.0, c.1)).collect();
        assert_eq!(
            owners,
            [
                (crowd::E_HUMAN.0, false),
                (crowd::E_HUMAN.0, true),
                (AI, false),
                (AI, true)
            ]
        );
        // Every chute is still in the air: mid-descent from 10,000 feet.
        let wings = world.ai_wings.as_ref().unwrap();
        let airborne = |e: &tore_sim::ejection::Escape| {
            !matches!(
                e.phase,
                tore_sim::ejection::Phase::Landed | tore_sim::ejection::Phase::Impact
            ) && e.position[1] > 3_000.
        };
        assert!(wings.escapees().all(|(_, e)| airborne(e)));
        assert!(wings.crew_escapees().all(|(_, e)| airborne(e)));
        assert!(airborne(
            cockpit(world).flight.crew_escape.as_ref().unwrap()
        ));
        // The picture draws both second crew members.
        let crew = world
            .combat
            .snapshot(0, &world.cockpits[0].flight, world.ai_wings.as_ref())
            .pilots
            .iter()
            .filter(|p| p.crew)
            .count();
        assert_eq!(crew, 2, "two second crew members drawn");
        format!("{chutes:?}")
    }
    fn after(world: &World, at_checkpoint: &str) {
        // Every chute fell on 600 ticks from where it was.
        let then: Vec<(u32, bool, u64)> = chutes(world)
            .into_iter()
            .map(|(id, crew, ticks)| (id, crew, ticks - 600))
            .collect();
        assert_eq!(format!("{then:?}"), at_checkpoint);
    }
    Scenario {
        name: "two-seaters' crews ejected",
        build,
        drive,
        at: 900,
        then: 600,
        expect,
        after: Some(after),
    }
}

// ---------------------------------------------------------------------------
// The lobby pass's slice R1.

/// An open mission, three against three, with AI respawn: seat 0 flies plane
/// 0; the AI's friendly wingman (plane 1) is shot down (step 100) and
/// respawned (110) as plane 6, which is shot down in turn (200) and
/// respawned (210) as plane 7; the enemy's AI plane 4 is shot down (300) and
/// is still waiting for its respawn at the checkpoint (400), which comes
/// after it (450). The checkpoint's book then holds two lineage roots, the
/// AI wrecks waiting to retire and a lineage lost with its respawn pending;
/// the restored copy must make the same plane of it.
pub(super) fn ai_respawns() -> Scenario {
    use super::revive::RevivalWeapons;
    use tore_sim::ai::launch::Side;
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 3;
        spec.wings[3].count = 3;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 10;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }
    fn destroy(world: &mut World, plane: u32) {
        world
            .combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == plane)
            .unwrap()
            .hp = 0;
    }
    /// The respawn of `root`'s lineage at its side's mean place now (a
    /// stand-in for the original spawn a host records: it depends only on
    /// the world, so both copies are driven alike).
    fn respawn(world: &World, root: u32) -> MissionCommand {
        let root = PlaneId(root);
        let side = world.roster.plane(root).unwrap().slot.wing.side;
        let at = world.side_mean(side).unwrap();
        let heading = if side == Side::Friendly { 0. } else { 3.0 };
        let spawn = world
            .respawn_spawn(root, at, heading, &[], None, RevivalWeapons::NoMissiles)
            .unwrap();
        MissionCommand::Respawn {
            root,
            spawn: Box::new(spawn),
        }
    }
    fn drive(world: &mut World, step: u64) -> Step {
        match step {
            100 => destroy(world, 1),
            200 => destroy(world, 6),
            300 => destroy(world, 4),
            _ => {}
        }
        let commands = match step {
            60 => vec![MissionCommand::Take {
                seat: SeatId(0),
                plane: PlaneId(0),
            }],
            110 | 210 => vec![respawn(world, 1)],
            450 => vec![respawn(world, 4)],
            _ => Vec::new(),
        };
        let flying = flying_after(world, &commands);
        let inputs = inputs_for(world, flying, |_| SeatInput {
            pilot: PilotInput {
                pitch: 0.05,
                roll: if step % 480 < 240 { 0.2 } else { -0.2 },
                ..PilotInput::default()
            },
            ..SeatInput::default()
        });
        (commands, inputs)
    }
    fn expect(world: &World) -> String {
        use crate::seats::Pilot;
        let book = &world.revival;
        assert_eq!(book.added(), [PlaneId(6), PlaneId(7)]);
        assert_eq!(book.root_of(PlaneId(6)), PlaneId(1));
        assert_eq!(book.root_of(PlaneId(7)), PlaneId(1));
        assert_eq!(
            world.lineage(PlaneId(1)),
            [PlaneId(1), PlaneId(6), PlaneId(7)]
        );
        let waiting: Vec<u32> = book.lost().iter().map(|l| l.plane.0).collect();
        assert_eq!(waiting, [1, 6], "the replaced AI wrecks wait to retire");
        assert_eq!(world.roster.plane(PlaneId(7)).unwrap().pilot, Pilot::Ai);
        // The enemy's lineage is lost, its respawn pending.
        assert!(world.lineage_lost(PlaneId(4)));
        assert!(!world.lineage_lost(PlaneId(1)));
        format!("{waiting:?}")
    }
    fn after(world: &World, _: &str) {
        // The pending respawn made plane 8 on both copies.
        assert_eq!(world.lineage_head(PlaneId(4)), PlaneId(8));
        assert!(!world.lineage_lost(PlaneId(4)));
        let entry = world.roster.plane(PlaneId(8)).unwrap();
        assert_eq!((entry.slot.wing.side, entry.slot.member), (Side::Enemy, 1));
    }
    Scenario {
        name: "AI respawns, one pending",
        build,
        drive,
        at: 400,
        then: 300,
        expect,
        after: Some(after),
    }
}

// ---------------------------------------------------------------------------
// The lobby pass's slice R2.

/// An open mission, a flight of four and one of one, with the lead hold on
/// (step 0): seat 0 takes the flight's lead (60) and seat 1 its plane 2
/// (61); the AI orders nothing. Seat 0's pilot is killed (150), so seat 1
/// stands in for it while it waits; at the checkpoint (300) the lead is
/// held. Seat 0 revives (400) in plane 5, which takes the lead back on both
/// copies.
pub(super) fn held_lead() -> Scenario {
    use super::lead_hold::LeadOwner;
    use super::revive::RevivalWeapons;
    use crate::ai_wings::FRIENDLY_SIDE;
    use tore_sim::ai::launch::{Side, WingId};
    const WING: WingId = WingId {
        side: Side::Friendly,
        index: 0,
    };
    fn build() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 4;
        spec.wings[1].count = 1;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }
    fn drive(world: &mut World, step: u64) -> Step {
        if step == 150 {
            world
                .cockpits
                .iter_mut()
                .find(|c| c.plane == PlaneId(0))
                .unwrap()
                .flight
                .systems
                .pilot
                .dead = true;
        }
        let commands = match step {
            0 => vec![MissionCommand::LeadHold { on: true }],
            60 => vec![MissionCommand::Take {
                seat: SeatId(0),
                plane: PlaneId(0),
            }],
            61 => vec![MissionCommand::Take {
                seat: SeatId(1),
                plane: PlaneId(2),
            }],
            400 => {
                let start = world.side_mean(Side::Friendly).unwrap();
                let spawn = world
                    .revival_spawn(
                        SeatId(0),
                        start,
                        10. * tore_sim::sensors::FEET_PER_NAUTICAL_MILE,
                        None,
                        RevivalWeapons::Missiles,
                    )
                    .unwrap();
                vec![MissionCommand::Revive {
                    seat: SeatId(0),
                    spawn: Box::new(spawn),
                }]
            }
            _ => Vec::new(),
        };
        let flying = flying_after(world, &commands);
        let inputs = inputs_for(world, flying, |_| SeatInput {
            pilot: PilotInput {
                pitch: 0.05,
                roll: if step % 480 < 240 { 0.2 } else { -0.2 },
                ..PilotInput::default()
            },
            ..SeatInput::default()
        });
        (commands, inputs)
    }
    fn leader(world: &World) -> Option<u32> {
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .wing_leader(FRIENDLY_SIDE, 0)
    }
    fn expect(world: &World) -> String {
        assert!(world.lead_hold());
        assert_eq!(world.lead_owner(WING), Some(LeadOwner::Seat(SeatId(0))));
        // Seat 1's plane stands in for the lost owner.
        assert_eq!(leader(world), Some(2));
        assert!(world.lead_acting(WING));
        let claims = world.ai_wings.as_ref().unwrap().mission().lead_claims();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].plane, Some(0));
        format!("{:?}", world.lead_owners())
    }
    fn after(world: &World, _: &str) {
        assert_eq!(leader(world), Some(5));
        assert!(!world.lead_acting(WING));
        let owned = world.lead_owners()[0];
        assert_eq!(owned.owner, LeadOwner::Seat(SeatId(0)));
        assert_eq!((owned.plane, owned.led), (PlaneId(5), true));
    }
    Scenario {
        name: "a lead held for a lost human",
        build,
        drive,
        at: 300,
        then: 200,
        expect,
        after: Some(after),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scenario builds twice identically, reaches the state it asserts
    /// at its checkpoint tick, and (where it asserts one) the state after it.
    #[test]
    fn every_scenario_builds_twice_identically_and_reaches_its_asserted_state() {
        for scenario in all() {
            let name = scenario.name;
            let mut a = scenario.flown(scenario.at);
            let b = scenario.flown(scenario.at);
            assert_eq!(a.tick(), b.tick(), "{name}");
            // Identical wherever a section can be coded yet.
            for section in Section::ALL {
                match (
                    a.checkpoint_sections(&[section]),
                    b.checkpoint_sections(&[section]),
                ) {
                    (Ok(x), Ok(y)) => assert!(x == y, "{name}: {} differs", section.name()),
                    (Err(_), Err(_)) => {}
                    _ => panic!("{name}: {} codes in one build only", section.name()),
                }
            }
            let note = (scenario.expect)(&a);
            eprintln!("{name}: {note}");
            if let Some(after) = scenario.after {
                let mut out = TickOutput::default();
                for step in scenario.at..scenario.at + scenario.then {
                    let (commands, inputs) = (scenario.drive)(&mut a, step);
                    a.step_with(&commands, &inputs, &mut out, |_, _| Ok(()))
                        .unwrap();
                }
                after(&a, &note);
            }
        }
    }
}
