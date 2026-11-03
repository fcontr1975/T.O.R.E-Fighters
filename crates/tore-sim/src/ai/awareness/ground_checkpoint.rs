//! The coders of a strike actor's ground picture (docs/formats/checkpoint.md).
//!
//! Every field of every stored type is coded: the lists, the memory, the
//! briefing status and the threat inputs are all read by later ticks (the
//! follow between rebuilds, the strike selector, the threat level).
//! `GroundSubject`, `GroundScene`, `Subjects`, `Refresh`, `RecentWarnings`,
//! `ThreatLevel` and `RetailAttention` are per-call values, never held.

use super::{
    BriefedKind, BriefedTarget, Channels, DefenseFacts, FighterThreat, GroundPicture, GroundTrack,
    KnownDefense, SeenGround, ThreatPicture, Unmasked,
};

crate::checkpoint_struct!(DefenseFacts {
    missile_reach_ft,
    rotor_reach_ft,
    gun_reach_ft,
    radar,
    working,
    ship,
    heading_deg,
});

crate::checkpoint_struct!(Channels {
    radar,
    infrared,
    visual,
});

crate::checkpoint_struct!(GroundTrack {
    id,
    side,
    class,
    position,
    velocity,
    distance_ft,
    channels,
    aircraft,
    emitting,
    destroyed,
});

crate::checkpoint_struct!(SeenGround {
    id,
    side,
    class,
    position,
    last_seen_tick,
    emitting,
    defense,
});

crate::checkpoint_enum!(BriefedKind {
    Target = 0,
    Defense = 1,
    Ordered = 2,
});

crate::checkpoint_struct!(BriefedTarget {
    id,
    kind,
    side,
    class,
    position,
    standing,
    perceived,
    defense,
});

crate::checkpoint_struct!(KnownDefense {
    id,
    side,
    position,
    facts,
    briefed,
});

crate::checkpoint_struct!(FighterThreat {
    id,
    side,
    position,
    distance_ft,
    off_nose_deg,
});

crate::checkpoint_struct!(ThreatPicture { defenses, fighters });

crate::checkpoint_struct!(Unmasked { id, tick });

crate::checkpoint_struct!(GroundPicture {
    role,
    objective,
    ordered,
    class_order,
    built,
    contacts,
    seen,
    briefed,
    threats,
    unmasked,
});
