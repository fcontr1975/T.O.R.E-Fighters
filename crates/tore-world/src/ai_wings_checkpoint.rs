//! The coders of the AI wings (the AI wings section), restored in place around
//! the AI mission (docs/formats/checkpoint.md).
//!
//! Stage H slice H6. The bridge's mutable state: the AI mission (slice H4's
//! coder), the slots, the humans, each aircraft's configuration and weapons
//! (shared records), damaged stations, handed-over skills, the two random
//! streams, the pending gun bursts, the last hit points, activities, the
//! projectile counter, the HUD line state and the shot attribution. The
//! chatter watch and the formation reports have their coders in
//! `chatter_checkpoint.rs` and `reports_checkpoint.rs`.
//!
//! A restore keeps the fresh bridge's setup (the theater's airfields, the
//! mission preset) and replaces everything else, structure included: handoffs
//! add and remove slots and actors mid-flight.
//!
//! Skipped, with their classes:
//!
//! - `mission_preset`: setup. `apply_mission_preset` writes it when the world
//!   is built, and nothing reads it afterward.
//! - `airfields`: setup, the theater's airfields kept for inserted aircraft;
//!   the fresh world built from the same mission has them.
//! - `formation_trace`: local, a diagnostic the app turns on for its CSV file.
//!   It is never read by a decision, and a restored bridge keeps the fresh
//!   one's setting.
//! - `last_output` and `decoy_rolls`: why-records (what the last step
//!   produced, for the replay recorder and the debug panels; the doc comments
//!   say nothing reads them back, and the next step rewrites both).
//!
//! `launch::Side` belongs to `tore-sim`, whose trait this crate cannot
//! implement for it (the orphan rule), so a side is coded by [`save_side`] and
//! [`load_side`], the way the roster codes a wing.

use super::{AiFlightModel, AiWings, HumanSlot, PendingGun, Slot};
use tore_sim::ai::launch::Side;
use tore_sim::checkpoint::{Checkpoint, CheckpointError, InPlace, Loader, Saver, invalid};
use tore_sim::combat::missiles::Rules;

/// Writes a wing side: one bit, every variant named so a new one fails to
/// compile here.
pub(super) fn save_side(s: &mut Saver, side: Side) {
    s.writer().write_bool(match side {
        Side::Friendly => false,
        Side::Enemy => true,
    });
}

/// Reads a side [`save_side`] wrote.
pub(super) fn load_side(l: &mut Loader<'_>) -> Result<Side, CheckpointError> {
    Ok(if l.reader().read_bool()? {
        Side::Enemy
    } else {
        Side::Friendly
    })
}

impl Checkpoint for Slot {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let Slot {
            id,
            side,
            wing_number,
            member_number,
            aircraft,
        } = self;
        id.save(s, None)?;
        save_side(s, *side);
        wing_number.save(s, None)?;
        member_number.save(s, None)?;
        aircraft.save(s, None)
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(Slot {
            id: Checkpoint::load(l, None)?,
            side: load_side(l)?,
            wing_number: Checkpoint::load(l, None)?,
            member_number: Checkpoint::load(l, None)?,
            aircraft: Checkpoint::load(l, None)?,
        })
    }
}

impl Checkpoint for HumanSlot {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let HumanSlot {
            id,
            side,
            wing,
            member,
        } = self;
        id.save(s, None)?;
        save_side(s, *side);
        wing.save(s, None)?;
        member.save(s, None)
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(HumanSlot {
            id: Checkpoint::load(l, None)?,
            side: load_side(l)?,
            wing: Checkpoint::load(l, None)?,
            member: Checkpoint::load(l, None)?,
        })
    }
}

tore_sim::checkpoint_enum!(AiFlightModel {
    Standard = 0,
    AllHybrid = 1,
});

// `Rules` belongs to `tore-sim`; the bridge reads its copy of the combat
// state's rule, so it is coded as the one varint.
fn save_rules(s: &mut Saver, rules: Rules) {
    s.writer().write_varint(match rules {
        Rules::Compatibility => 0,
        Rules::Spec => 1,
        Rules::Energy => 2,
    });
}

fn load_rules(l: &mut Loader<'_>) -> Result<Rules, CheckpointError> {
    match l.reader().read_varint()? {
        0 => Ok(Rules::Compatibility),
        1 => Ok(Rules::Spec),
        2 => Ok(Rules::Energy),
        other => invalid(format!("the weapon rules have no variant {other}")),
    }
}

tore_sim::checkpoint_struct!(PendingGun {
    groups,
    next_scaled,
    ordinal,
});

/// Writes a map whose values are shared records: the key, then the record's
/// index.
fn save_shared_map<K: Checkpoint, V: Checkpoint>(
    s: &mut Saver,
    map: &std::collections::BTreeMap<K, V>,
) -> Result<(), CheckpointError> {
    s.count(map.len());
    for (key, value) in map {
        key.save(s, None)?;
        s.shared(value)?;
    }
    Ok(())
}

/// Reads a map [`save_shared_map`] wrote; the keys must ascend.
fn load_shared_map<K: Checkpoint + Ord, V: Checkpoint>(
    l: &mut Loader<'_>,
) -> Result<std::collections::BTreeMap<K, V>, CheckpointError> {
    let count = l.count()?;
    let mut map = std::collections::BTreeMap::new();
    for _ in 0..count {
        let key = K::load(l, None)?;
        if map.last_key_value().is_some_and(|(last, _)| *last >= key) {
            return invalid("map keys out of order");
        }
        let value = l.shared()?;
        map.insert(key, value);
    }
    Ok(map)
}

impl InPlace for AiWings {
    fn save_in_place(&self, s: &mut Saver) -> Result<(), CheckpointError> {
        // The mission first: it is a value of its own (slice H4).
        self.mission.save(s, None)?;
        self.save_bridge(s)
    }

    fn restore_in_place(&mut self, l: &mut Loader<'_>) -> Result<(), CheckpointError> {
        self.mission = Checkpoint::load(l, None)?;
        self.restore_bridge(l)
    }
}

impl AiWings {
    /// Writes everything but the AI mission. Apart from the section so that
    /// the bridge's own coding can be tested before the mission's is coded.
    pub(crate) fn save_bridge(&self, s: &mut Saver) -> Result<(), CheckpointError> {
        let AiWings {
            // Coded by `save_in_place`, ahead of the rest.
            mission: _,
            // Setup: written by `apply_mission_preset` at build, never read.
            mission_preset: _,
            reports,
            // Local: the app's diagnostic file feed.
            formation_trace: _,
            slots,
            flight_model,
            // Setup: the theater's airfields, from the same mission.
            airfields: _,
            humans,
            weapons,
            device_random,
            device_effectiveness,
            configs,
            damaged_stations,
            handed_over,
            handed_tasking,
            fault_random,
            seen_projectiles,
            // Derived: the surface missiles' reach at a helicopter, a fixed
            // function of the records, worked out again on demand.
            rotor_reach: _,
            flight_times: _,
            shot_cache: _,
            ai_shots,
            last_hp,
            uncredited_losses,
            ejection_events,
            chatter,
            watch,
            last_activity,
            next_projectile_id,
            weapon_rules,
            last_message_tick,
            shown_actor,
            stale_line,
            dropped_launches,
            realised_launches,
            threat_reports,
            pending_message,
            pending_guns,
            gun_ordinals,
            enemy_skill,
            mission_skill,
            guns_only,
            departing,
            // Why-record: what the last step produced, for the recorder and
            // the debug panels; the next step rewrites it.
            last_output: _,
            // Why-record: this step's decoy rolls, for the recorder.
            decoy_rolls: _,
            // Setup: worked out from the world's terrain, which never changes.
            ground_ceiling: _,
            // Setup: the AI test settings, set by the host.
            ai_test: _,
            // Setup: the host sets it before every step (slice ST-T2).
            mission_height_ft: _,
            // Setup: the host sets it before every step (slice H4).
            painting: _,
        } = self;
        reports.save(s, None)?;
        slots.save(s, None)?;
        flight_model.save(s, None)?;
        humans.save(s, None)?;
        save_shared_map(s, weapons)?;
        device_random.save(s, None)?;
        device_effectiveness.save(s, None)?;
        save_shared_map(s, configs)?;
        damaged_stations.save(s, None)?;
        handed_over.save(s, None)?;
        handed_tasking.save(s, None)?;
        fault_random.save(s, None)?;
        seen_projectiles.save(s, None)?;
        ai_shots.save(s, None)?;
        last_hp.save(s, None)?;
        uncredited_losses.save(s, None)?;
        ejection_events.save(s, None)?;
        chatter.save(s, None)?;
        watch.save_in_place(s)?;
        last_activity.save(s, None)?;
        next_projectile_id.save(s, None)?;
        save_rules(s, *weapon_rules);
        last_message_tick.save(s, None)?;
        shown_actor.save(s, None)?;
        stale_line.save(s, None)?;
        dropped_launches.save(s, None)?;
        realised_launches.save(s, None)?;
        threat_reports.save(s, None)?;
        pending_message.save(s, None)?;
        pending_guns.save(s, None)?;
        gun_ordinals.save(s, None)?;
        enemy_skill.save(s, None)?;
        mission_skill.save(s, None)?;
        guns_only.save(s, None)?;
        departing.save(s, None)
    }

    /// Reads what [`Self::save_bridge`] wrote into this bridge, keeping its
    /// setup and its mission.
    pub(crate) fn restore_bridge(&mut self, l: &mut Loader<'_>) -> Result<(), CheckpointError> {
        let AiWings {
            mission: _,
            mission_preset: _,
            reports,
            formation_trace: _,
            slots,
            flight_model,
            airfields: _,
            humans,
            weapons,
            device_random,
            device_effectiveness,
            configs,
            damaged_stations,
            handed_over,
            handed_tasking,
            fault_random,
            seen_projectiles,
            // Derived: the surface missiles' reach at a helicopter, a fixed
            // function of the records, worked out again on demand.
            rotor_reach: _,
            flight_times: _,
            shot_cache: _,
            ai_shots,
            last_hp,
            uncredited_losses,
            ejection_events,
            chatter,
            watch,
            last_activity,
            next_projectile_id,
            weapon_rules,
            last_message_tick,
            shown_actor,
            stale_line,
            dropped_launches,
            realised_launches,
            threat_reports,
            pending_message,
            pending_guns,
            gun_ordinals,
            enemy_skill,
            mission_skill,
            guns_only,
            departing,
            last_output: _,
            decoy_rolls: _,
            ground_ceiling: _,
            // Setup: the AI test settings, set by the host.
            ai_test: _,
            // Setup: the host sets it before every step (slice ST-T2).
            mission_height_ft: _,
            // Setup: the host sets it before every step (slice H4).
            painting: _,
        } = self;
        *reports = Checkpoint::load(l, None)?;
        *slots = Checkpoint::load(l, None)?;
        *flight_model = Checkpoint::load(l, None)?;
        *humans = Checkpoint::load(l, None)?;
        *weapons = load_shared_map(l)?;
        *device_random = Checkpoint::load(l, None)?;
        *device_effectiveness = Checkpoint::load(l, None)?;
        *configs = load_shared_map(l)?;
        *damaged_stations = Checkpoint::load(l, None)?;
        *handed_over = Checkpoint::load(l, None)?;
        *handed_tasking = Checkpoint::load(l, None)?;
        *fault_random = Checkpoint::load(l, None)?;
        *seen_projectiles = Checkpoint::load(l, None)?;
        *ai_shots = Checkpoint::load(l, None)?;
        *last_hp = Checkpoint::load(l, None)?;
        *uncredited_losses = Checkpoint::load(l, None)?;
        *ejection_events = Checkpoint::load(l, None)?;
        *chatter = Checkpoint::load(l, None)?;
        watch.restore_in_place(l)?;
        *last_activity = Checkpoint::load(l, None)?;
        *next_projectile_id = Checkpoint::load(l, None)?;
        *weapon_rules = load_rules(l)?;
        *last_message_tick = Checkpoint::load(l, None)?;
        *shown_actor = Checkpoint::load(l, None)?;
        *stale_line = Checkpoint::load(l, None)?;
        *dropped_launches = Checkpoint::load(l, None)?;
        *realised_launches = Checkpoint::load(l, None)?;
        *threat_reports = Checkpoint::load(l, None)?;
        *pending_message = Checkpoint::load(l, None)?;
        *pending_guns = Checkpoint::load(l, None)?;
        *gun_ordinals = Checkpoint::load(l, None)?;
        *enemy_skill = Checkpoint::load(l, None)?;
        *mission_skill = Checkpoint::load(l, None)?;
        *guns_only = Checkpoint::load(l, None)?;
        *departing = Checkpoint::load(l, None)?;
        Ok(())
    }
}

#[cfg(test)]
impl AiWings {
    /// Everything [`Self::save_bridge`] codes, as text, for tests to compare
    /// (floats print in their exact shortest form).
    pub(crate) fn bridge_digest(&self) -> String {
        let guns: Vec<_> = self
            .pending_guns
            .iter()
            .map(|(key, gun)| (key, &gun.groups, gun.next_scaled, gun.ordinal))
            .collect();
        format!(
            "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n\
             {:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
            self.slots,
            self.flight_model,
            self.humans,
            self.weapons,
            self.device_effectiveness,
            self.configs,
            self.damaged_stations,
            (&self.handed_over, &self.handed_tasking),
            self.seen_projectiles,
            self.ai_shots,
            self.last_hp,
            self.uncredited_losses,
            self.ejection_events,
            self.chatter,
            self.last_activity,
            self.next_projectile_id,
            self.weapon_rules,
            self.last_message_tick,
            self.shown_actor,
            self.stale_line,
            (self.dropped_launches, self.realised_launches),
            self.threat_reports,
            self.pending_message,
            guns,
            self.gun_ordinals,
            (self.enemy_skill, &self.mission_skill),
            (self.guns_only, &self.departing),
        )
    }

    /// How much of each coded collection the bridge holds, so a test can say
    /// its fixture exercised what the coder carries.
    pub(crate) fn bridge_census(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("slots", self.slots.len()),
            ("humans", self.humans.len()),
            ("weapons", self.weapons.len()),
            ("device effectiveness", self.device_effectiveness.len()),
            ("configurations", self.configs.len()),
            ("damaged stations", self.damaged_stations.len()),
            ("handed over", self.handed_over.len()),
            ("handed tasking", self.handed_tasking.len()),
            ("seen projectiles", self.seen_projectiles.len()),
            ("AI shots", self.ai_shots.len()),
            ("last hit points", self.last_hp.len()),
            ("uncredited losses", self.uncredited_losses.len()),
            ("ejection events", self.ejection_events.len()),
            ("chatter", self.chatter.len()),
            ("activities", self.last_activity.len()),
            ("threat reports", self.threat_reports.len()),
            ("pending guns", self.pending_guns.len()),
            ("gun ordinals", self.gun_ordinals.len()),
            ("mission skills", self.mission_skill.len()),
            ("departing", self.departing.len()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::super::Chatter;
    use super::super::tests::*;
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use tore_formats::aircraft::AircraftId;
    use tore_sim::ai::controller::Activity;
    use tore_sim::ai::experience::{ExperienceOrigin, ResolvedExperience};
    use tore_sim::ai::{Experience, launch};
    use tore_sim::checkpoint::{Coded, Models, from_bytes, to_bytes};

    fn wings() -> AiWings {
        AiWings::build_with(&payload(None), &spawned(), 0, |_| Ok((aircraft(), None))).unwrap()
    }

    fn coded(wings: &AiWings) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut s = Saver::with_models(Models::default());
        wings.save_bridge(&mut s).unwrap();
        let body = s.finish_section();
        (body, s.into_records())
    }

    fn restore(
        wings: &mut AiWings,
        bytes: &(Vec<u8>, Vec<Vec<u8>>),
    ) -> Result<(), CheckpointError> {
        let models = Models::default();
        let mut l = Loader::new(&bytes.0, &bytes.1, &models);
        wings.restore_bridge(&mut l)?;
        l.finish()
    }

    /// A bridge with every coded field away from its start value.
    fn lived_in() -> AiWings {
        let mut wings = wings();
        wings.humans.push(HumanSlot {
            id: 9,
            side: launch::Side::Enemy,
            wing: 2,
            member: 3,
        });
        wings.flight_model = AiFlightModel::AllHybrid;
        wings
            .device_effectiveness
            .extend([(1, (20, 30)), (3, (0, 255))]);
        wings.damaged_stations.extend([(1, 0), (1, 4), (3, 2)]);
        wings.handed_over.insert(
            2,
            ResolvedExperience {
                level: Experience::Ace,
                origin: ExperienceOrigin::QuickMission {
                    selected: Experience::Novice,
                },
            },
        );
        wings.handed_tasking.insert(
            2,
            crate::ai_wings::takeover::Tasking {
                role: tore_sim::ai::engagement::StrikeRole::Strike,
                order: Some(0x5000_0001),
                class_order: vec![0x5000_0002, 0x5000_0003],
                lase: Some(0x5000_0004),
            },
        );
        wings.mission_skill.insert(
            3,
            ResolvedExperience {
                level: Experience::Experienced,
                origin: ExperienceOrigin::EnemyOverride,
            },
        );
        wings.seen_projectiles.extend([1 << 24, (1 << 24) + 1, 7]);
        wings.ai_shots.extend([(1 << 24, 1), (7, 3)]);
        wings.last_hp.extend([(1, 100), (2, -3), (3, i32::MIN)]);
        wings.uncredited_losses.extend([2, 4]);
        wings
            .ejection_events
            .push((2, "F18.PT ejected".into(), true));
        wings.chatter = vec![
            Chatter::Showtime { speaker: 1 },
            Chatter::Leadership {
                speaker: 2,
                side: launch::Side::Enemy,
                wing_number: 2,
                leader: 4,
                previous_pilot_alive: true,
            },
        ];
        wings.last_activity.extend([
            (1, Activity::Attacking),
            (2, Activity::HoldingMarshal),
            (3, Activity::Destroyed),
        ]);
        wings.next_projectile_id = (1 << 24) + 99;
        wings.weapon_rules = Rules::Compatibility;
        wings.last_message_tick = 4321;
        wings.shown_actor = Some(3);
        wings.stale_line = Some((2, Activity::Evading));
        wings.dropped_launches = 2;
        wings.realised_launches = 14;
        wings.threat_reports = vec![(1, 7), (3, 8)];
        wings.pending_message = Some("Friendly 2-1: Attacking".into());
        wings.pending_guns.insert(
            (3, 1),
            PendingGun {
                groups: VecDeque::from([(10, 3), (5, 3)]),
                next_scaled: 123_456,
                ordinal: 5,
            },
        );
        wings.gun_ordinals.insert((3, 1), 12);
        wings.enemy_skill = Some(Experience::Ace);
        wings.guns_only = true;
        wings.departing.insert(1);
        wings
    }

    #[test]
    fn a_lived_in_bridge_round_trips_into_a_fresh_one() {
        let original = lived_in();
        assert!(original.bridge_census().iter().all(|(what, n)| {
            // The fixture's two aircraft hold no configurations or weapons;
            // the world tests carry those from real fixtures.
            *n > 0 || ["weapons", "configurations"].contains(what)
        }));
        let bytes = coded(&original);
        let mut fresh = wings();
        restore(&mut fresh, &bytes).unwrap();
        assert_eq!(fresh.bridge_digest(), original.bridge_digest());
        assert!(coded(&fresh) == bytes);
        assert_eq!(fresh.weapon_rules, Rules::Compatibility);
        assert_eq!(fresh.pending_guns[&(3, 1)].next_scaled, 123_456);
    }

    #[test]
    fn a_restore_keeps_the_setup_and_replaces_the_structure() {
        let original = lived_in();
        let bytes = coded(&original);
        // A fresh bridge with a different structure, and setup of its own.
        let mut fresh = wings();
        fresh.slots.pop();
        fresh.humans.clear();
        fresh.mission_preset = super::super::Preset::Escort;
        fresh.formation_trace = Some(super::super::FormationBatch::default());
        fresh.last_hp.insert(99, 1);
        restore(&mut fresh, &bytes).unwrap();
        assert_eq!(fresh.slots.len(), original.slots.len());
        assert_eq!(fresh.humans, original.humans);
        assert!(!fresh.last_hp.contains_key(&99));
        // Setup and local diagnostics are the fresh bridge's own.
        assert_eq!(fresh.mission_preset, super::super::Preset::Escort);
        assert!(fresh.formation_trace.is_some());
    }

    #[test]
    fn why_records_are_not_coded() {
        let quiet = lived_in();
        let mut loud = lived_in();
        loud.formation_trace = Some(super::super::FormationBatch::default());
        loud.mission_preset = super::super::Preset::Hold;
        assert!(coded(&quiet) == coded(&loud));
    }

    #[test]
    fn sides_and_slots_round_trip() {
        for side in [launch::Side::Friendly, launch::Side::Enemy] {
            let slot = Slot {
                id: 31,
                side,
                wing_number: 3,
                member_number: 5,
                aircraft: AircraftId::Su27,
            };
            let coded: Coded = to_bytes(&slot, &Models::default()).unwrap();
            assert_eq!(
                from_bytes::<Slot>(&coded, &Models::default()).unwrap(),
                slot
            );
            let human = HumanSlot {
                id: 4,
                side,
                wing: 1,
                member: 2,
            };
            let coded: Coded = to_bytes(&human, &Models::default()).unwrap();
            assert_eq!(
                from_bytes::<HumanSlot>(&coded, &Models::default()).unwrap(),
                human
            );
        }
    }

    #[test]
    fn maps_of_shared_records_refuse_keys_out_of_order() {
        let models = Models::default();
        let mut map = BTreeMap::new();
        map.extend([((1u32, 0u8), 7u32), ((1, 3), 7), ((4, 1), 9)]);
        let mut s = Saver::with_models(models.clone());
        save_shared_map(&mut s, &map).unwrap();
        let body = s.finish_section();
        let records = s.into_records();
        // Equal values share a record: three entries, two records.
        assert_eq!(records.len(), 2);
        let mut l = Loader::new(&body, &records, &models);
        let copy: BTreeMap<(u32, u8), u32> = load_shared_map(&mut l).unwrap();
        assert_eq!(copy, map);

        // Written backwards by hand: the reader refuses it.
        let mut s = Saver::with_models(models.clone());
        s.count(2);
        for key in [(4u32, 1u8), (1, 0)] {
            key.save(&mut s, None).unwrap();
            s.shared(&7u32).unwrap();
        }
        let body = s.finish_section();
        let records = s.into_records();
        let mut l = Loader::new(&body, &records, &models);
        assert!(load_shared_map::<(u32, u8), u32>(&mut l).is_err());
    }

    #[test]
    fn damaged_bytes_are_refused_without_a_panic() {
        let bytes = coded(&lived_in());
        let mut fresh = wings();
        for cut in 0..bytes.0.len() {
            let _ = restore(&mut fresh, &(bytes.0[..cut].to_vec(), bytes.1.clone()));
        }
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..5_000 {
            let mut damaged = bytes.0.clone();
            let at = next() as usize % damaged.len();
            damaged[at] ^= 1 << (next() % 8);
            let _ = restore(&mut fresh, &(damaged, bytes.1.clone()));
        }
    }
}
