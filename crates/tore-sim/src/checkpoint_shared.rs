//! The leaf types that more than one module's coder needs, coded once here so
//! stage H's slices do not code them twice: aircraft identity, sides, the AI's
//! activity and experience, store accounting, runway views, attitude bases,
//! a pilot's controls, and the types that already have an exact own-plane
//! coder.
//!
//! Every type here has public fields; types with private fields are coded in
//! their own module's `checkpoint` child (the random stream `DecisionRandom`
//! in `ai/checkpoint.rs`).

use super::{Checkpoint, CheckpointError, Loader, Saver, checkpoint_via_exact, invalid};
use crate::ai::airfield::{AirfieldAnchors, LandingOrder, LandingReason, Phase, RunwayView};
use crate::ai::controller::{Activity, FrameEvent, TargetView, ThreatReport};
use crate::ai::experience::{ExperienceOrigin, ResolvedExperience};
use crate::ai::gunnery;
use crate::ai::route::FuelState;
use crate::ai::targeting::Side;
use crate::ai::threat::{DispenserStore, SeekerClass};
use crate::ai::weapon_service::{Rounds, StationId, StoreCapability, StoreState};
use crate::ai::wing::{Formation, WingControl};
use crate::ai::{ScalarSpeed, SpeedLimits};
use crate::airport::ApproachEnd;
use crate::attitude::Basis;
use crate::sensors::passive::{Emitter, Symbol};
use tore_input::{
    FlightAxis, LiftCommand, NozzlePreset, PilotCommand, PilotInput, StabilityLevel, Switch,
    TrimAxis,
};

// The types with an exact own-plane coder already: one coder to keep
// complete, not two.
checkpoint_via_exact!(
    tore_formats::flight_model::clock_rng::NativeRng,
    tore_formats::flight_model::clock_rng::FixedClock,
    crate::turbulence::Turbulence,
    crate::cheats::Cheats,
    crate::cheats::Damage,
    crate::ai::Experience,
    crate::sensors::Controls,
    crate::sensors::Channel,
    crate::combat::live::DamageSection,
    crate::wreck::Wreck,
    crate::wreck::Phase,
    crate::wreck::Power,
    crate::ejection::Escape,
    crate::ejection::Phase,
);

crate::checkpoint_enum!(tore_formats::aircraft::AircraftId {
    F18 = 0,
    Rafale = 1,
    F14 = 2,
    A4E = 3,
    X31 = 4,
    Mig29 = 5,
    Su27 = 6,
    Mig21 = 7,
    Su25 = 8,
    Mig23 = 9,
    Su35 = 10,
    F22 = 11,
    F22n = 12,
    Faxx = 13,
    C130 = 14,
    Ac130 = 15,
    E3 = 16,
    Il76 = 17,
    E2 = 18,
    Av8 = 19,
    Yak141 = 20,
    V22 = 21,
    Ah64 = 22,
    Mi24 = 23,
    Ch47 = 24,
    Mig17 = 25,
    F4B = 26,
    F4J = 27,
    F4E = 28,
    F4G = 29,
    A7 = 30,
    F15 = 31,
    F16C = 32,
    F104 = 33,
    A10 = 34,
    B747 = 35,
    A310 = 36,
});

crate::checkpoint_tuple!(Side(side));
crate::checkpoint_tuple!(ScalarSpeed(speed));
crate::checkpoint_struct!(SpeedLimits {
    minimum,
    maximum,
    corner
});

crate::checkpoint_enum!(Activity {
    Idle = 0,
    Formation = 1,
    Searching = 2,
    Acquiring = 3,
    Pursuing = 4,
    Attacking = 5,
    Defending = 6,
    Evading = 7,
    Breaking = 8,
    Rejoining = 9,
    ReturningToBase = 10,
    Waiting = 11,
    Taxiing = 12,
    TakingOff = 13,
    HoldingMarshal = 14,
    Landing = 15,
    Landed = 16,
    OutOfFuel = 17,
    Destroyed = 18,
});

crate::checkpoint_enum!(SeekerClass {
    Infrared = 0,
    Radar = 1,
});

impl Checkpoint for ExperienceOrigin {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::ExplicitPerObject => s.writer().write_varint(0),
            Self::EditorAssignment { selected } => {
                s.writer().write_varint(1);
                selected.save(s, None)?;
            }
            Self::QuickMission { selected } => {
                s.writer().write_varint(2);
                selected.save(s, None)?;
            }
            Self::EnemyOverride => s.writer().write_varint(3),
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::ExplicitPerObject,
            1 => Self::EditorAssignment {
                selected: Checkpoint::load(l, None)?,
            },
            2 => Self::QuickMission {
                selected: Checkpoint::load(l, None)?,
            },
            3 => Self::EnemyOverride,
            other => return invalid(format!("ExperienceOrigin has no variant {other}")),
        })
    }
}

crate::checkpoint_struct!(ResolvedExperience { level, origin });

crate::checkpoint_enum!(ApproachEnd { Near = 0, Far = 1 });

crate::checkpoint_enum!(Phase {
    Waiting = 0,
    Taxi = 1,
    LineUp = 2,
    TakeoffRoll = 3,
    ClimbOut = 4,
    Inbound = 5,
    Marshal = 6,
    Approach = 7,
    Final = 8,
    Rollout = 9,
    TaxiClear = 10,
    Parked = 11,
    VerticalHold = 12,
    VerticalClimb = 13,
    Transition = 14,
    HoverApproach = 15,
    VerticalDescent = 16,
    Settle = 17,
    AirTaxi = 18,
});

crate::checkpoint_enum!(LandingReason {
    BugOut = 0,
    Ordered = 1,
    Fuel = 2,
    JoinLeader = 3,
    Damage = 4,
});

crate::checkpoint_struct!(AirfieldAnchors {
    taxi_out,
    takeoff_spot,
    takeoff_heading,
    landing_point,
    landing_heading,
    taxi_in,
    parking,
    parking_heading,
});

crate::checkpoint_struct!(RunwayView {
    airport,
    object,
    center,
    heading,
    length_ft,
    elevation_ft,
    anchors,
});

crate::checkpoint_struct!(LandingOrder { runway, reason });

crate::checkpoint_struct!(Basis { right, up, forward });

crate::checkpoint_enum!(FuelState {
    NoManagement = 0,
    OutOfFuel = 1,
    Critical = 2,
    Bingo = 3,
    Caution = 4,
    Ok = 5,
});

crate::checkpoint_tuple!(StationId(station));

impl Checkpoint for Rounds {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::Unlimited => s.writer().write_bool(false),
            Self::Finite(rounds) => {
                s.writer().write_bool(true);
                rounds.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(if l.reader().read_bool()? {
            Self::Finite(Checkpoint::load(l, None)?)
        } else {
            Self::Unlimited
        })
    }
}

crate::checkpoint_struct!(StoreState { inhibited, rounds });
crate::checkpoint_struct!(StoreCapability { air, surface });

crate::checkpoint_enum!(Formation {
    Echelon = 0,
    LineAbreast = 1,
    LineAstern = 2,
});

crate::checkpoint_enum!(WingControl {
    Loose = 1,
    Medium = 2,
    Tight = 3,
});

crate::checkpoint_struct!(DispenserStore { class, count });

crate::checkpoint_enum!(Symbol {
    Unknown = 0,
    Aircraft = 1,
    Ground = 2,
});

crate::checkpoint_struct!(Emitter {
    id,
    bearing_rad,
    distance_nmi,
    symbol,
    received,
});

crate::checkpoint_struct!(TargetView {
    id,
    side,
    position,
    heading_deg,
    pitch_deg,
    speed,
    maximum_speed,
    is_aircraft,
    is_fighter,
    human_controlled,
    infrared_strength,
    valid,
    type_allowed,
    seeker_eligible,
    wing_attackers,
    terrain_blocked,
    sensor_supported,
    link_track,
});

crate::checkpoint_struct!(ThreatReport {
    missile_id,
    seeker,
    launcher_id,
    launcher_same_side,
    distance_at_launch_ft,
    launch_tick,
});

impl Checkpoint for FrameEvent {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::ThreatReported(report) => {
                s.writer().write_varint(0);
                report.save(s, None)?;
            }
            Self::Hit => s.writer().write_varint(1),
            Self::TargetUnavailable(id) => {
                s.writer().write_varint(2);
                id.save(s, None)?;
            }
            Self::ActorRemoved(id) => {
                s.writer().write_varint(3);
                id.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::ThreatReported(Checkpoint::load(l, None)?),
            1 => Self::Hit,
            2 => Self::TargetUnavailable(Checkpoint::load(l, None)?),
            3 => Self::ActorRemoved(Checkpoint::load(l, None)?),
            other => return invalid(format!("FrameEvent has no variant {other}")),
        })
    }
}

/// The target an AI gunner tracks.
type GunTarget = gunnery::Target;
crate::checkpoint_struct!(GunTarget {
    id,
    position,
    velocity,
    basis
});

crate::checkpoint_enum!(crate::combat::missiles::Rules {
    Compatibility = 0,
    Spec = 1,
    Energy = 2,
});

// A pilot's controls, as an AI actor last flew them (`AiActor::last_input`,
// slice H9): a destroyed actor no longer steps, so its last controls are
// never rewritten and a restore must carry them.

crate::checkpoint_enum!(Switch {
    Gear = 0,
    Flaps = 1,
    Airbrake = 2,
    Hook = 3,
    Bay = 4,
    Engine = 5,
    Burner = 6,
    Radar = 7,
    Jammer = 8,
    Autopilot = 9,
    WaypointAutopilot = 10,
    HoverHold = 11,
});

crate::checkpoint_enum!(FlightAxis {
    VectorPitch = 0,
    VectorYaw = 1,
    Conversion = 2,
    Collective = 3,
});

crate::checkpoint_enum!(StabilityLevel {
    Off = 0,
    Damper = 1,
    Attitude = 2,
});

crate::checkpoint_enum!(TrimAxis {
    Pitch = 0,
    Roll = 1,
    Pedal = 2,
});

crate::checkpoint_enum!(NozzlePreset {
    Forward = 0,
    Vertical = 1,
});

impl Checkpoint for LiftCommand {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::SetStability(level) => {
                s.writer().write_varint(0);
                level.save(s, None)?;
            }
            Self::CycleStability => s.writer().write_varint(1),
            Self::TrimSet => s.writer().write_varint(2),
            Self::TrimAdjust(axis, amount) => {
                s.writer().write_varint(3);
                axis.save(s, None)?;
                amount.save(s, None)?;
            }
            Self::TrimCentre => s.writer().write_varint(4),
            Self::NozzleStep { down } => {
                s.writer().write_varint(5);
                down.save(s, None)?;
            }
            Self::NozzlePreset(preset) => {
                s.writer().write_varint(6);
                preset.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::SetStability(Checkpoint::load(l, None)?),
            1 => Self::CycleStability,
            2 => Self::TrimSet,
            3 => Self::TrimAdjust(Checkpoint::load(l, None)?, Checkpoint::load(l, None)?),
            4 => Self::TrimCentre,
            5 => Self::NozzleStep {
                down: Checkpoint::load(l, None)?,
            },
            6 => Self::NozzlePreset(Checkpoint::load(l, None)?),
            other => return invalid(format!("LiftCommand has no variant {other}")),
        })
    }
}

impl Checkpoint for PilotCommand {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::Eject => s.writer().write_varint(0),
            Self::Toggle(switch) => {
                s.writer().write_varint(1);
                switch.save(s, None)?;
            }
            Self::Set(switch, on) => {
                s.writer().write_varint(2);
                switch.save(s, None)?;
                on.save(s, None)?;
            }
            Self::Throttle(value) => {
                s.writer().write_varint(3);
                value.save(s, None)?;
            }
            Self::AdjustThrottle(value) => {
                s.writer().write_varint(4);
                value.save(s, None)?;
            }
            Self::SetAxis(axis, value) => {
                s.writer().write_varint(5);
                axis.save(s, None)?;
                value.save(s, None)?;
            }
            Self::AdjustAxis(axis, value) => {
                s.writer().write_varint(6);
                axis.save(s, None)?;
                value.save(s, None)?;
            }
            Self::NeutralVector => s.writer().write_varint(7),
            Self::Lift(command) => {
                s.writer().write_varint(8);
                command.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::Eject,
            1 => Self::Toggle(Checkpoint::load(l, None)?),
            2 => Self::Set(Checkpoint::load(l, None)?, Checkpoint::load(l, None)?),
            3 => Self::Throttle(Checkpoint::load(l, None)?),
            4 => Self::AdjustThrottle(Checkpoint::load(l, None)?),
            5 => Self::SetAxis(Checkpoint::load(l, None)?, Checkpoint::load(l, None)?),
            6 => Self::AdjustAxis(Checkpoint::load(l, None)?, Checkpoint::load(l, None)?),
            7 => Self::NeutralVector,
            8 => Self::Lift(Checkpoint::load(l, None)?),
            other => return invalid(format!("PilotCommand has no variant {other}")),
        })
    }
}

crate::checkpoint_struct!(PilotInput {
    pitch,
    roll,
    yaw,
    throttle_rate,
    throttle,
    vector_pitch_rate,
    vector_yaw_rate,
    conversion_rate,
    collective_rate,
    vector_pitch,
    vector_yaw,
    conversion,
    collective,
    commands,
});
