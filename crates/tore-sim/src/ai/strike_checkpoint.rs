//! The coders of a striker's strike state (docs/formats/checkpoint.md).
//!
//! Every field the next step reads is coded: the phase and its start, the
//! pass, the target and store, the pass count that seeds the next aim, the
//! drawn aim, the final's line, the release count, the exit's heading, a gun
//! run's burst state and the height the striker was tasked at, and (T.O.R.E,
//! John, 2026-10-28) the release estimate, the committed-to-release window
//! and whether the exit or turn back follows an early retarget; and (T.O.R.E,
//! John, 2026-10-29) the pass's stand-off release plan, whether it was given
//! up, and when its last stand-off round lands; and (T.O.R.E, John,
//! 2026-10-29) a helicopter's move-ahead ambush plan.
//! `inputs` is per-step scratch: the host hands it over before every mission
//! step and the step takes it, so between steps it is always empty and a
//! restored strike starts without it. `Context`, `Order`, `Steer`, `Handling`,
//! `Selection`, `Inputs`, `LaseInputs`, `WingFacts`, `Store`, `Solution`,
//! `PassInputs`, `PassChoice`, `TargetFacts` and `AimDraw` are per-call
//! values, never held. The lase in progress, and whether the aircraft has a
//! designator and laser rounds (slice ST-L), are coded.

use super::{
    Lase, LaseEnd, NoLaze, NoLazeCall, Pass, Phase, ReleasePlan, Segment, StoreKind, Strike,
};
use crate::ai::awareness::ground::ThreatLevel;
use crate::ai::helicopter_ahead::Ahead;
use crate::ai::helicopter_threat::{Call, Decision, HeliThreat, Ring};

crate::checkpoint_enum!(Pass {
    Straight = 0,
    PopUp = 1,
    FastHigh = 2,
    Dive = 3,
    Level = 4,
    Orbit = 5,
    StandOff = 6,
    // ST-T2's passes follow ST-F's (same-build bytes; merged at ST-INT).
    Laydown = 7,
    Strafe = 8,
});

crate::checkpoint_enum!(StoreKind {
    Bomb = 0,
    Rocket = 1,
    Laser = 2,
    Coordinate = 3,
    Command = 4,
    Seeker = 5,
    RadarAgm = 6,
    AntiRadiation = 7,
    Gun = 8,
});

crate::checkpoint_enum!(Phase {
    Ingress = 0,
    Regain = 1,
    Attack = 2,
    Exit = 3,
    TurnBack = 4,
    Wait = 5,
    Defend = 6,
    Home = 7,
    Lase = 8,
    HoldOff = 9,
});

crate::checkpoint_enum!(LaseEnd {
    Timed = 0,
    Impact = 1,
    Dies = 2,
});

crate::checkpoint_enum!(NoLaze {
    Manual = 0,
    Rtb = 1,
});

crate::checkpoint_struct!(NoLazeCall {
    kind,
    station,
    target,
    told,
});

crate::checkpoint_struct!(Lase {
    target,
    end,
    ticks,
    since,
    side,
    hold_ft,
    own,
    wing,
    guided,
});

crate::checkpoint_enum!(Segment {
    Approach = 0,
    Pull = 1,
    Final = 2,
    Climb = 3,
    Guide = 4,
});

crate::checkpoint_enum!(Decision {
    HoldOff = 0,
    Resume = 1,
    Retarget = 2,
    Home = 3,
    Leave = 4,
});

crate::checkpoint_struct!(Ring {
    id,
    centre,
    radius_ft,
});

crate::checkpoint_struct!(Call {
    decision,
    tick,
    target,
    ring,
    told,
});

crate::checkpoint_struct!(HeliThreat {
    hold,
    announced,
    waited,
    ambush,
    approach,
    standoff_ft,
    ahead,
    call,
});

crate::checkpoint_struct!(Ahead {
    point,
    orbit,
    aim,
    range_ft,
    offset_deg,
    fire_tick,
    planned_tick,
});

crate::checkpoint_struct!(ReleasePlan {
    range_ft,
    seconds,
    advance_ft,
});

crate::checkpoint_enum!(ThreatLevel {
    Clear = 0,
    Air = 1,
    Guns = 2,
    Missiles = 3,
});

crate::checkpoint_struct!(Strike {
    seed,
    phase,
    segment,
    since,
    pass,
    forced,
    forced_station,
    target,
    station,
    kind,
    pass_number,
    aim_ft,
    lead_ft,
    gate_ft,
    inside,
    holding,
    hold_ft,
    released,
    next_release,
    exit_heading_deg,
    jink_side,
    cruise_ft,
    warned,
    threat,
    withdrawn,
    takeover,
    lase,
    designator,
    laser_rounds,
    laser_set,
    no_laze,
    burst,
    burst_open,
    recovery_until,
    cue_across_sum,
    cue_last,
    tasked_agl_ft,
    heli,
    release_cue,
    release_in_s,
    committed,
    retargeted,
    release_plan,
    plan_dropped,
    standoff_until,
} skip {
    inputs = None,
});
