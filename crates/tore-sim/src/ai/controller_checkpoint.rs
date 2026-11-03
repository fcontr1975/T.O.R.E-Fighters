//! The coders of an AI pilot's controller and everything it owns directly
//! (docs/formats/checkpoint.md, stage H slice H5): its identity and profile,
//! the maneuver it is flying, the intent batch it last produced, its searches,
//! defense motion and pending launch warnings, and its fitted fallback log.
//! The parts it owns that have their own module (the weapon service, gunnery,
//! formation guidance, wing orders, motion requests, steering mode, pursuit
//! offset and script reason) are coded beside those modules.
//!
//! Three fields are left out, each with its class:
//!
//! - `gun_views` and `formation_traffic` are per-tick scratch (proofs at the
//!   skip);
//! - `trace` is a why-record: nothing reads it, and it is outside equality.

use super::{
    ActiveManeuver, ActorIdentity, BehaviorFamily, BehaviorProfile, Completion, Controller,
    DefenseMotion, DefenseSource, DeviceIntent, FallbackLog, IntentBatch, MissionRole,
    MotionIntent, SearchContact, SensorIntent, WeaponIntent,
};
use crate::ai::fitted::Fallback;
use crate::checkpoint::{Checkpoint, CheckpointError, Loader, Saver, invalid};

crate::checkpoint_enum!(BehaviorFamily {
    FighterStrike = 0,
    F117 = 1,
    Helicopter = 2,
    Bomber = 3,
    AC130 = 4,
    LargeAircraft = 5,
    Airliner = 6,
    Moth = 7,
});

crate::checkpoint_enum!(MissionRole {
    AirToAir = 0,
    AirToGround = 1,
    Escort = 2,
});

crate::checkpoint_struct!(ActorIdentity {
    actor,
    side,
    wing,
    member,
    leads,
    aircraft,
    human_controlled,
});

crate::checkpoint_struct!(BehaviorProfile { family, role });

crate::checkpoint_enum!(DefenseSource {
    Missile = 0,
    IncomingFire = 1,
});

crate::checkpoint_struct!(DefenseMotion {
    source,
    heading_deg,
    pitch_deg,
    rotorcraft,
    extend,
});

crate::checkpoint_struct!(SearchContact {
    id,
    position,
    observed_tick,
});

// The numbers are the variants' places in `Fallback::ALL`, which is also the
// index `FallbackLog` counts them by.
crate::checkpoint_enum!(Fallback {
    EngagementPitch = 0,
    BasePitchRate = 1,
    CompletionAxis = 2,
    LastDitchCandidate = 3,
    RandomTacticMenu = 4,
    RemainingTactics = 5,
    LeadSpeedEstimator = 6,
    BurstPacing = 7,
    HitChance = 8,
    LeaderReturnToBase = 9,
});

crate::checkpoint_struct!(FallbackLog { counts });

impl Checkpoint for Completion {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::Deadline(deadline) => {
                s.writer().write_varint(0);
                deadline.save(s, None)?;
            }
            Self::Axis(axis) => {
                s.writer().write_varint(1);
                axis.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::Deadline(Checkpoint::load(l, None)?),
            1 => Self::Axis(Checkpoint::load(l, None)?),
            other => return invalid(format!("Completion has no variant {other}")),
        })
    }
}

crate::checkpoint_struct!(MotionIntent {
    id,
    request,
    heading_deg,
    flight_path_pitch_deg,
    speed,
    bank,
    completion,
    steering_point,
    mode,
    formation_flight,
    afterburner,
});

crate::checkpoint_struct!(SensorIntent {
    designate,
    clear_designation,
});

crate::checkpoint_struct!(WeaponIntent { request });

crate::checkpoint_struct!(DeviceIntent { class, count });

crate::checkpoint_struct!(IntentBatch {
    motion,
    gun_aim,
    sensor,
    weapons,
    devices,
    wing,
    activity,
    fallbacks,
    reason,
    fuel_state,
    launch_calls,
});

crate::checkpoint_struct!(ActiveManeuver {
    intent,
    submitted_at,
    formation,
    search,
    ordered,
});

// `gun_views`: per-tick scratch. The mission builds the views and hands them
// over with `set_gun_views` on every actor's step, immediately before it
// calls `Controller::step` (`ai/mission.rs`: the call and the step sit in one
// function and no path reaches the step without the call). The only reads are
// inside `step` (station choice and gun tracking), and a step that does not
// advance returns the last batch without reading them. So a restored
// controller starts with none and the next step writes them first.
//
// `formation_traffic`: per-tick scratch for the same reason. The mission
// calls `set_formation_observation` with the tick's traffic before the
// actor's step, and the only read is `formation_point` inside that step.
//
// `strike`: per-tick scratch. The mission hands a striker's order over with
// `set_strike` immediately before `Controller::step`, which takes it.
//
// `strike_handling`: per-tick scratch. The step sets it from the strike
// steer it flew and the mission takes it before flying the motion.
//
// `trace`: a why-record. `Controller::trace` hands it to the debug panels and
// `thought::Record` takes no part in equality.
crate::checkpoint_struct!(Controller {
    identity,
    profile,
    experience,
    random,
    service,
    gun_cycles,
    gun_phase,
    damage_recovery,
    mission_complete,
    gun_tracking,
    missile_retry_until,
    gun_tracking_since,
    variation,
    smooth_variation,
    variation_tick,
    formation_configuration,
    active,
    target,
    reason,
    recipient,
    next_choice_quarters,
    last_tick,
    last_batch,
    next_motion_id,
    next_request_id,
    fallbacks,
    pending_warnings,
    pursuit,
    formation_guidance,
    ordered_approach,
    search_contact,
    search_started_tick,
    search_orbit_altitude_ft,
    completed_search,
    wing_search,
    wing_route,
    defense_motion,
    defense_motion_id,
    mission_target,
    mission_rejoin,
    mission_search_bearing,
} skip {
    gun_views = Vec::new(),
    formation_traffic = std::sync::Arc::default(),
    strike = None,
    strike_handling = crate::ai::strike::Handling::PLAIN,
    trace = crate::ai::thought::Record::default(),
});

#[cfg(test)]
#[path = "controller_checkpoint_tests.rs"]
mod tests;
