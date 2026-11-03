//! The coders of combat's state: ownships, targets, projectiles, effects, the
//! ledger, the rewind history, the random stream and the combat tick
//! (docs/formats/checkpoint.md, section 2). `live::State` is coded as a value;
//! the world's `Combat` wrapper restores it in place around its own setup.
//!
//! Every field of every type here is named, so a field added without coding
//! it fails to compile. What is skipped, with its class:
//!
//! - `State::device_log` and `State::decoy_log`: why-records. They explain the
//!   releases and decoy rolls for a recording, the host drains them, and
//!   nothing in combat reads them back.
//!
//! Everything else is coded, including the fields that look like caches or
//! presentation but a later tick reads: an ownship's `range_estimate` and
//! `mounted_key` refresh only on a change, `sound_events` and `strikes` wait
//! for the host to drain them, and the hit records and the `ownship_rows` are
//! shown before the next step.
//!
//! *Agent decision:* the ground contact volumes (`ground_bounds`) are coded
//! too. The world adds them when it builds the mission, so a restore over the
//! same mission would rebuild them, but `State` is decoded as a value with no
//! fresh state beside it, and they are a few boxes per airport.
//!
//! Weapon records, configurations and signature profiles are shared records
//! (equal copies cost one coding), as the records slice settled.

use super::super::ballistics::BombMotion;
use super::{
    ActorSupport, AiCockpit, DecoyRoll, DeviceNote, Effect, EffectKind, FriendlyFire, GunCadence,
    HitRecord, Kept, LocalizedDamage, Ownship, Projectile, RangeEstimate, Readiness, State, Strike,
    StrikeCockpit, StrikeTarget, SystemFaults, Target,
};
use crate::acoustics::{Emission, Kind as SoundKind};
use crate::airport::OrientedBox;
use crate::checkpoint::{Checkpoint, CheckpointError, Loader, Saver, invalid};
use crate::sensors;
use std::collections::VecDeque;

crate::checkpoint_enum!(Readiness {
    Ready = 0,
    Safe = 1,
    BayClosed = 2,
    LauncherLost = 3,
    StationFailed = 4,
    Empty = 5,
    Capacity = 6,
    NoTarget = 7,
    TargetDestroyed = 8,
    WrongTarget = 9,
    NoRadar = 10,
    RadarOff = 11,
    RadarFailed = 12,
    RadarCoverage = 13,
    RadarSearchOnly = 14,
    RadarAcquiring = 15,
    MinimumRange = 16,
    MaximumRange = 17,
    Altitude = 18,
    FieldOfView = 19,
    GunArc = 20,
    GunSlewing = 21,
    GroupEmpty = 22,
    GunObscured = 23,
    TerrainMask = 24,
    NoEmitter = 25,
    TrackLost = 26,
});

// The AC-130's player-directed gun mounts and gunsight: the actual slewed
// angles, the linked membership, the sight's mode, look angles and aim point,
// the seat's held sight controls, a pending Backslash, the last notice and
// each gun's readiness are stepped at the fixed tick, so a restore carries
// them.
type GunshipState = crate::combat::gunship::State;
crate::checkpoint_struct!(GunshipState {
    stations,
    included,
    headings,
    elevations,
    sight,
    look,
    returning,
    aim,
    impacts,
    impacts_tick,
    status,
    input,
    slew_held,
    request,
    notice,
    gun_lock,
});
type GunLock = crate::combat::gunship::GunLock;
crate::checkpoint_struct!(GunLock { object, point });

type Sight = crate::combat::gunship::Sight;
impl Checkpoint for Sight {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::Free => s.writer().write_varint(0),
            Self::Pinned(point) => {
                s.writer().write_varint(1);
                point.save(s, None)?;
            }
            Self::Tracked(id) => {
                s.writer().write_varint(2);
                id.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::Free,
            1 => Self::Pinned(Checkpoint::load(l, None)?),
            2 => Self::Tracked(Checkpoint::load(l, None)?),
            other => return invalid(format!("a gunsight has no mode {other}")),
        })
    }
}
type Impact = crate::combat::gunship_impact::Impact;
impl Checkpoint for Impact {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let (kind, point, seconds, range_ft) = match *self {
            Self::Ground {
                point,
                seconds,
                range_ft,
            } => (0, point, seconds, range_ft),
            Self::Air {
                point,
                seconds,
                range_ft,
            } => (1, point, seconds, range_ft),
            Self::Spent {
                point,
                seconds,
                range_ft,
            } => (2, point, seconds, range_ft),
        };
        s.writer().write_varint(kind);
        point.save(s, None)?;
        seconds.save(s, None)?;
        range_ft.save(s, None)
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        let kind = l.reader().read_varint()?;
        let point = Checkpoint::load(l, None)?;
        let seconds = Checkpoint::load(l, None)?;
        let range_ft = Checkpoint::load(l, None)?;
        Ok(match kind {
            0 => Self::Ground {
                point,
                seconds,
                range_ft,
            },
            1 => Self::Air {
                point,
                seconds,
                range_ft,
            },
            2 => Self::Spent {
                point,
                seconds,
                range_ft,
            },
            other => return invalid(format!("a gun impact has no kind {other}")),
        })
    }
}
type SightInput = crate::combat::gunship::SightInput;
crate::checkpoint_struct!(SightInput { deflection, zoom });
crate::checkpoint_enum!(crate::combat::gunship::SightRequest {
    Designate = 0,
    Pin = 1,
    Next = 2,
});
crate::checkpoint_enum!(crate::combat::gunship::Notice {
    NoGroundPoint = 0,
    DropToSlew = 1,
    GimbalLimit = 2,
});
type SightNotice = crate::combat::gunship::SightNotice;
crate::checkpoint_struct!(SightNotice { notice, tick });

crate::checkpoint_enum!(FriendlyFire { On = 0, Off = 1 });

crate::checkpoint_enum!(EffectKind {
    Flare = 0,
    Chaff = 1,
    Launch = 2,
    Hit = 3,
    Destroyed = 4,
    Ground = 5,
    DebrisImpact = 6,
    Flak = 7,
});

// A surface unit's round in flight: its end tick (a flak shell's time fuze)
// and whether it is flak; and how a ground object with a unit record
// explodes. Both are read by later ticks.
type SurfaceRound = super::SurfaceRound;
crate::checkpoint_struct!(SurfaceRound { end_tick, flak });

crate::checkpoint_struct!(BombMotion {
    origin,
    horizontal,
    vertical,
    body,
    tick,
    flight,
});
type BombBody = super::super::bomb_flight::Body;
crate::checkpoint_struct!(BombBody {
    mass,
    area,
    retarder,
    steering,
    guidance_seconds
});
type BombFlight = super::super::bomb_flight::Flight;
crate::checkpoint_struct!(BombFlight { position, velocity });
type GroundLook = super::GroundLook;
crate::checkpoint_struct!(GroundLook {
    explosion,
    crater,
    burns,
    lasting,
    pavement
});

// The type facts the ground pages read of a ground object.
type GroundListing = super::GroundListing;
crate::checkpoint_struct!(GroundListing {
    flags,
    max_visible,
    team,
});

type ParkedSite = super::ParkedSite;
crate::checkpoint_struct!(ParkedSite {
    ground,
    water,
    aircraft
});

crate::checkpoint_struct!(HitRecord {
    tick,
    target,
    station,
    class,
    nominal,
    applied,
    hp_after,
});

crate::checkpoint_struct!(Strike {
    owner,
    victim,
    weapon_flags,
    destroyed,
    amount,
});

crate::checkpoint_struct!(Effect {
    position,
    kind,
    ticks,
    blast,
    scale,
});

crate::checkpoint_struct!(ActorSupport {
    owner,
    observation,
    supported,
    radar_position,
    radar_emitting,
});

// The deployed fractions of gear, flaps and bay and the external stores. A
// target row holds them between ticks, so they are state.
type Deployed = sensors::Configuration;
crate::checkpoint_struct!(Deployed {
    gear,
    flaps,
    bay,
    stores,
});
type Carried = sensors::signature::Stores;
crate::checkpoint_struct!(Carried { count, lbs });

crate::checkpoint_struct!(OrientedBox {
    center,
    half,
    heading,
    pitch,
    bank,
});

impl Checkpoint for SoundKind {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match self {
            Self::Impact => s.writer().write_varint(0),
            Self::Explosion => s.writer().write_varint(1),
            Self::Blast(kind) => {
                s.writer().write_varint(2);
                kind.save(s, None)?;
            }
            Self::AircraftPass => s.writer().write_varint(3),
            Self::MissilePass => s.writer().write_varint(4),
            Self::SonicBoom => s.writer().write_varint(5),
            Self::Chaff => s.writer().write_varint(6),
            Self::Flare => s.writer().write_varint(7),
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::Impact,
            1 => Self::Explosion,
            2 => Self::Blast(Checkpoint::load(l, None)?),
            3 => Self::AircraftPass,
            4 => Self::MissilePass,
            5 => Self::SonicBoom,
            6 => Self::Chaff,
            7 => Self::Flare,
            other => return invalid(format!("a sound has no kind {other}")),
        })
    }
}

crate::checkpoint_struct!(Emission {
    kind,
    position,
    arrived,
    own,
});

crate::checkpoint_struct!(LocalizedDamage {
    amounts,
    structural_variant,
    structural_section,
});

// `table` and `afterburner_available` are copied from the aircraft's
// configuration when the row is made; a row is decoded as a value, so they
// are coded with the counts.
crate::checkpoint_struct!(SystemFaults {
    table,
    afterburner_available,
    damage,
    counts,
});

// The signature profile is an imported record copied into every row of the
// same aircraft type: a shared record.
crate::checkpoint_struct!(Target {
    aircraft,
    role,
    heat,
    radar_emitting,
    id,
    position,
    velocity,
    basis,
    configuration,
    jammer,
    jammer_active,
    airborne,
    on_ground,
    wreck,
    wreck_power,
    radius,
    hp,
    initial_hp,
    fragment_offsets,
    fragment_released,
    localized_damage,
    faults,
    category,
    side,
} shared { signature });

impl Checkpoint for Projectile {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let Projectile {
            id,
            owner,
            weapon,
            guidance,
            motion,
            guidance_ticks,
            age,
            incoming,
            station,
            position,
            previous,
            direction,
            speed_f8,
            launched_t,
            target,
            fall,
            gun_round,
            tracer,
        } = self;
        id.save(s, None)?;
        owner.save(s, None)?;
        // An AI actor's round carries its weapon record: a shared record, so
        // the rounds of one burst cost one coding. A player's round has none
        // and reads its owner's station.
        s.writer().write_bool(weapon.is_some());
        if let Some(weapon) = weapon {
            s.shared(weapon)?;
        }
        guidance.save(s, None)?;
        motion.save(s, None)?;
        guidance_ticks.save(s, None)?;
        age.save(s, None)?;
        incoming.save(s, None)?;
        station.save(s, None)?;
        position.save(s, None)?;
        previous.save(s, None)?;
        direction.save(s, None)?;
        speed_f8.save(s, None)?;
        launched_t.save(s, None)?;
        target.save(s, None)?;
        fall.save(s, None)?;
        gun_round.save(s, None)?;
        tracer.save(s, None)?;
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        let id = Checkpoint::load(l, None)?;
        let owner = Checkpoint::load(l, None)?;
        let weapon = if l.reader().read_bool()? {
            Some(l.shared()?)
        } else {
            None
        };
        Ok(Projectile {
            id,
            owner,
            weapon,
            guidance: Checkpoint::load(l, None)?,
            motion: Checkpoint::load(l, None)?,
            guidance_ticks: Checkpoint::load(l, None)?,
            age: Checkpoint::load(l, None)?,
            incoming: Checkpoint::load(l, None)?,
            station: Checkpoint::load(l, None)?,
            position: Checkpoint::load(l, None)?,
            previous: Checkpoint::load(l, None)?,
            direction: Checkpoint::load(l, None)?,
            speed_f8: Checkpoint::load(l, None)?,
            launched_t: Checkpoint::load(l, None)?,
            target: Checkpoint::load(l, None)?,
            fall: Checkpoint::load(l, None)?,
            gun_round: Checkpoint::load(l, None)?,
            tracer: Checkpoint::load(l, None)?,
        })
    }
}

crate::checkpoint_struct!(RangeEstimate {
    station,
    target,
    mode,
    minimum,
    maximum,
    favorable,
});

crate::checkpoint_struct!(GunCadence {
    pending,
    next_scaled,
    ordinal,
});

// What a human left set in a cockpit it gave back to the AI (slice FX-J):
// the sight and the laser step on while the AI flies, so a restore carries
// them all.
// The strike cockpit (slice ST-C): one record for a human's, a kept and an
// AI striker's cockpit.
impl Checkpoint for StrikeTarget {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        match *self {
            Self::Object(id) => {
                s.writer().write_varint(0);
                id.save(s, None)?;
            }
            Self::Point(point) => {
                s.writer().write_varint(1);
                point.save(s, None)?;
            }
        }
        Ok(())
    }
    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        Ok(match l.reader().read_varint()? {
            0 => Self::Object(Checkpoint::load(l, None)?),
            1 => Self::Point(Checkpoint::load(l, None)?),
            other => return invalid(format!("a strike target has no kind {other}")),
        })
    }
}

crate::checkpoint_struct!(StrikeCockpit {
    target,
    selected,
    armed,
    launch_mode,
    delivery_mode,
    sight,
    laser,
});

crate::checkpoint_struct!(AiCockpit { side, cockpit });

crate::checkpoint_struct!(Kept {
    side,
    cockpit,
    ever_loaded,
    gunship,
    picks,
    hud_selection,
    sight_hold,
    shots,
    hits,
    kills,
    radar,
    airport,
});

// The configuration is a shared record, and so is the sensors' profile set
// inside it: both ownship fields that carry the same copy resolve to one
// coding.
crate::checkpoint_struct!(Ownship {
    aircraft,
    side,
    external,
    release_readiness,
    cockpit,
    mounted,
    heat,
    energy_hit,
    bore_observation,
    mounted_key,
    range_estimate,
    ammo,
    ever_loaded,
    gunship,
    sensors,
    hud_selection,
    sight_hold,
    friendlies,
    painted_by,
    spotted_teams,
    harm_cue,
    emitters,
    missile_threats,
    hp,
    damage,
    subsystem_counts,
    last_subsystem,
    radar_failed,
    visual_failed,
    infrared_failed,
    rwr_failed,
    ecm_failed,
    chaff,
    flares,
    localized_damage,
    fragment_released,
    explosion_reported,
    shots,
    hits,
    kills,
    pending_damage,
    previous_position,
    triggers,
    gun_cadence,
    bay_release,
    bay_hold_until,
} shared { config } skip { delivery = None, guided_sight = None, gun_key = false });

impl Checkpoint for State {
    fn save(&self, s: &mut Saver, _: Option<&Self>) -> Result<(), CheckpointError> {
        let State {
            weapon_rules,
            ownships,
            projectiles,
            ended_missile_threats,
            targets,
            ownship_rows,
            actor_support,
            ground_bounds,
            effects,
            marks,
            crashed,
            marks_made,
            blast_rolls,
            sound_events,
            smoke,
            devices,
            energy_devices,
            energy_seed,
            energy_predictions,
            energy_predictions_enabled,
            measure_prediction_cost: _,
            prediction_cpu_clock: _,
            prediction_cpu_cost_ns: _,
            prediction_cost_ns: _,
            prediction_request_ns: _,
            prediction_request_cpu_ns: _,
            prediction_gate_samples: _,
            prediction_player_samples: _,
            // Why-record: the releases for a recording, drained by the host.
            device_log: _,
            // Why-record: the decoy rolls for a recording, drained by the host.
            decoy_log: _,
            // Why-record: this step's ground bursts, drained by the host.
            ground_bursts: _,
            debris,
            ledger,
            target_jammer,
            rng,
            history,
            strikes,
            range_category,
            next_target_id,
            next_shot,
            tick,
            service_remainder,
            cheats,
            friendly_fire,
            volumes,
            rewinds,
            surface_rounds,
            next_surface_shot,
            bombs,
            jettisoned,
            // Transient: rebuilt from `bombs` and `guided` with identical results.
            falls: _,
            guided,
            guided_impacts,
            kept,
            ai_cockpits,
            ai_laser_armed,
            ai_laser_on,
            ground_looks,
            parked,
            ground_listing,
            airfield,
            sensor_weather,
            // Host setup, not state: the host sets it from its terrain.
            ground_ceiling: _,
        } = self;
        weapon_rules.save(s, None)?;
        ownships.save(s, None)?;
        projectiles.save(s, None)?;
        ended_missile_threats.save(s, None)?;
        targets.save(s, None)?;
        ownship_rows.save(s, None)?;
        actor_support.save(s, None)?;
        ground_bounds.save(s, None)?;
        effects.save(s, None)?;
        marks.save(s, None)?;
        crashed.save(s, None)?;
        marks_made.save(s, None)?;
        blast_rolls.save(s, None)?;
        sound_events.save(s, None)?;
        smoke.save(s, None)?;
        devices.save(s, None)?;
        energy_devices.save(s, None)?;
        energy_seed.save(s, None)?;
        energy_predictions.save(s, None)?;
        energy_predictions_enabled.save(s, None)?;
        debris.save(s, None)?;
        ledger.save(s, None)?;
        target_jammer.save(s, None)?;
        rng.save(s, None)?;
        history.save(s, None)?;
        strikes.save(s, None)?;
        range_category.save(s, None)?;
        next_target_id.save(s, None)?;
        next_shot.save(s, None)?;
        tick.save(s, None)?;
        service_remainder.save(s, None)?;
        cheats.save(s, None)?;
        friendly_fire.save(s, None)?;
        volumes.save(s, None)?;
        rewinds.save(s, None)?;
        surface_rounds.save(s, None)?;
        next_surface_shot.save(s, None)?;
        bombs.save(s, None)?;
        jettisoned.save(s, None)?;
        guided.save(s, None)?;
        guided_impacts.save(s, None)?;
        kept.save(s, None)?;
        ai_cockpits.save(s, None)?;
        ai_laser_armed.save(s, None)?;
        // The AI's own laser want in a cockpit a human left (ST-INT).
        ai_laser_on.save(s, None)?;
        ground_looks.save(s, None)?;
        parked.save(s, None)?;
        ground_listing.save(s, None)?;
        airfield.save(s, None)?;
        sensor_weather.save(s, None)?;
        Ok(())
    }

    fn load(l: &mut Loader<'_>, _: Option<&Self>) -> Result<Self, CheckpointError> {
        let weapon_rules = Checkpoint::load(l, None)?;
        let ownships: Vec<Ownship> = Checkpoint::load(l, None)?;
        // Ownships are found by bisection on their aircraft: they must come
        // back strictly ascending, as `add_ownship` keeps them.
        if ownships
            .windows(2)
            .any(|pair| pair[0].aircraft >= pair[1].aircraft)
        {
            return invalid("ownships out of aircraft order");
        }
        Ok(State {
            weapon_rules,
            ownships,
            projectiles: Checkpoint::load(l, None)?,
            ended_missile_threats: Checkpoint::load(l, None)?,
            targets: Checkpoint::load(l, None)?,
            ownship_rows: Checkpoint::load(l, None)?,
            actor_support: Checkpoint::load(l, None)?,
            ground_bounds: Checkpoint::load(l, None)?,
            effects: Checkpoint::load(l, None)?,
            marks: Checkpoint::load(l, None)?,
            crashed: Checkpoint::load(l, None)?,
            marks_made: Checkpoint::load(l, None)?,
            blast_rolls: Checkpoint::load(l, None)?,
            sound_events: Checkpoint::load(l, None)?,
            smoke: Checkpoint::load(l, None)?,
            devices: Checkpoint::load(l, None)?,
            energy_devices: Checkpoint::load(l, None)?,
            energy_seed: Checkpoint::load(l, None)?,
            energy_predictions: Checkpoint::load(l, None)?,
            energy_predictions_enabled: Checkpoint::load(l, None)?,
            measure_prediction_cost: false,
            prediction_cpu_clock: None,
            prediction_cpu_cost_ns: None,
            prediction_cost_ns: 0,
            prediction_request_ns: 0,
            prediction_request_cpu_ns: 0,
            prediction_gate_samples: [0; 3],
            prediction_player_samples: [0; 3],
            device_log: VecDeque::<DeviceNote>::new(),
            decoy_log: VecDeque::<DecoyRoll>::new(),
            ground_bursts: Vec::new(),
            debris: Checkpoint::load(l, None)?,
            ledger: Checkpoint::load(l, None)?,
            target_jammer: Checkpoint::load(l, None)?,
            rng: Checkpoint::load(l, None)?,
            history: Checkpoint::load(l, None)?,
            strikes: Checkpoint::load(l, None)?,
            range_category: Checkpoint::load(l, None)?,
            next_target_id: Checkpoint::load(l, None)?,
            next_shot: Checkpoint::load(l, None)?,
            tick: Checkpoint::load(l, None)?,
            service_remainder: Checkpoint::load(l, None)?,
            cheats: Checkpoint::load(l, None)?,
            friendly_fire: Checkpoint::load(l, None)?,
            volumes: Checkpoint::load(l, None)?,
            rewinds: Checkpoint::load(l, None)?,
            surface_rounds: Checkpoint::load(l, None)?,
            next_surface_shot: Checkpoint::load(l, None)?,
            bombs: Checkpoint::load(l, None)?,
            jettisoned: Checkpoint::load(l, None)?,
            falls: Default::default(),
            guided: Checkpoint::load(l, None)?,
            guided_impacts: Checkpoint::load(l, None)?,
            kept: Checkpoint::load(l, None)?,
            ai_cockpits: Checkpoint::load(l, None)?,
            ai_laser_armed: Checkpoint::load(l, None)?,
            ai_laser_on: Checkpoint::load(l, None)?,
            ground_looks: Checkpoint::load(l, None)?,
            parked: Checkpoint::load(l, None)?,
            ground_listing: Checkpoint::load(l, None)?,
            airfield: Checkpoint::load(l, None)?,
            sensor_weather: Checkpoint::load(l, None)?,
            ground_ceiling: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, target};
    use super::super::{EffectKind, Launcher, OwnshipInput, Projectile, Side, State, Strike};
    use super::*;
    use crate::attitude::Basis;
    use crate::checkpoint::{Models, from_bytes, round_trip, to_bytes};
    use tore_formats::aircraft::AircraftId;

    /// The types of the effects slice (H3b). Until that slice is merged their
    /// coders report themselves not covered, so a whole `State` cannot be coded
    /// yet; once it is, `whole` returns the copy and the tests below compare it.
    const EFFECTS_SLICE: [&str; 5] = [
        "combat::smoke::Smoke",
        "combat::countermeasures::Devices",
        "combat::debris::Piece",
        "combat::blast::Mark",
        "combat::blast::Rolls",
    ];

    #[test]
    fn jettison_checkpoint_keeps_the_inert_path() {
        let mut state = super::super::tests::fixture(true);
        state.command(0, super::super::Command::Jettison, shooter());
        assert!(!state.jettisoned.is_empty());
        let mut copy: State = round_trip(&state, &Models::default()).unwrap();
        assert_eq!(copy.jettisoned, state.jettisoned);
        for _ in 0..1200 {
            step(&mut state, false, 0);
            step(&mut copy, false, 0);
        }
        assert_eq!(text(&copy), text(&state));
    }

    /// A checkpoint taken while jettisoned stores fall leaves their cursors
    /// behind (optimization slice S1); the restored copy rebuilds them and
    /// flies on bit for bit with the uninterrupted run, and both stay on the
    /// stored motion flown from its release.
    #[test]
    fn jettisoned_stores_fall_on_identically_after_a_mid_fall_checkpoint() {
        let mut state = super::super::tests::fixture(true);
        state.command(0, super::super::Command::Jettison, shooter());
        for _ in 0..240 {
            step(&mut state, false, 0);
        }
        let falling: Vec<u32> = state
            .projectiles
            .iter()
            .filter(|p| state.jettisoned.contains_key(&p.id))
            .map(|p| p.id)
            .collect();
        assert!(!falling.is_empty(), "stores still falling");
        assert!(!state.falls.is_empty());
        let mut copy: State = round_trip(&state, &Models::default()).unwrap();
        assert!(copy.falls.is_empty(), "the cursors are not checkpointed");
        let mut landed = false;
        for _ in 0..1200 {
            step(&mut state, false, 0);
            step(&mut copy, false, 0);
            assert_eq!(text(&copy.projectiles), text(&state.projectiles));
            for p in state.projectiles.iter().filter(|p| falling.contains(&p.id)) {
                let motion = state.bomb_motion(p.id).unwrap();
                let seconds = p.age as f64 / 120.;
                assert_eq!(
                    p.position.map(f64::to_bits),
                    motion.position(seconds).map(f64::to_bits)
                );
                assert_eq!(
                    p.direction.map(f64::to_bits),
                    motion.direction(seconds).map(f64::to_bits)
                );
            }
            landed |= !state.projectiles.iter().any(|p| falling.contains(&p.id));
        }
        assert!(landed, "the stores reached the ground");
        assert!(state.falls.is_empty() && copy.falls.is_empty());
        assert_eq!(text(&copy), text(&state));
    }

    fn shooter() -> Launcher {
        Launcher {
            position: [0., 1000., 0.],
            basis: Basis::new(0., 0., 0.),
            speed_fps: 300.,
            velocity: [0., 0., 300.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: crate::sensors::Controls::default(),
        }
    }

    fn step(state: &mut State, held: bool, rewind: u16) {
        state.step_rewound(
            &[OwnshipInput {
                aircraft: 0,
                held,
                launcher: shooter(),
            }],
            &[(0, rewind)],
            |_, _| 0.,
            |_, _| false,
        );
    }

    /// A gun state: ownship 0 fires a burst at a row crossing ahead, with a
    /// rewind, so rounds are in flight with their rewinds recorded.
    fn gun_fight() -> State {
        let mut state = fixture(false);
        state.own_mut().config.stations[0].weapon.source = AircraftId::F18.gun().unwrap().into();
        let mut row = target(7, [-200., 1000., 1500.], 1000, 0);
        row.velocity = [260., 0., 0.];
        row.radius = 60.;
        row.side = Side(2);
        state.targets.push(row);
        for n in 0..70 {
            step(&mut state, (40..70).contains(&n), 31);
        }
        state
    }

    /// A missile state: ownship 0 has an AIM-120 in flight at a target ahead.
    fn missile_fight() -> State {
        use crate::combat::missiles::{Flight, LaunchMode, Motion, Profile};
        let mut state = fixture(true);
        {
            let weapon = &mut state.own_mut().config.stations[0].weapon;
            weapon.source = "AIM120.JT".into();
            weapon.flags = 0x41;
            weapon.seeker.signature = 3;
        }
        let weapon = state.own().configuration().stations[0].weapon.clone();
        let profile = Profile::for_weapon(&weapon).unwrap();
        let mut row = target(1, [0., 1000., 9000.], 1000, 0x80);
        row.velocity = [0., 0., 200.];
        state.targets.push(row);
        state.projectiles.push(Projectile {
            id: 0,
            owner: 0,
            weapon: None,
            guidance: Some(Flight::new(
                profile,
                LaunchMode::Cued,
                Some(1),
                [0., 1000., 0.],
            )),
            motion: Some(Motion::new(&weapon.movement, [0., 0., 600.], 1000.)),
            guidance_ticks: Some(profile.guidance_ticks),
            age: 0,
            incoming: None,
            station: 0,
            position: [0., 1000., 0.],
            previous: [0., 1000., 0.],
            direction: [0., 0., 1.],
            speed_f8: 600 * 256,
            launched_t: 0,
            target: Some(1),
            fall: Default::default(),
            gun_round: None,
            tracer: false,
        });
        for _ in 0..50 {
            step(&mut state, false, 0);
        }
        state
    }

    fn text<T: std::fmt::Debug>(value: &T) -> String {
        format!("{value:?}")
    }

    /// Codes `state`, or says the effects slice has not merged yet.
    fn whole(state: &State) -> Option<State> {
        let models = Models::default();
        match to_bytes(state, &models) {
            Ok(coded) => {
                println!(
                    "whole state: {} bytes and {} shared records",
                    coded.body.len(),
                    coded.records.len()
                );
                let copy: State = from_bytes(&coded, &models).unwrap();
                assert_eq!(
                    to_bytes(&copy, &models).unwrap(),
                    coded,
                    "a restored state codes differently"
                );
                Some(copy)
            }
            Err(CheckpointError::NotCovered(what)) if EFFECTS_SLICE.contains(&what) => None,
            Err(error) => panic!("the state did not code: {error}"),
        }
    }

    /// The state as a checkpoint carries it: the why-records drained.
    fn drained(mut state: State) -> State {
        state.take_device_notes();
        state.take_decoy_rolls();
        state.ledger.take_outcomes();
        state
    }

    #[test]
    fn rounds_in_flight_round_trip_with_their_rewinds_and_history() {
        let models = Models::default();
        let state = gun_fight();
        assert!(!state.projectiles.is_empty(), "no round in flight");
        assert!(!state.rewinds.is_empty(), "no round carries a rewind");
        assert_eq!(state.volumes.len(), 70);
        assert!(state.projectiles.len() > 1, "want several rounds in flight");
        for round in &state.projectiles {
            let copy: Projectile = round_trip(round, &models).unwrap();
            assert_eq!(&copy, round);
        }
        for own in state.ownships() {
            let copy: Ownship = round_trip(own, &models).unwrap();
            assert_eq!(text(&copy), text(own));
        }
        for row in &state.targets {
            let copy: Target = round_trip(row, &models).unwrap();
            assert_eq!(&copy, row);
        }
        let rewinds: std::collections::BTreeMap<u32, u16> =
            round_trip(&state.rewinds, &models).unwrap();
        assert_eq!(rewinds, state.rewinds);
        let history = round_trip(&state.volumes, &models).unwrap();
        // 70 ticks were recorded; the newest 61 are the ones a rewind reads.
        assert_eq!(history.len(), 61);
        assert_eq!(
            history.span().map(|s| s.1),
            state.volumes.span().map(|s| s.1)
        );
        for id in 0..4 {
            for rewind in 0..=60 {
                assert_eq!(history.volume(id, rewind), state.volumes.volume(id, rewind));
            }
        }
        let ledger = round_trip(&state.ledger, &models).unwrap();
        assert_eq!(ledger.kills(), state.ledger.kills());
        println!(
            "gun fight: {} rounds, {} rewinds, history {} bytes, {} ownships",
            state.projectiles.len(),
            state.rewinds.len(),
            to_bytes(&state.volumes, &models).unwrap().body.len(),
            state.ownships().len(),
        );
    }

    #[test]
    fn a_guiding_missile_round_trips_with_its_seeker() {
        let models = Models::default();
        let state = missile_fight();
        let guided: Vec<&Projectile> = state
            .projectiles
            .iter()
            .filter(|p| p.guidance.is_some())
            .collect();
        assert!(!guided.is_empty(), "no missile is guiding");
        for round in guided {
            let copy: Projectile = round_trip(round, &models).unwrap();
            assert_eq!(&copy, round);
            assert!(copy.guidance.is_some() && copy.motion.is_some());
        }
        // The ownship's missile warning tracks it.
        let own = state.own();
        let copy: Ownship = round_trip(own, &models).unwrap();
        assert_eq!(text(&copy), text(own));
    }

    #[test]
    fn a_weapon_record_costs_one_coding_however_many_rounds_carry_it() {
        let models = Models::default();
        let mut state = fixture(false);
        let weapon = state.own().configuration().stations[0].weapon.clone();
        for id in 0..40 {
            state.projectiles.push(Projectile {
                id,
                owner: 9,
                weapon: Some(weapon.clone()),
                guidance: None,
                motion: None,
                guidance_ticks: None,
                age: u64::from(id),
                incoming: None,
                station: 0,
                position: [f64::from(id), 1000., 0.],
                previous: [f64::from(id) - 1., 1000., 0.],
                direction: [0., 0., 1.],
                speed_f8: 1000 << 8,
                launched_t: 0,
                target: None,
                fall: Default::default(),
                gun_round: Some(0),
                tracer: id % 3 == 0,
            });
        }
        let coded = to_bytes(&state.projectiles, &models).unwrap();
        assert_eq!(coded.records.len(), 1, "one record for 40 equal weapons");
        let copy: Vec<Projectile> = from_bytes(&coded, &models).unwrap();
        assert_eq!(copy, state.projectiles);
    }

    #[test]
    fn small_parts_round_trip() {
        let models = Models::default();
        let strike = Strike {
            owner: 3,
            victim: 9,
            weapon_flags: 0x80,
            destroyed: true,
            amount: 77,
        };
        assert_eq!(round_trip(&strike, &models).unwrap(), strike);
        let effect = Effect {
            position: [1., 2., 3.],
            kind: EffectKind::DebrisImpact,
            ticks: 240,
            blast: Some(34),
            scale: 159,
        };
        assert_eq!(round_trip(&effect, &models).unwrap(), effect);
        let none = Effect {
            blast: None,
            ..effect
        };
        assert_eq!(round_trip(&none, &models).unwrap(), none);
        let support = ActorSupport {
            owner: 4,
            observation: None,
            supported: true,
            radar_position: [5., 6., 7.],
            radar_emitting: true,
        };
        assert_eq!(round_trip(&support, &models).unwrap(), support);
        let ground = OrientedBox {
            center: [10., 20., 30.],
            half: [1., 2., 3.],
            heading: 0.5,
            pitch: -0.25,
            bank: 0.125,
        };
        let copy: OrientedBox = round_trip(&ground, &models).unwrap();
        assert_eq!(text(&copy), text(&ground));
        for kind in [
            SoundKind::Impact,
            SoundKind::Explosion,
            SoundKind::Blast(17),
            SoundKind::AircraftPass,
            SoundKind::MissilePass,
            SoundKind::SonicBoom,
            SoundKind::Chaff,
            SoundKind::Flare,
        ] {
            assert_eq!(round_trip(&kind, &models).unwrap(), kind);
        }
        for readiness in [Readiness::Ready, Readiness::FieldOfView, Readiness::NoRadar] {
            assert_eq!(round_trip(&readiness, &models).unwrap(), readiness);
        }
    }

    #[test]
    fn a_whole_state_restores_and_steps_on_identically() {
        let models = Models::default();
        for (name, build) in [
            ("guns", gun_fight as fn() -> State),
            ("missile", missile_fight),
        ] {
            let original = drained(build());
            let Some(mut copy) = whole(&original) else {
                // The effects slice is not merged: nothing more to check here.
                continue;
            };
            let mut original = original;
            for n in 0..600 {
                let held = name == "guns" && (40..70).contains(&n);
                step(&mut original, held, 31);
                step(&mut copy, held, 31);
                if n % 50 == 0 {
                    let (a, b) = (drained(original.clone()), drained(copy.clone()));
                    assert_eq!(
                        to_bytes(&a, &models).unwrap(),
                        to_bytes(&b, &models).unwrap(),
                        "{name}: tick {n} after the restore"
                    );
                    // The restored history is shorter for its first second
                    // (it carries only the frames a lookup can reach); after
                    // that the two are equal in every field.
                    if n >= 70 {
                        assert_eq!(text(&a), text(&b), "{name}: tick {n} after the restore");
                    }
                }
            }
        }
    }

    #[test]
    fn a_state_with_two_ownships_keeps_them_in_order_and_a_disordered_one_is_refused() {
        let models = Models::default();
        let mut state = fixture(false);
        let config = state.own().configuration().clone();
        state
            .add_ownship(Ownship::new(5, Side(2), config.clone(), true).unwrap())
            .unwrap();
        state
            .add_ownship(Ownship::new(3, Side(2), config, true).unwrap())
            .unwrap();
        assert_eq!(
            state
                .ownships()
                .iter()
                .map(|o| o.aircraft)
                .collect::<Vec<_>>(),
            [0, 3, 5]
        );
        if let Some(copy) = whole(&state) {
            assert_eq!(
                copy.ownships()
                    .iter()
                    .map(|o| o.aircraft)
                    .collect::<Vec<_>>(),
                [0, 3, 5]
            );
            state.ownships.swap(0, 2);
            let coded = to_bytes(&state, &models).unwrap();
            assert!(from_bytes::<State>(&coded, &models).is_err());
        }
    }

    #[test]
    fn the_why_records_are_not_coded() {
        let mut state = fixture(true);
        state.command(0, super::super::Command::ReleaseChaff, shooter());
        state.command(0, super::super::Command::Incoming, shooter());
        let noisy = state.clone();
        let quiet = drained(state);
        let models = Models::default();
        // Whatever the logs hold, the coding is the same (when it can be made).
        match (to_bytes(&noisy, &models), to_bytes(&quiet, &models)) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("one coded and one did not: {a:?} / {b:?}"),
        }
    }
}
