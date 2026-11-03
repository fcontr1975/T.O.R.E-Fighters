//! The coders of combat in the world (the combat section): the wrapper's triggers, poses, contrails and render history, restored in place around `live::State`
//! (docs/formats/checkpoint.md, stage H slice H3b).
//!
//! `Combat` mixes mission setup with state, so it restores in place over the
//! fresh world's. The coded state is `live::State` (slice H3a's coder), the
//! host's contrail clock and smoke, each human's trigger, the engine outlets
//! and last pose of every other human-flown aircraft, the AI-pose switch and
//! the render history (the last two snapshots, coded because the picture and
//! the devices of an aircraft whose AI stopped read them back), and the
//! surface units' state (refused when it belongs to another surface).
//!
//! Skipped, each with its class:
//!
//! - setup, fixed when the mission is built and the same in the fresh world:
//!   `contrail_offsets`, `range`, `clean_recording`, `open`, `initial_ammo`,
//!   `dummies`, `mission_spawns`, `mission_layout`, `dummy_types`,
//!   `dummy_configs`, `airport_objects`, `ground_sides`, `ground_looks`,
//!   `surface_targets`;
//! - local, one machine's: `tape` (the recorder's list, which the app owns),
//!   `last_launcher` (read only to write the tape), `terrain_tti` (the
//!   readout's kept fly-out times, cleared on a load) and `notes` (the
//!   mission recorder's command notes, which nothing in flight reads);
//! - rebuilt: `RenderHistory::places` is each snapshot's target index by id,
//!   built again from the coded snapshots, as `set_current` and `advance` do;
//!   `RenderHistory::spare`, `ai_stores` and `angles` are scratch the next
//!   snapshot is written over and from, and start empty.
//!
//! The wrapper's own fields are coded by [`Combat::save_wrapper`] and
//! [`Combat::restore_wrapper`], apart from `state`, so they are tested before
//! the state's coder merges.

use super::{Combat, FireInput, Pose, RenderHistory, Trigger};
use crate::snapshot::RenderSnapshot;
use tore_sim::checkpoint::{Checkpoint, CheckpointError, InPlace, Loader, Saver};
use tore_sim::combat::live;

tore_sim::checkpoint_struct!(FireInput { held, inhibited });

tore_sim::checkpoint_struct!(Trigger { input, controller });

tore_sim::checkpoint_struct!(Pose {
    position,
    attitude,
    velocity,
    devices,
    engine,
    wreck,
    crashed,
    escape,
    crew_escape,
});

/// The two snapshots, the current one against the previous one (each target
/// then against itself a tick earlier), the plane they are drawn for and how
/// many times the history has started over.
impl Checkpoint for RenderHistory {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let RenderHistory {
            plane,
            previous,
            current,
            // Rebuilt from the snapshots, as `set_current` and `advance`
            // build it.
            places: _,
            // The spare snapshot and the stores cache are only what a
            // snapshot is written over and from; neither is read back.
            spare: _,
            ai_stores: _,
            angles: _,
            restarts,
        } = self;
        plane.save(s, None)?;
        restarts.save(s, None)?;
        previous.save(s, None)?;
        current.save(s, previous.as_ref())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        let plane = Checkpoint::load(l, None)?;
        let restarts = Checkpoint::load(l, None)?;
        let previous: Option<RenderSnapshot> = Checkpoint::load(l, None)?;
        let current = RenderSnapshot::load(l, previous.as_ref())?;
        // `places[0]` indexes `previous` (empty without one, as after a
        // restart) and `places[1]` indexes `current`.
        let mut places: [Vec<(u32, u32)>; 2] = Default::default();
        if let Some(previous) = &previous {
            Self::place(&mut places[0], previous);
        }
        Self::place(&mut places[1], &current);
        Ok(RenderHistory {
            plane,
            previous,
            current,
            places,
            restarts,
            ..RenderHistory::default()
        })
    }
}

impl Combat {
    /// Codes everything the combat section holds but `live::State`. The
    /// destructuring names every field, so a field added without coding or
    /// skipping it fails to compile.
    pub(crate) fn save_wrapper(&self, s: &mut Saver) -> Result<(), CheckpointError> {
        let Combat {
            // The state is the caller's (slice H3a's coder).
            state: _,
            // Setup: fixed when the mission is built.
            contrail_offsets: _,
            contrail_sortie,
            contrails,
            contrail_puffs,
            triggers,
            ownship_contrails,
            poses,
            // Setup.
            range: _,
            ai_poses,
            // Setup.
            clean_recording: _,
            // Setup.
            open: _,
            initial_ammo: _,
            render,
            dummies: _,
            mission_spawns: _,
            mission_layout: _,
            dummy_types: _,
            dummy_configs: _,
            airport_objects: _,
            ground_sides: _,
            ground_looks: _,
            // Setup: rebuilt from the surface with the scene.
            parked: _,
            surface_targets: _,
            ground_teams: _,
            // Setup: rebuilt with the scene.
            weapon_boxes: _,
            surface,
            // Local: the recorder's list, owned by the app.
            tape: _,
            // Local: read only to write the tape.
            last_launcher: _,
            // Local: the readout's kept fly-out times.
            terrain_tti: _,
            // Local: the mission recorder's notes, which no tick reads.
            notes: _,
            // Setup: built from the world on the first step.
            ground_ceiling: _,
        } = self;
        contrail_sortie.save(s, None)?;
        contrail_puffs.save(s, None)?;
        contrails.save(s, None)?;
        triggers.save(s, None)?;
        ownship_contrails.save(s, None)?;
        poses.save(s, None)?;
        ai_poses.save(s, None)?;
        render.save(s, None)?;
        surface.save(s, None)
    }

    /// Reads what [`Self::save_wrapper`] wrote over this combat's fields.
    /// Everything is decoded before anything is replaced, so a refusal leaves
    /// the wrapper as it was.
    pub(crate) fn restore_wrapper(&mut self, l: &mut Loader<'_>) -> Result<(), CheckpointError> {
        let contrail_sortie = Checkpoint::load(l, None)?;
        let contrail_puffs = Checkpoint::load(l, None)?;
        let contrails = Checkpoint::load(l, None)?;
        let triggers = Checkpoint::load(l, None)?;
        let ownship_contrails = Checkpoint::load(l, None)?;
        let poses = Checkpoint::load(l, None)?;
        let ai_poses = Checkpoint::load(l, None)?;
        let render = Checkpoint::load(l, None)?;
        let surface: crate::surface::SurfaceState = Checkpoint::load(l, None)?;
        surface.check_against(&self.surface)?;
        let Combat {
            // The state is the caller's.
            state: _,
            // Setup, local or rebuilt: kept as the fresh world has it (see
            // the module's list).
            contrail_offsets: _,
            contrail_sortie: contrail_sortie_slot,
            contrails: contrails_slot,
            contrail_puffs: contrail_puffs_slot,
            triggers: triggers_slot,
            ownship_contrails: ownship_contrails_slot,
            poses: poses_slot,
            range: _,
            ai_poses: ai_poses_slot,
            clean_recording: _,
            open: _,
            initial_ammo: _,
            render: render_slot,
            dummies: _,
            mission_spawns: _,
            mission_layout: _,
            dummy_types: _,
            dummy_configs: _,
            airport_objects: _,
            ground_sides: _,
            ground_looks: _,
            parked: _,
            surface_targets: _,
            ground_teams: _,
            weapon_boxes: _,
            surface: surface_slot,
            tape: _,
            last_launcher: _,
            terrain_tti: terrain_tti_slot,
            notes: _,
            ground_ceiling: _,
        } = self;
        terrain_tti_slot.clear();
        *contrail_sortie_slot = contrail_sortie;
        *contrail_puffs_slot = contrail_puffs;
        *contrails_slot = contrails;
        *triggers_slot = triggers;
        *ownship_contrails_slot = ownship_contrails;
        *poses_slot = poses;
        *ai_poses_slot = ai_poses;
        *render_slot = render;
        *surface_slot = surface;
        Ok(())
    }
}

impl InPlace for Combat {
    fn save_in_place(&self, s: &mut Saver) -> Result<(), CheckpointError> {
        self.state.save(s, None)?;
        self.save_wrapper(s)
    }
    fn restore_in_place(&mut self, l: &mut Loader<'_>) -> Result<(), CheckpointError> {
        // The state is decoded as a new value; its holder keeps the setup.
        let state = live::State::load(l, None)?;
        self.restore_wrapper(l)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        combat::fixtures,
        mission::{MissionSpec, Skill, Start},
        seats::{PlaneId, SeatId, SeatInput},
        test_support::resources::{THEATER, resources},
        world::{MissionCommand, Seating, TickOutput, World},
    };
    use tore_formats::aircraft::AircraftId;
    use tore_input::PilotInput;
    use tore_sim::checkpoint::{Coded, Models};
    use tore_sim::combat::smoke::Kind;
    use tore_sim::{ejection::Escape, flight};

    fn wrapper(combat: &Combat) -> Coded {
        let mut s = Saver::with_models(Models::default());
        combat.save_wrapper(&mut s).unwrap();
        Coded {
            body: s.finish_section(),
            records: s.into_records(),
        }
    }

    fn restore(combat: &mut Combat, coded: &Coded) -> Result<(), CheckpointError> {
        let models = Models::default();
        let mut l = Loader::new(&coded.body, &coded.records, &models);
        combat.restore_wrapper(&mut l)?;
        l.finish()
    }

    /// Smoke with every kind of puff and moving contrail outlets.
    fn smoke_after(ticks: u64) -> tore_sim::combat::smoke::Smoke {
        let mut smoke = tore_sim::combat::smoke::Smoke::default();
        smoke.wind = [9., 0., -4.];
        for n in 0..ticks {
            let t = n as f64;
            smoke.step([
                ([t, 5000., 40. * t], Kind::Missile),
                ([0., 3000., 200. * t], Kind::Aircraft),
                ([1000., 520., 2000.], Kind::Burning),
            ]);
            smoke.contrails([(1, [0., 31_000., 250. * t]), (2, [20., 31_000., 250. * t])]);
        }
        smoke
    }

    /// A combat holding something in every coded field of the wrapper.
    fn lived_in() -> Combat {
        let mut combat = fixtures::combat(fixtures::types(), Vec::new());
        let flight = flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        let scene = fixtures::scene(combat.state.own().configuration());
        fixtures::load(&mut combat, &scene, true, &flight);
        combat.contrail_sortie = 77;
        combat.contrails = smoke_after(400);
        combat.trigger(0).input.space(true, false, false);
        combat.trigger(3).controller.space(true, false, false);
        combat.trigger(3).input.space(true, false, true);
        combat.trigger(5).input.space(true, false, false);
        combat.trigger(5).input.cancel();
        combat
            .ownship_contrails
            .insert(3, vec![[1., 2., 3.], [4., 5., -6.5]]);
        combat.ownship_contrails.insert(5, Vec::new());
        let mut pose = Pose::of(&flight);
        combat.poses.insert(3, Pose::of(&flight));
        pose.escape = Some(Escape::new(
            [10., 3000., 20.],
            [30., -4., 500.],
            tore_sim::attitude::Basis::new(0.3, 0.1, 0.2),
        ));
        pose.crashed = true;
        pose.wreck = Some(tore_sim::wreck::Phase::Falling);
        combat.poses.insert(5, pose);
        combat
    }

    fn blank() -> Combat {
        let mut combat = fixtures::combat(fixtures::types(), Vec::new());
        combat.ai_poses = false;
        combat.contrail_sortie = 5;
        combat
    }

    #[test]
    fn a_combat_with_everything_set_restores_its_wrapper_exactly() {
        let original = lived_in();
        assert!(!original.contrails.puffs.is_empty());
        assert!(original.render.previous.is_some());
        assert!(!original.render.places[0].is_empty() && !original.render.places[1].is_empty());
        let coded = wrapper(&original);
        println!(
            "the wrapper of a lived-in combat: {} bytes (the contrails {} puffs, {} targets in each snapshot)",
            coded.body.len(),
            original.contrails.puffs.len(),
            original.render.current.targets.len()
        );

        let mut fresh = blank();
        restore(&mut fresh, &coded).unwrap();
        assert_eq!(wrapper(&fresh), coded);
        assert_eq!(fresh.contrail_sortie, 77);
        assert!(fresh.ai_poses);
        assert_eq!(fresh.contrails, original.contrails);
        assert_eq!(fresh.render.plane, original.render.plane);
        assert_eq!(fresh.render.restarts, original.render.restarts);
        assert_eq!(fresh.render.previous, original.render.previous);
        assert_eq!(fresh.render.current, original.render.current);
        assert_eq!(fresh.render.places, original.render.places);
        assert_eq!(fresh.ownship_contrails, original.ownship_contrails);
        // The triggers: who holds, who is inhibited, keyboard and controller.
        let flags = |c: &Combat| -> Vec<(u32, [bool; 4])> {
            c.triggers
                .iter()
                .map(|(id, t)| {
                    (
                        *id,
                        [
                            t.input.held,
                            t.input.inhibited,
                            t.controller.held,
                            t.controller.inhibited,
                        ],
                    )
                })
                .collect()
        };
        assert_eq!(flags(&fresh), flags(&original));
        assert!(flags(&original).iter().any(|(_, f)| f[0]));
        assert!(flags(&original).iter().any(|(_, f)| f[1]));
        assert!(flags(&original).iter().any(|(_, f)| f[2]));
        assert_eq!(fresh.poses.len(), 2);
        assert_eq!(fresh.poses[&5].escape, original.poses[&5].escape);
        assert!(fresh.poses[&5].crashed);
        // The lookups the picture makes work on the rebuilt places.
        for id in 1..=3 {
            assert_eq!(
                fresh.current_target(id).map(|p| p.position),
                original.current_target(id).map(|p| p.position)
            );
            assert_eq!(
                fresh.previous_target(id).map(|p| p.position),
                original.previous_target(id).map(|p| p.position)
            );
        }
        assert!(fresh.current_target(1).is_some() && fresh.previous_target(1).is_some());
    }

    #[test]
    fn a_history_that_just_started_over_has_no_previous_snapshot() {
        let mut original = lived_in();
        original.render.restart();
        let current = original.render.current.clone();
        original.render.set_current(current);
        assert!(original.render.previous.is_none());
        let mut fresh = lived_in();
        restore(&mut fresh, &wrapper(&original)).unwrap();
        assert!(fresh.render.previous.is_none());
        assert!(fresh.render.places[0].is_empty());
        assert_eq!(fresh.render.restarts, original.render.restarts);
        assert!(fresh.previous_target(1).is_none());
        // And an empty history.
        let empty = blank();
        let mut fresh = lived_in();
        restore(&mut fresh, &wrapper(&empty)).unwrap();
        assert_eq!(wrapper(&fresh), wrapper(&empty));
        assert!(fresh.render_plane().is_none() && fresh.contrails.puffs.is_empty());
    }

    #[test]
    fn setup_and_local_fields_keep_the_fresh_worlds_values() {
        let original = lived_in();
        let mut fresh = blank();
        fresh.range = true;
        fresh.open = true;
        fresh.clean_recording = true;
        fresh.initial_ammo = Some(vec![7, 8]);
        fresh.contrail_offsets = vec![[0.5, 0., 0.]];
        fresh.start_tape();
        restore(&mut fresh, &wrapper(&original)).unwrap();
        assert!(fresh.range && fresh.open && fresh.clean_recording);
        assert_eq!(fresh.initial_ammo, Some(vec![7, 8]));
        assert_eq!(fresh.contrail_offsets, vec![[0.5, 0., 0.]]);
        assert!(fresh.recording_tape());
        assert_eq!(fresh.dummy_types().len(), original.dummy_types().len());
    }

    #[test]
    fn a_refused_restore_leaves_the_wrapper_as_it_was() {
        let coded = wrapper(&lived_in());
        let mut fresh = blank();
        let before = wrapper(&fresh);
        let cut = Coded {
            body: coded.body[..coded.body.len() / 2].to_vec(),
            records: coded.records.clone(),
        };
        assert!(restore(&mut fresh, &cut).is_err());
        assert_eq!(wrapper(&fresh), before);
    }

    #[test]
    fn damaged_wrapper_bytes_never_panic() {
        let coded = wrapper(&lived_in());
        let mut fresh = blank();
        for cut in 0..coded.body.len() {
            let _ = restore(
                &mut fresh,
                &Coded {
                    body: coded.body[..cut].to_vec(),
                    records: coded.records.clone(),
                },
            );
        }
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut body = coded.body.clone();
            let at = (seed as usize >> 8) % body.len();
            body[at] ^= 1 << (seed & 7);
            let _ = restore(
                &mut fresh,
                &Coded {
                    body,
                    records: coded.records.clone(),
                },
            );
        }
    }

    /// The whole section codes once the state's coder (slice H3a) has merged;
    /// until then the state reports itself not covered, and this says so.
    #[test]
    fn the_whole_section_round_trips_once_the_state_is_coded() {
        let original = lived_in();
        let models = Models::default();
        let mut s = Saver::with_models(models.clone());
        match original.save_in_place(&mut s) {
            Err(CheckpointError::NotCovered(what)) => {
                println!("skipped: {what} is not coded yet (slice H3a)");
            }
            Ok(()) => {
                let mut fresh = blank();
                fresh.state = live::State::open_mission();
                tore_sim::checkpoint::round_trip_in_place(&original, &mut fresh, &models).unwrap();
            }
            Err(error) => panic!("the combat section failed: {error}"),
        }
    }

    // -- On a whole world ---------------------------------------------------

    /// An open mission, three against three, two seats taking planes at
    /// steps 60 and 61 and firing their guns from 300 to 480.
    fn mission() -> World {
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

    fn drive(world: &World, step: u64) -> (Vec<MissionCommand>, Vec<SeatInput>) {
        let planes: Vec<PlaneId> = world.roster.planes().iter().map(|p| p.id).collect();
        let commands = match step {
            60 => vec![MissionCommand::Take {
                seat: SeatId(0),
                plane: planes[0],
            }],
            61 => vec![MissionCommand::Take {
                seat: SeatId(1),
                plane: planes[planes.len() - 1],
            }],
            _ => Vec::new(),
        };
        let tick = world.tick();
        let inputs = (0..2)
            .filter(|seat| step >= 60 + *seat)
            .map(|seat| SeatInput {
                seat: SeatId(seat as u8),
                tick,
                trigger: (300..480).contains(&step),
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

    fn step(world: &mut World, n: u64, out: &mut TickOutput) {
        let (commands, inputs) = drive(world, n);
        world
            .step_with(&commands, &inputs, out, |_, _| Ok(()))
            .unwrap();
    }

    /// What a tick changed outside the wrapper's own fields: the events,
    /// every plane's combat terms and the weapon sounds. A trigger or a
    /// pose the restore lost shows here as different rounds.
    fn effects(out: &TickOutput) -> String {
        format!("{:?}\n{:?}\n{:?}", out.events, out.terms, out.releases)
    }

    /// The twin restore of the wrapper alone: two worlds flown alike, the
    /// wrapper of one scrambled and restored from the other, both flown on.
    /// The state (`live::State`) is equal in both already, so any difference
    /// comes from the wrapper's coding.
    #[test]
    fn a_scrambled_wrapper_restored_from_its_twin_flies_on_identically() {
        const AT: u64 = 330;
        let (mut a, mut b) = (mission(), mission());
        let (mut out_a, mut out_b) = (TickOutput::default(), TickOutput::default());
        for n in 0..AT {
            step(&mut a, n, &mut out_a);
            step(&mut b, n, &mut out_b);
        }
        // The state the test exists to exercise: two humans holding their
        // triggers with rounds in flight, a picture of each, a history.
        assert!(a.combat.triggers.values().any(Trigger::held));
        assert!(!a.combat.state.projectiles.is_empty());
        assert!(a.combat.poses.len() == 2, "{}", a.combat.poses.len());
        assert!(a.combat.render.previous.is_some());
        assert!(!a.combat.render.current.targets.is_empty());
        // Smoke the quiet fight does not make, and a trigger the pilot must
        // let go of before it fires again: put both in the original only, so
        // the restore has to carry them (a lost one fires on in the twin).
        a.combat.contrails = smoke_after(900);
        let plane = a.cockpits[0].plane.0;
        let trigger = a.combat.trigger(plane);
        trigger.controller.held = false;
        trigger.controller.inhibited = true;
        assert!(wrapper(&b.combat).body.len() < wrapper(&a.combat).body.len());

        let coded = wrapper(&a.combat);
        println!(
            "the wrapper of the open mission at tick {AT}: {} bytes",
            coded.body.len()
        );
        // Scramble every coded field of the twin.
        let c = &mut b.combat;
        c.contrails = Default::default();
        c.triggers.clear();
        c.poses.clear();
        c.ownship_contrails.clear();
        c.render = RenderHistory::default();
        c.ai_poses = !c.ai_poses;
        c.contrail_sortie += 9;
        assert!(wrapper(&b.combat) != coded);
        restore(&mut b.combat, &coded).unwrap();
        assert!(wrapper(&b.combat) == coded);

        for n in AT..AT + 400 {
            step(&mut a, n, &mut out_a);
            step(&mut b, n, &mut out_b);
            assert_eq!(effects(&out_a), effects(&out_b), "tick {n}");
            assert_eq!(a.combat.contrails, b.combat.contrails, "tick {n}");
            assert_eq!(
                a.combat.render.current, b.combat.render.current,
                "the picture at tick {n}"
            );
            assert_eq!(a.combat.render.previous, b.combat.render.previous);
            if (n + 1 - AT).is_multiple_of(20) {
                assert!(
                    wrapper(&a.combat) == wrapper(&b.combat),
                    "the coded wrapper at tick {n}"
                );
            }
        }
        // The guns kept firing while the trigger was held and stopped when
        // it was let go, in both.
        assert!(a.combat.triggers.values().all(|t| !t.held()));
    }

    /// The open mission at 40,000 feet, above every aircraft's contrail
    /// height.
    fn high_mission() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 3;
        spec.wings[3].count = 3;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 5;
        spec.start = Start::Airborne {
            altitude_ft: 40_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }

    /// Engine outlets for the human-flown planes, which the synthetic
    /// aircraft have none of, so their type leaves contrails.
    fn give_outlets(world: &mut World) {
        let planes: Vec<u32> = world
            .combat
            .state
            .ownships()
            .iter()
            .map(|own| own.aircraft)
            .collect();
        for plane in planes {
            world
                .combat
                .ownship_contrails
                .insert(plane, vec![[4., 0., -25.], [-4., 0., -25.]]);
        }
    }

    /// `world`'s checkpoint as it would be with no contrail puffs: the
    /// switch off and the puffs gone, put back afterwards.
    fn checkpoint_without_puffs(world: &mut World) -> Vec<u8> {
        let puffs = std::mem::take(&mut world.combat.contrails);
        let on = std::mem::replace(&mut world.combat.contrail_puffs, false);
        let bytes = world.checkpoint().unwrap();
        world.combat.contrails = puffs;
        world.combat.contrail_puffs = on;
        bytes
    }

    /// Slice S11: a world that makes no contrail puffs (a dedicated
    /// server's) steps exactly as one that makes them, but for the puffs:
    /// the same tick output, the same pictures, and a checkpoint equal to
    /// the other's with its puffs taken out. The sortie number, which every
    /// client's own contrails follow, is the same.
    #[test]
    fn a_world_without_contrail_puffs_steps_as_one_with_them_but_for_the_puffs() {
        let (mut a, mut b) = (high_mission(), high_mission());
        b.combat.set_contrail_puffs(false);
        assert!(a.combat.contrail_puffs() && !b.combat.contrail_puffs());
        let (mut out_a, mut out_b) = (TickOutput::default(), TickOutput::default());
        let mut most = 0;
        for n in 0..600 {
            if n == 62 {
                give_outlets(&mut a);
                give_outlets(&mut b);
            }
            step(&mut a, n, &mut out_a);
            step(&mut b, n, &mut out_b);
            assert_eq!(effects(&out_a), effects(&out_b), "tick {n}");
            assert_eq!(a.combat.render.current, b.combat.render.current);
            assert!(b.combat.contrails.puffs.is_empty(), "tick {n}");
            most = most.max(a.combat.contrails.puffs.len());
            if n % 50 == 49 {
                assert!(
                    checkpoint_without_puffs(&mut a) == b.checkpoint().unwrap(),
                    "the checkpoints but for the puffs at tick {n}"
                );
            }
        }
        assert!(most > 100, "the other world made contrails: {most} puffs");
        assert_eq!(a.combat.contrail_sortie(), b.combat.contrail_sortie());
    }

    /// The switch travels in the checkpoint: a world restored from one that
    /// makes no puffs makes none either, and both fly on alike.
    #[test]
    fn the_contrail_switch_is_restored_with_the_checkpoint() {
        let (mut a, mut b) = (high_mission(), high_mission());
        a.combat.set_contrail_puffs(false);
        let (mut out_a, mut out_b) = (TickOutput::default(), TickOutput::default());
        for n in 0..200 {
            if n == 62 {
                give_outlets(&mut a);
            }
            step(&mut a, n, &mut out_a);
        }
        let bytes = a.checkpoint().unwrap();
        b.restore(&bytes).unwrap();
        assert!(!b.combat.contrail_puffs(), "restored off");
        for n in 200..500 {
            step(&mut a, n, &mut out_a);
            step(&mut b, n, &mut out_b);
            assert_eq!(effects(&out_a), effects(&out_b), "tick {n}");
            if n % 50 == 49 {
                assert!(
                    a.checkpoint().unwrap() == b.checkpoint().unwrap(),
                    "tick {n}"
                );
            }
        }
        assert!(b.combat.contrails.puffs.is_empty());
        // And back on: puffs come again.
        b.combat.set_contrail_puffs(true);
        for n in 500..560 {
            step(&mut b, n, &mut out_b);
        }
        assert!(!b.combat.contrails.puffs.is_empty());
    }
}
