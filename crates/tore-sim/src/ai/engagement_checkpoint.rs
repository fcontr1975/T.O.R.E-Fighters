//! The coders of an AI aircraft's mission assignment and rules-of-engagement
//! state, and of the attack reports it keeps (docs/formats/checkpoint.md,
//! stage H slice H4).
//!
//! `GroupObjective` is resolved before the mission starts, and `Priority`,
//! `Selection`, `Explanation` and `ProtectedView` are per-call results; none
//! is held between ticks (the one journal-only `Selection` lives in the
//! why-record `JournalMemory`).

use super::{
    Assignment, HostileEscort, PatrolRegion, Policy, Role, Stance, StrikeObjective, StrikeRole,
    ThreatReport,
};

crate::checkpoint_enum!(StrikeRole {
    Cap = 0,
    Strike = 1,
});

crate::checkpoint_struct!(StrikeObjective {
    targets,
    defenses,
    point,
});

crate::checkpoint_enum!(Role {
    FreeEngagement = 0,
    CombatAirPatrol = 1,
    Intercept = 2,
    Escort = 3,
    Disengage = 4,
});

crate::checkpoint_enum!(Stance {
    WeaponsHold = 0,
    SelfDefense = 1,
    ProtectAssigned = 2,
    EngageAssigned = 3,
});

crate::checkpoint_struct!(HostileEscort {
    principal_id,
    escort_id,
});

crate::checkpoint_struct!(PatrolRegion {
    clip,
    center_ft,
    radius_ft,
});

crate::checkpoint_struct!(Assignment {
    role,
    stance,
    protected_ids,
    destroy_ids,
    hostile_escorts,
    patrol,
});

crate::checkpoint_struct!(Policy {
    escort_outside_leash
});

crate::checkpoint_struct!(ThreatReport {
    attacker_id,
    defended_id,
});
