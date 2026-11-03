//! The coders of the AI mission and its actors (docs/formats/checkpoint.md,
//! stage H slice H4): every AI aircraft with its flight, controller,
//! awareness, sensors, stores, orders, airfield sequence and escape monitor;
//! the wings' leaders, humans, assignments, opportunities and routes, and
//! the lead hold's claims and waiting re-forms (slice R2 of the lobby pass).
//!
//! `AiMission` and `AiActor` are coded as new values, since a handoff adds
//! and removes actors; the AI wings section (slice H6) restores them whole.
//! The controller (slice H5), the sensors and threat service (slice H1) and
//! the types in the sibling `*_checkpoint.rs` files are called, not coded
//! here.
//!
//! What is left out, by class:
//!
//! - **Why-records:** `AiMission::journal` (the message journal, drained by
//!   the host, which no decision reads); `AiActor::trace` (the latest step's
//!   explanation, started afresh on every step) and `AiActor::journal_memory`
//!   (which journal lines were already written; it only gates journal text)
//!   and `AiActor::last_visual` (the lookout's verdicts of the last
//!   observation, shown again in the trace between observations at an AI
//!   observation rate below 120 Hz).
//! - **Per-tick scratch:** `AiMission::missiles` and `AiMission::gun_rounds`
//!   (proof at the skip) and `AiActor::adapter` (proof at the skip).
//!
//! Every other field is coded, including the ones that look like caches:
//! `last_defense`, `pending_threats`, `pending_events`, `device_schedule`,
//! `observed_attacks`, `ignored_attack_ids` and `received_emitters` are all
//! read by a later tick, and `last_input`, which a destroyed actor keeps
//! from its last flown step.
//!
//! Copied records are shared records: the runway views of an actor (its home
//! runway, its landing order, its ground start and its airfield sequence)
//! and the weapon record of each of its guns.

use super::{
    AiActor, AiMission, EquipmentFaults, HumanMember, JournalMemory, LeadClaim, ObservedAttack,
    StationSpec, WingLeader,
};
use crate::ai::airfield::{LandingOrder, RunwayView};
use crate::ai::link::{SortStamp, Yield};
use crate::ai::thought::{ActorTrace, Journal};
use crate::ai::weapon_service::{Delay, ProjectilePacing};
use crate::checkpoint::{
    Checkpoint, CheckpointError, Loader, Saver, invalid, load_flight, save_flight,
};
use std::collections::BTreeMap;
use tore_formats::weapons::Weapon;

impl Checkpoint for Delay {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        self.quarter_count().save(s, None)
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(Delay::quarters(u32::load(l, None)?))
    }
}

crate::checkpoint_struct!(ProjectilePacing {
    burst_count,
    burst_interval,
    reload,
    startup,
});

crate::checkpoint_struct!(StationSpec {
    station,
    guided,
    capability,
    store,
    debit,
    external_round_lbs,
    projectile_count,
    employment_limit_deg,
    damage_vs_category,
    store_speed,
    tracking_delay,
    pacing,
    maximum_range_ft,
    energy_target,
    minimum_range_ft,
    requires_radar,
    requires_sensor,
    employment_zone,
    infrared_envelope,
    mount,
});

crate::checkpoint_struct!(ObservedAttack {
    report,
    bearing_world_deg,
    observed_tick,
    event_id,
});

crate::checkpoint_struct!(EquipmentFaults {
    radar,
    infrared,
    visual,
    rwr,
});

crate::checkpoint_struct!(HumanMember {
    id,
    side,
    wing,
    member,
    pilot_alive,
});

crate::checkpoint_struct!(Yield {
    target,
    until,
    announced,
});

crate::checkpoint_struct!(SortStamp { side, wing, tick });

crate::checkpoint_struct!(WingLeader {
    side,
    wing,
    leader,
    reformed,
    acting,
});

crate::checkpoint_struct!(LeadClaim {
    side,
    wing,
    plane,
    fresh,
});

/// An optional runway view, the view itself a shared record.
fn save_runway(s: &mut Saver, runway: &Option<RunwayView>) -> Result<(), CheckpointError> {
    s.writer().write_bool(runway.is_some());
    match runway {
        Some(view) => s.shared(view),
        None => Ok(()),
    }
}

fn load_runway(l: &mut Loader<'_>) -> Result<Option<RunwayView>, CheckpointError> {
    if l.reader().read_bool()? {
        Ok(Some(l.shared()?))
    } else {
        Ok(None)
    }
}

/// An optional landing order: its runway view a shared record, then its
/// reason.
fn save_landing_order(s: &mut Saver, order: &Option<LandingOrder>) -> Result<(), CheckpointError> {
    s.writer().write_bool(order.is_some());
    if let Some(LandingOrder { runway, reason }) = order {
        s.shared(runway)?;
        reason.save(s, None)?;
    }
    Ok(())
}

fn load_landing_order(l: &mut Loader<'_>) -> Result<Option<LandingOrder>, CheckpointError> {
    if !l.reader().read_bool()? {
        return Ok(None);
    }
    let runway = l.shared()?;
    let reason = Checkpoint::load(l, None)?;
    Ok(Some(LandingOrder { runway, reason }))
}

/// An actor's gun records, by station: each weapon a shared record.
fn save_guns(s: &mut Saver, guns: &BTreeMap<u8, Weapon>) -> Result<(), CheckpointError> {
    s.count(guns.len());
    for (station, weapon) in guns {
        station.save(s, None)?;
        s.shared(weapon)?;
    }
    Ok(())
}

fn load_guns(l: &mut Loader<'_>) -> Result<BTreeMap<u8, Weapon>, CheckpointError> {
    let count = l.count()?;
    let mut guns = BTreeMap::new();
    for _ in 0..count {
        let station = u8::load(l, None)?;
        let weapon = l.shared()?;
        if guns.insert(station, weapon).is_some() {
            return invalid(format!("gun station {station} coded twice"));
        }
    }
    Ok(guns)
}

impl Checkpoint for AiActor {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        // `adapter`: per-tick scratch and a report. Its `last_requested` is
        // written by `ControlAdapter::controls` for tests and reporting and
        // read by nothing in the simulation. Its `gun_aim`, `strike_handling`
        // and defensive handling are cleared by `step_actor` before anything
        // else and set again before `controls`, in the same step.
        //
        // `trace` and `journal_memory`: why-records.
        let AiActor {
            identity,
            controller,
            adapter: _,
            flight,
            sensors,
            awareness,
            search_target,
            missile_threats,
            defense_state,
            last_defense,
            incoming_fire,
            lookout,
            fire_defending,
            assignment,
            mission_policy,
            observed_attacks,
            neutral,
            formation_order_tick,
            ignored_attack_ids,
            received_emitters,
            equipment,
            damage_return,
            stations,
            guns,
            gun_target,
            dispensers,
            wing_slot,
            home_airport,
            home_runway,
            ground_start,
            landing_order,
            bugged_out,
            join_cancelled,
            avoiding,
            avoiding_from,
            airfield,
            route_random,
            pending_threats,
            pending_events,
            device_schedule,
            activity,
            last_input,
            alive,
            dummy,
            escape_monitor,
            yields,
            ground,
            seed,
            strike,
            lase,
            rotor,
            jet_lift,
            trace: _,
            journal_memory: _,
            last_visual: _,
        } = self;
        identity.save(s, None)?;
        controller.save(s, None)?;
        save_flight(s, flight, identity.aircraft)?;
        sensors.save(s, None)?;
        awareness.save(s, None)?;
        search_target.save(s, None)?;
        missile_threats.save(s, None)?;
        defense_state.save(s, None)?;
        last_defense.save(s, None)?;
        incoming_fire.save(s, None)?;
        lookout.save(s, None)?;
        fire_defending.save(s, None)?;
        assignment.save(s, None)?;
        mission_policy.save(s, None)?;
        observed_attacks.save(s, None)?;
        neutral.save(s, None)?;
        formation_order_tick.save(s, None)?;
        ignored_attack_ids.save(s, None)?;
        received_emitters.save(s, None)?;
        equipment.save(s, None)?;
        damage_return.save(s, None)?;
        stations.save(s, None)?;
        save_guns(s, guns)?;
        gun_target.save(s, None)?;
        dispensers.save(s, None)?;
        wing_slot.save(s, None)?;
        home_airport.save(s, None)?;
        save_runway(s, home_runway)?;
        ground_start.save(s, None)?;
        save_landing_order(s, landing_order)?;
        bugged_out.save(s, None)?;
        join_cancelled.save(s, None)?;
        avoiding.save(s, None)?;
        avoiding_from.save(s, None)?;
        airfield.save(s, None)?;
        route_random.save(s, None)?;
        pending_threats.save(s, None)?;
        pending_events.save(s, None)?;
        device_schedule.save(s, None)?;
        activity.save(s, None)?;
        // The controls the actor flew last (slice H9): read only by the
        // formation trace and the replay recorder's control display, and
        // rewritten by every step that flies, but a destroyed actor no
        // longer flies, so they are coded rather than skipped as scratch.
        last_input.save(s, None)?;
        alive.save(s, None)?;
        dummy.save(s, None)?;
        escape_monitor.save(s, None)?;
        // The bandits it left alone for a flightmate (slice G4): read by
        // every choice until each lapses.
        yields.save(s, None)?;
        // A striker's ground picture (ST-P): the lists, memory, briefing and
        // threat inputs the next refresh and the strike choice read.
        ground.save(s, None)?;
        // A striker's decision seed and strike state (ST-T): the phase, pass
        // and aim the next step flies on.
        seed.save(s, None)?;
        strike.save(s, None)?;
        // The ground target a wingman was ordered to lase (ST-O).
        lase.save(s, None)?;
        // The rotorcraft and jet-lift laws' memory (slice H1): integrators
        // and regime the next tick flies on.
        rotor.save(s, None)?;
        jet_lift.save(s, None)
    }

    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        let identity: crate::ai::controller::ActorIdentity = Checkpoint::load(l, None)?;
        let controller = Checkpoint::load(l, None)?;
        let (aircraft, flight) = load_flight(l)?;
        if aircraft != identity.aircraft {
            return invalid(format!(
                "an actor of {:?} flies a {aircraft:?}",
                identity.aircraft
            ));
        }
        Ok(AiActor {
            identity,
            controller,
            adapter: crate::ai::steering_adapter::ControlAdapter::new(),
            flight,
            sensors: Checkpoint::load(l, None)?,
            awareness: Checkpoint::load(l, None)?,
            search_target: Checkpoint::load(l, None)?,
            missile_threats: Checkpoint::load(l, None)?,
            defense_state: Checkpoint::load(l, None)?,
            last_defense: Checkpoint::load(l, None)?,
            incoming_fire: Checkpoint::load(l, None)?,
            lookout: Checkpoint::load(l, None)?,
            fire_defending: Checkpoint::load(l, None)?,
            assignment: Checkpoint::load(l, None)?,
            mission_policy: Checkpoint::load(l, None)?,
            observed_attacks: Checkpoint::load(l, None)?,
            neutral: Checkpoint::load(l, None)?,
            formation_order_tick: Checkpoint::load(l, None)?,
            ignored_attack_ids: Checkpoint::load(l, None)?,
            received_emitters: Checkpoint::load(l, None)?,
            equipment: Checkpoint::load(l, None)?,
            damage_return: Checkpoint::load(l, None)?,
            stations: Checkpoint::load(l, None)?,
            guns: load_guns(l)?,
            gun_target: Checkpoint::load(l, None)?,
            dispensers: Checkpoint::load(l, None)?,
            wing_slot: Checkpoint::load(l, None)?,
            home_airport: Checkpoint::load(l, None)?,
            home_runway: load_runway(l)?,
            ground_start: Checkpoint::load(l, None)?,
            landing_order: load_landing_order(l)?,
            bugged_out: Checkpoint::load(l, None)?,
            join_cancelled: Checkpoint::load(l, None)?,
            avoiding: Checkpoint::load(l, None)?,
            avoiding_from: Checkpoint::load(l, None)?,
            airfield: Checkpoint::load(l, None)?,
            route_random: Checkpoint::load(l, None)?,
            pending_threats: Checkpoint::load(l, None)?,
            pending_events: Checkpoint::load(l, None)?,
            device_schedule: Checkpoint::load(l, None)?,
            activity: Checkpoint::load(l, None)?,
            last_input: Checkpoint::load(l, None)?,
            alive: Checkpoint::load(l, None)?,
            dummy: Checkpoint::load(l, None)?,
            escape_monitor: Checkpoint::load(l, None)?,
            yields: Checkpoint::load(l, None)?,
            ground: Checkpoint::load(l, None)?,
            seed: Checkpoint::load(l, None)?,
            strike: Checkpoint::load(l, None)?,
            lase: Checkpoint::load(l, None)?,
            rotor: Checkpoint::load(l, None)?,
            jet_lift: Checkpoint::load(l, None)?,
            trace: ActorTrace::default(),
            journal_memory: JournalMemory::default(),
            last_visual: Vec::new(),
        })
    }
}

// `missiles` and `gun_rounds`: per-tick scratch. The host hands the mission
// the complete lifecycle snapshot with `set_missiles` and `set_gun_rounds`
// before every step (`AiWings::step` sets both on every call, empty when
// there is nothing in flight, and the golden fixtures set them every tick),
// and the only reads are inside that step (`update_defense`). A restored
// mission starts with none and the next tick's host sets them first. A test
// that sets them once and steps many times must set them again after a
// restore.
//
// `ground_ceiling`: host setup, not state. The host sets it from its terrain
// before every step (`AiWings::step`); it only spares terrain samples that
// could not change an answer, so a restored mission without it steps the same.
//
// `test`: the AI test settings (observation rate, lean sensors), host setup
// like the ground ceiling: the host sets them before every step. What they
// hold between two observations (the awareness memory, the sensors, the
// lookout, the received emitters) is all coded above, and the phase of an
// actor's observations is `(tick + index) % divisor`, so a checkpoint taken
// between observations resumes exactly while the host keeps the setting.
//
// `link`: per-step scratch too. The host hands the mission the data link's
// input with `set_link` before every step and the step takes it
// (`AiWings::step`'s caller does it every tick, empty when nothing is locked),
// so between steps it is always empty and a restored mission starts the same.
//
// `journal`: why-record. Nothing reads the message journal; the host drains
// it for the recorder.
crate::checkpoint_struct!(AiMission {
    actors,
    tick,
    wing_control,
    formation,
    horizontal_spacing_ft,
    vertical_spacing_ft,
    humans,
    leaders,
    humans_flying,
    human_assignments,
    must_survive,
    pending_attack_reports,
    priority_landing,
    hostiles_seen,
    airborne_seen,
    opportunities,
    routes,
    sort_clock,
    lead_claims,
    reform_after,
} skip {
    missiles = Vec::new(),
    gun_rounds = Vec::new(),
    ground_ceiling = None,
    test = crate::ai::test_settings::AiTestSettings::default(),
    link = crate::ai::link::LinkInput::default(),
    journal = Journal::default(),
});

#[cfg(test)]
#[path = "mission_checkpoint_tests.rs"]
mod tests;
