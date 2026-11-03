//! Combat goldens: the live-fire adapter firing guns and bombs, radar,
//! supported and infrared missiles with seeker activation, decoys and
//! jamming, AI-owned shots at the player and at targets, hits, damage and the
//! debrief ledger; and the player's own chaff and flares with their decoy
//! rolls, sounds and released devices.

use std::collections::BTreeMap;

use tore_formats::aircraft::AircraftId;
use tore_formats::weapons::{
    Burst, Countermeasures, Damage, Effects, Guidance, Movement, Seeker, Weapon, Zone,
};

use super::{Fingerprint, Probe, record_threat, run_twice, verify};
use crate::ai::DecisionRandom;
use crate::ai::threat::{DecoyOutcome, GuidingMissile, SeekerClass, decoy_missile};
use crate::attitude::Basis;
use crate::combat::FallState;
use crate::combat::countermeasures::Devices;
use crate::combat::ledger::Resolution;
use crate::combat::live::{
    ActorSupport, Command, Configuration, Event, Launcher, LocalizedDamage, OwnshipInput,
    Projectile, State, Station, Target,
};
use crate::combat::missiles::{self, Flight, LaunchMode, Motion, TargetRole, seeker};
use crate::sensors;

// Recorded on macOS aarch64. See the module comment in golden_tests.rs before
// changing any of these.
/// The ownship's aircraft in these scenarios.
const OWN: u32 = 0;
// Present wrecks now receive physical hits, remain observable/selectable, and
// missiles stop orbiting passed memory points. Measured on Apple Silicon CI.
const GUNS_AND_DAMAGE: u64 = 0x5a40_7a7e_c359_f7ac;
const GUIDED_MISSILES: u64 = 0x2e69_080a_40fa_b9dc;
const PLAYER_COUNTERMEASURES: u64 = 0x832b_d9f6_16cd_8618;

#[test]
fn combat_guns_and_damage_match_recorded_fingerprint() {
    let seen = std::cell::RefCell::new(BTreeMap::new());
    let outcome = run_twice("combat/guns-and-damage", GUNS_AND_DAMAGE, |probe| {
        let (value, events) = guns_and_damage(probe);
        *seen.borrow_mut() = events;
        value
    });
    verify([outcome]);
    require(
        "combat/guns-and-damage",
        &seen.into_inner(),
        &["Fired", "Hit", "Destroyed", "Ground", "PlayerDamaged"],
    );
}

#[test]
fn combat_guided_missiles_match_recorded_fingerprint() {
    let seen = std::cell::RefCell::new(BTreeMap::new());
    let outcome = run_twice("combat/guided-missiles", GUIDED_MISSILES, |probe| {
        let (value, events) = guided_missiles(probe);
        *seen.borrow_mut() = events;
        value
    });
    verify([outcome]);
    require(
        "combat/guided-missiles",
        &seen.into_inner(),
        &[
            "Fired",
            "SeekerActivated",
            "Pitbull",
            "Hit",
            "TrackLost",
            "Defeated",
            "Spoofed",
            "PlayerDamaged",
            "SubsystemDamaged",
            "Jolt",
        ],
    );
}

#[test]
fn combat_player_countermeasures_match_recorded_fingerprint() {
    let seen = std::cell::RefCell::new(BTreeMap::new());
    let outcome = run_twice(
        "combat/player-countermeasures",
        PLAYER_COUNTERMEASURES,
        |probe| {
            let (value, events) = player_countermeasures(probe);
            *seen.borrow_mut() = events;
            value
        },
    );
    verify([outcome]);
    require(
        "combat/player-countermeasures",
        &seen.into_inner(),
        &[
            "ChaffDecoyed",
            "FlareDecoyed",
            "Resisted",
            "Empty",
            "Unlimited",
            "Disabled",
            "FlareLanded",
            "FlareBurnedOut",
            "ChaffExpired",
            "PlayerDamaged",
        ],
    );
}

/// A scenario must keep producing the outcomes its name promises.
fn require(scenario: &str, seen: &BTreeMap<&'static str, u32>, wanted: &[&str]) {
    for name in wanted {
        assert!(
            seen.get(name).is_some_and(|count| *count > 0),
            "{scenario} no longer produces {name}: {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Recording

fn event_name(event: &Event) -> &'static str {
    match event {
        Event::Fired { .. } => "Fired",
        Event::SeekerActivated(_) => "SeekerActivated",
        Event::Pitbull(_) => "Pitbull",
        Event::Hit(_) => "Hit",
        Event::Destroyed(_) => "Destroyed",
        Event::Airburst(_) => "Airburst",
        Event::Ground => "Ground",
        Event::TrackLost(_) => "TrackLost",
        Event::OwnshipDamaged { .. } => "PlayerDamaged",
        Event::SubsystemDamaged { .. } => "SubsystemDamaged",
        Event::OwnshipDestroyed { .. } => "PlayerDestroyed",
        Event::PilotKilled { .. } => "PilotKilled",
        Event::OwnshipGroundImpact { .. } => "PlayerGroundImpact",
        Event::Defeated(_) => "Defeated",
        Event::Jolt(_) => "Jolt",
    }
}

fn record_event(fp: &mut Fingerprint, event: &Event) {
    fp.text(event_name(event));
    match event {
        Event::Fired { station, .. } => fp.count(*station),
        Event::SubsystemDamaged { index, .. } => fp.count(*index),
        Event::SeekerActivated(id)
        | Event::Pitbull(id)
        | Event::Hit(id)
        | Event::Destroyed(id)
        | Event::Airburst(id)
        | Event::TrackLost(id)
        | Event::Defeated(id) => fp.int(*id),
        Event::OwnshipDamaged { amount, .. } => fp.int(*amount),
        Event::Jolt(jolt) => {
            // The fingerprint keeps the earlier encoding: none for the ownship.
            fp.option((jolt.target != OWN).then_some(jolt.target), |fp, id| {
                fp.int(id)
            });
            fp.vector(jolt.from);
            fp.f64(jolt.strength);
        }
        Event::Ground
        | Event::OwnshipDestroyed { .. }
        | Event::PilotKilled { .. }
        | Event::OwnshipGroundImpact { .. } => {}
    }
}

fn record_observation(fp: &mut Fingerprint, observation: &seeker::Observation) {
    fp.int(observation.id);
    fp.vector(observation.position);
    fp.vector(observation.velocity);
    fp.f64(observation.quality);
    fp.f64(observation.off_axis);
    fp.f64(observation.range);
}

fn record_seeker(fp: &mut Fingerprint, s: &seeker::Seeker) {
    fp.option(s.target, |fp, id| fp.int(id));
    fp.option(s.candidate, |fp, id| fp.int(id));
    fp.int(s.dwell);
    fp.int(s.missing);
    fp.bool(s.acquired);
    fp.name(&s.status);
    fp.f64(s.quality);
    fp.option(s.observation.as_ref(), record_observation);
}

fn record_flight_guidance(fp: &mut Fingerprint, flight: &Flight) {
    fp.bool(flight.unguided);
    fp.vector(flight.launch_origin);
    fp.option(flight.qualified_target, |fp, id| fp.int(id));
    fp.name(&flight.mode);
    record_seeker(fp, &flight.seeker);
    fp.bool(flight.enabled);
    fp.option(flight.last_intercept, |fp, point| fp.vector(point));
    fp.option(flight.solution, |fp, solution| {
        fp.vector(solution.point);
        fp.f64(solution.seconds);
    });
}

fn record_projectile(fp: &mut Fingerprint, p: &Projectile) {
    fp.int(p.id);
    fp.int(p.owner);
    fp.option(p.weapon.as_ref(), |fp, weapon| fp.text(&weapon.source));
    fp.option(p.guidance.as_ref(), record_flight_guidance);
    fp.option(p.motion, |fp, motion| {
        fp.vector(motion.velocity);
        fp.f64(motion.gain);
        fp.f64(motion.budget);
    });
    fp.option(p.guidance_ticks, |fp, ticks| fp.u64(ticks));
    fp.u64(p.age);
    fp.bool(p.incoming.is_some());
    fp.count(p.station);
    fp.vector(p.position);
    fp.vector(p.previous);
    fp.vector(p.direction);
    fp.int(p.speed_f8);
    fp.int(p.launched_t);
    fp.option(p.target, |fp, id| fp.int(id));
    fp.int(p.fall.velocity_f8);
    fp.option(p.gun_round, |fp, round| fp.int(round));
    fp.bool(p.tracer);
}

fn record_target(fp: &mut Fingerprint, t: &Target) {
    fp.int(t.id);
    fp.int(t.hp);
    fp.int(t.initial_hp);
    fp.vector(t.position);
    fp.vector(t.velocity);
    fp.vector(t.basis.forward);
    fp.vector(t.basis.up);
    fp.bool(t.airborne);
    fp.bool(t.on_ground);
    fp.bool(t.radar_emitting);
    fp.bool(t.jammer_active);
    fp.name(&t.heat);
    fp.option(t.wreck.as_ref(), |fp, wreck| {
        fp.name(&wreck.phase);
        fp.u64(wreck.ticks);
        fp.int(wreck.polls);
        fp.vector(wreck.angular_rates);
    });
    for amount in t.localized_damage.amounts {
        fp.int(amount);
    }
    fp.option(t.localized_damage.structural_variant, |fp, v| fp.count(v));
    fp.option(t.localized_damage.structural_section, |fp, s| fp.name(&s));
    fp.bool(t.fragment_released);
    fp.int(t.category);
}

/// Everything a tick of combat can change, as the host and the player see it.
fn record_state(fp: &mut Fingerprint, s: &mut State, launcher: Launcher) {
    for strike in s.take_strikes() {
        fp.int(strike.owner);
        fp.option((strike.victim != OWN).then_some(strike.victim), |fp, id| {
            fp.int(id)
        });
        fp.int(strike.weapon_flags);
        fp.bool(strike.destroyed);
    }
    for sound in s.take_sound_events() {
        fp.name(&sound.kind);
        fp.vector(sound.position);
        fp.bool(sound.arrived);
    }
    fp.u64(s.tick());
    fp.name(&s.own().release_readiness);
    fp.name(&s.own().launch_mode);
    record_seeker(fp, &s.own().mounted);
    fp.option(s.own().bore_observation.as_ref(), record_observation);
    fp.name(&s.weapon_rules);
    fp.count(s.own().ammo.len());
    for ammo in &s.own().ammo {
        fp.int(*ammo);
    }
    fp.count(s.own().selected);
    fp.bool(s.own().armed);
    fp.option(s.own().sensors.selected(), |fp, id| fp.int(id));
    fp.option(s.own().sensors.acquired(), |fp, id| fp.int(id));
    fp.count(s.own().sensors.contacts().len());
    for contact in s.own().sensors.contacts() {
        fp.int(contact.id);
        fp.name(&contact.channel);
        fp.vector(contact.position);
    }
    fp.count(s.projectiles.len());
    for projectile in &s.projectiles {
        record_projectile(fp, projectile);
    }
    fp.count(s.targets.len());
    for target in &s.targets {
        record_target(fp, target);
    }
    fp.count(s.effects.len());
    for effect in &s.effects {
        fp.name(&effect.kind);
        fp.vector(effect.position);
        fp.int(effect.ticks);
    }
    fp.count(s.smoke.puffs.len());
    for puff in &s.smoke.puffs {
        fp.name(&puff.kind);
        fp.vector(puff.position);
        fp.int(puff.age);
    }
    fp.count(s.debris.len());
    for piece in &s.debris {
        fp.int(piece.owner);
        fp.count(piece.variant);
        fp.vector(piece.position);
        fp.vector(piece.velocity);
    }
    for count in [s.own().shots, s.own().hits, s.own().kills] {
        fp.int(count);
    }
    fp.int(s.own().hp);
    fp.int(s.own().damage);
    for count in s.own().subsystem_counts {
        fp.int(count);
    }
    fp.option(s.own().last_subsystem, |fp, index| fp.count(index));
    for failed in [
        s.own().radar_failed,
        s.own().visual_failed,
        s.own().infrared_failed,
        s.own().rwr_failed,
        s.own().ecm_failed,
        s.target_jammer,
    ] {
        fp.bool(failed);
    }
    fp.int(s.own().chaff);
    fp.int(s.own().flares);
    fp.count(s.history.len());
    for record in &s.history {
        fp.u64(record.tick);
        fp.int(record.target);
        fp.count(record.station);
        fp.count(record.class);
        fp.int(record.nominal);
        fp.int(record.applied);
        fp.int(record.hp_after);
    }
    fp.int(s.range_category);
    let threats: Vec<_> = s.own().missile_threats.records().collect();
    fp.count(threats.len());
    for record in threats {
        record_threat(fp, record);
    }
    // The debrief ledger through its public reads only, so a per-shot
    // outcome list added later cannot move this fingerprint.
    let tallies: Vec<_> = s.ledger.tallies().collect();
    fp.count(tallies.len());
    for (key, tally) in tallies {
        fp.int(key.owner);
        fp.option(key.aim, |fp, id| fp.int(id));
        fp.name(&key.kind);
        for value in [
            tally.launched,
            tally.hit,
            tally.damage,
            tally.missed,
            tally.spoofed,
            tally.jammed,
        ] {
            fp.int(value);
        }
    }
    fp.count(s.ledger.kills().len());
    for kill in s.ledger.kills() {
        fp.int(kill.owner);
        fp.int(kill.victim);
        fp.int(kill.category);
        fp.bool(kill.aircraft);
    }
    let victims: Vec<u32> = std::iter::once(OWN)
        .chain(s.targets.iter().map(|t| t.id))
        .collect();
    for victim in victims {
        fp.option(s.ledger.credit(victim), |fp, kill| {
            fp.int(kill.owner);
            fp.int(kill.category);
        });
    }
    // What the cockpit shows and plays.
    fp.name(&s.own_view().readiness(launcher));
    fp.bool(s.own_view().can_lock(launcher));
    fp.bool(s.own_view().guidance_available(launcher));
    fp.option(s.own_view().seeker_tone(launcher), |fp, tone| {
        fp.f64(tone.strength);
        fp.bool(tone.ground);
        fp.bool(tone.radar);
        fp.bool(tone.locked);
    });
    fp.option(s.own_view().estimated_max_range(launcher), |fp, range| {
        fp.f64(range)
    });
    fp.bool(s.own_view().in_estimated_range(launcher));
    fp.int(s.own_view().estimated_hit_percent(launcher));
    fp.option(s.own_view().favorable_firing_band(launcher), |fp, band| {
        fp.f64(band.minimum);
        fp.f64(band.maximum);
    });
    fp.option(s.own_view().designated(), |fp, id| fp.int(id));
    for region in s.own().damage_regions() {
        fp.f64(region);
    }
    fp.option(s.own().damage_section(), |fp, section| fp.name(&section));
    fp.f64(s.own().payload_lbs());
}

// ---------------------------------------------------------------------------
// Fixtures: private copies of the live-fire test fixture's synthetic values

fn zone(range: i32) -> Zone {
    Zone {
        heading: 12_000,
        pitch: 12_000,
        minimum_range: 0,
        maximum_range: range,
        minimum_altitude: i32::MIN,
        maximum_altitude: i32::MAX,
    }
}

/// The live-fire fixture's synthetic weapon under a reviewed source name,
/// which selects the guidance profile. Signature 0 is unguided, 2 infrared
/// and 3 radar.
fn weapon(source: &str, signature: u8) -> Weapon {
    let guided = signature != 0;
    Weapon {
        source: source.into(),
        name: "Synthetic".into(),
        hud_name: "SYN".into(),
        shape: None,
        fire_sound: None,
        native_callback: "_PROJProc".into(),
        flags: if guided { 0x240 } else { 0x844 },
        object_flags: 0,
        weight: 10,
        movement: Movement {
            minimum_speed: 10,
            corner_speed: 1_000,
            maximum_speed: 2_000,
            acceleration: 100,
            deceleration: 2,
            initial_speed: 1_000,
            final_speed: 500,
            launch_retard: 100,
            ignite_t: 0,
            fuel_t: 10,
            remove_t: 20,
            powered_turn_rate: 10_000,
            unpowered_turn_rate: 10_000,
            performance_at_0: 100,
            performance_at_20: 100,
            cruise: [0; 4],
            jink: [0; 3],
        },
        burst: Burst {
            projectiles_in_pod: 1,
            actual_rounds_per_game: 2,
            game_rounds_in_burst: 1,
            game_rounds_in_carpet_burst: 1,
            game_burst_t: 1,
            reload_t: 0,
            startup_shots: 0,
            random_fire_percent: 0,
            offset_fire_percent: 0,
            offset_fire_heading: 0,
            offset_fire_pitch: 0,
            sine_pattern: [0; 4],
        },
        seeker: Seeker {
            flags: [0; 2],
            signature,
            look_down: 0,
            doppler_above: 0,
            doppler_below: 0,
            doppler_minimum_range: 0,
            all_aspect: 0,
            zones: [zone(10_000); 2],
            chaff_flare_chance: 0,
            deception_chance: 0,
        },
        guidance: Guidance {
            track_t: 1,
            track_max_g_raw: 1,
            target_sun_chance: 0,
            max_aon: 0,
            chances: [100; 4],
            hit_modifiers: [0; 9],
        },
        damage: Damage {
            by_class: [10; 5],
            fuze_arm_t: 0,
            fuze_radius: 0,
            side_hit_fuze_failure: 0,
            collateral_radius: 0,
            collateral_percent: 0,
        },
        effects: Effects {
            object_explosion: 0,
            land_explosion: 0,
            water_explosion: 0,
            crater_size: 0,
            smoke: [0; 5],
            max_sound_distance: 0,
            frequency_adjustment: 0,
        },
    }
}

/// A longer-legged synthetic missile: 40 s of flight, 10 s of it powered.
fn missile(source: &str, signature: u8, range: i32, decoy_percent: u8) -> Weapon {
    let mut w = weapon(source, signature);
    w.flags = if matches!(source, "R530.JT" | "SYNTHETIC.JT") {
        0x241
    } else {
        0x41
    };
    w.movement.maximum_speed = 3_000;
    w.movement.initial_speed = 1_200;
    w.movement.final_speed = 1_500;
    w.movement.acceleration = 300;
    w.movement.fuel_t = 40;
    w.movement.remove_t = 160;
    w.movement.powered_turn_rate = 12_000;
    w.movement.unpowered_turn_rate = 8_000;
    w.seeker.zones = [zone(range); 2];
    w.seeker.chaff_flare_chance = decoy_percent;
    w.damage.by_class = [60; 5];
    w.damage.fuze_radius = 30;
    w
}

fn gun() -> Weapon {
    let mut w = weapon("M61.JT", 0);
    w.burst.actual_rounds_per_game = 2;
    w.burst.game_rounds_in_burst = 4;
    w.damage.by_class = [12; 5];
    w
}

fn bomb() -> Weapon {
    let mut w = weapon("MK82.JT", 0);
    w.flags = 0x14;
    w.movement.remove_t = 120;
    w.damage.by_class = [40; 5];
    w.damage.fuze_radius = 40;
    w
}

/// A 90/50 nm radar so the default display range selects track-while-scan,
/// plus the visual channel every aircraft has.
fn sensor_profiles() -> sensors::SensorProfiles {
    let volume = |nmi: f64| sensors::Volume {
        azimuth_rad: 1.,
        elevation_rad: 1.,
        minimum_ft: 0.,
        maximum_ft: nmi * sensors::FEET_PER_NAUTICAL_MILE,
        minimum_relative_ft: f64::NEG_INFINITY,
        maximum_relative_ft: f64::INFINITY,
    };
    sensors::SensorProfiles {
        aircraft: AircraftId::F18,
        radar: Some(sensors::RadarProfile {
            record: "GOLDEN.SEE".into(),
            search: volume(90.),
            track: volume(50.),
            look_down: 0.,
            preset: sensors::Preset::Advanced,
            notch: sensors::Preset::Advanced.notch(),
            resistance: sensors::Preset::Advanced.resistance(),
            band: 0,
            source_flags: [0; 2],
            source_doppler: [0; 3],
        }),
        infrared: None,
        visual: Some(sensors::profile::VisualProfile {
            record: "GOLDEN.VIS".into(),
            search: volume(10.),
            track: volume(5.),
        }),
        jammer: None,
        signature: sensors::SignatureProfile::default(),
    }
}

fn configuration(stations: Vec<(Weapon, u16, bool)>) -> Configuration {
    let slots = stations.len();
    Configuration {
        fragment_offsets: [[0.; 3]; 2],
        ecm: Countermeasures {
            weight: 0,
            flags: 0,
            mode_flags: 0x110,
            chaff: [4, 0, 0, 0],
            flare: [4, 0, 0, 0],
            radar_deception_chance: 30,
            radar_signature_add: 0,
            radar_noise_range: [0; 2],
            infrared_deception_chance: 20,
            infrared_signature_add: 0,
            infrared_lose_lock_time: 0,
        },
        system_damage: [0x11; 45],
        damage_capacity: 60,
        afterburner_available: true,
        hardpoint_slots: (0..slots).map(Some).chain([None, None, None]).collect(),
        radar_hardpoint: Some(slots),
        visual_hardpoint: slots + 2,
        ecm_hardpoint: Some(slots + 1),
        aircraft: AircraftId::F18,
        strike_class: crate::models::strike_class::StrikeClass::Fighter,
        stations: stations
            .into_iter()
            .map(|(weapon, count, internal)| Station {
                weapon,
                mount: [0.; 3],
                count,
                internal,
                rack: Default::default(),
            })
            .collect(),
        hit_points: 20,
        target_category: 0x80,
        fixed_external_equipment_lbs: 0,
        tanks: vec![],
        gun_pods: vec![],
        external_equipment_lbs: 0,
        external_fuel_lbs: [0.; 9],
        engines: 1,
        wreck_power: crate::wreck::Power::default(),
        infrared_hardpoint: None,
        rwr_hardpoint: None,
        sensors: sensor_profiles(),
        sight: None,
        designator_pods: vec![],
    }
}

fn aircraft_target(id: u32, position: [f64; 3], velocity: [f64; 3], hp: i32) -> Target {
    let forward = crate::attitude::unit(velocity);
    Target {
        aircraft: Some(AircraftId::Mig29),
        role: TargetRole::Aircraft,
        heat: seeker::Heat::Engine {
            on: true,
            throttle: 0.8,
            afterburner: false,
        },
        radar_emitting: true,
        id,
        position,
        velocity,
        basis: Basis::new(forward[0].atan2(forward[2]), forward[1].asin(), 0.),
        configuration: sensors::Configuration::CLEAN,
        signature: sensors::SignatureProfile::default(),
        jammer: None,
        jammer_active: false,
        airborne: true,
        on_ground: false,
        radius: 28.,
        hp,
        initial_hp: hp,
        fragment_offsets: [[0.; 3]; 2],
        wreck: None,
        wreck_power: crate::wreck::Power::symmetric(2, 20., 30.),
        fragment_released: false,
        localized_damage: LocalizedDamage::default(),
        faults: Default::default(),
        category: 0x80,
        side: crate::combat::live::NO_SIDE,
    }
}

/// The player's aircraft as combat sees it: level, straight and fast, with
/// the host moving it between ticks.
fn launcher(position: [f64; 3], heading: f64, pitch: f64, speed: f64) -> Launcher {
    let basis = Basis::new(heading, pitch, 0.);
    Launcher {
        radar_power: true,
        position,
        basis,
        speed_fps: speed,
        velocity: basis.forward.map(|axis| axis * speed),
        bay_ready: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: sensors::Controls::default(),
    }
}

fn fly(launcher: &mut Launcher) {
    for (position, velocity) in launcher.position.iter_mut().zip(launcher.velocity) {
        *position += velocity * crate::flight::DT;
    }
}

/// The host's decoy rule for one released chaff or flare, as the app applies
/// it: matching seeker class, guiding on the releaser, a percentage roll.
fn release_decoy(
    s: &mut State,
    class: SeekerClass,
    releaser: u32,
    random: &mut DecisionRandom,
    fp: &mut Fingerprint,
    seen: &mut BTreeMap<&'static str, u32>,
) {
    let config = s.own().configuration().clone();
    for p in &mut s.projectiles {
        let w = p.weapon(&config);
        let seeker = match w.seeker.signature {
            2 => SeekerClass::Infrared,
            3 => SeekerClass::Radar,
            _ => continue,
        };
        let guiding = p.target == Some(releaser)
            && p.guidance.as_ref().is_none_or(|flight| {
                flight.enabled && flight.seeker.acquired && flight.seeker.observation.is_some()
            });
        let outcome = decoy_missile(
            &GuidingMissile {
                seeker,
                guiding_on_releaser: guiding,
                decoy_susceptibility_percent: w.seeker.chaff_flare_chance,
            },
            class,
            100,
            random,
        )
        .expect("bounded percentages");
        fp.name(&outcome);
        if outcome == DecoyOutcome::Decoyed {
            *seen.entry("Spoofed").or_default() += 1;
            s.ledger.resolve(p.id, Resolution::Spoofed);
            p.target = None;
            p.guidance = None;
        }
    }
}

/// An AI actor's missile, created the way the app realises an AI launch.
fn owned_missile(
    id: u32,
    owner: u32,
    weapon: &Weapon,
    origin: [f64; 3],
    target: u32,
    aim: [f64; 3],
    tick: u64,
) -> Projectile {
    let profile = missiles::Profile::for_weapon(weapon).expect("reviewed missile");
    let direction = crate::attitude::unit(std::array::from_fn(|i| aim[i] - origin[i]));
    let velocity = direction.map(|axis| axis * 900.);
    Projectile {
        id,
        owner,
        weapon: Some(weapon.clone()),
        guidance: Some(Flight::from_supported_launch(
            profile,
            LaunchMode::Cued,
            seeker::Observation {
                id: target,
                position: aim,
                velocity: [0.; 3],
                quality: 1.,
                off_axis: 0.,
                range: missiles::length(missiles::sub(aim, origin)),
            },
            origin,
        )),
        motion: Some(Motion::launch(weapon, velocity, origin[1])),
        guidance_ticks: Some(profile.guidance_ticks),
        age: 0,
        incoming: (target == OWN).then_some(OWN),
        station: 0,
        position: origin,
        previous: origin,
        direction,
        speed_f8: crate::combat::launch_speed(&weapon.movement, 900 * 256).expect("valid speeds")
            * 256,
        launched_t: (tick / 30) as u16,
        target: Some(target),
        fall: FallState::default(),
        gun_round: None,
        tracer: false,
    }
}

fn step(
    s: &mut State,
    held: bool,
    launcher: Launcher,
    fp: &mut Fingerprint,
    seen: &mut BTreeMap<&'static str, u32>,
) {
    let events = s.step(
        &[OwnshipInput {
            aircraft: 0,
            held,
            launcher,
        }],
        |_, _| 0.,
    );
    fp.bool(held);
    fp.count(events.len());
    for event in &events {
        record_event(fp, event);
        *seen.entry(event_name(event)).or_default() += 1;
    }
    record_state(fp, s, launcher);
}

// ---------------------------------------------------------------------------
// Scenario: guns, bombs, player damage and range commands

fn guns_and_damage(probe: &Probe) -> (u64, BTreeMap<&'static str, u32>) {
    let mut fp = Fingerprint::default();
    let mut seen = BTreeMap::new();
    let mut s = State::new(
        configuration(vec![(gun(), 600, true), (bomb(), 6, false)]),
        true,
    )
    .expect("synthetic configuration");
    // Recorded with the full damage model, system faults included.
    s.cheats.damage = crate::cheats::Damage::Realistic;
    let mut l = launcher([0., 3_000., 0.], 0., 0., 600.);
    s.range_target(0, l);
    s.targets.push(aircraft_target(
        20,
        [800., 3_150., 2_000.],
        [-200., 0., 250.],
        20,
    ));
    s.targets.push(aircraft_target(
        21,
        [0., 3_050., 3_500.],
        [0., 0., 450.],
        60,
    ));
    s.add_ground_target(
        0x4000_0001,
        crate::airport::OrientedBox {
            center: [0., 0., 9_000.],
            half: [60., 20., 60.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
        },
        30,
        0x100,
        crate::combat::live::NO_SIDE,
    )
    .expect("ground target");
    for tick in 0..1_800u64 {
        let pitch = if (640..900).contains(&tick) { -0.6 } else { 0. };
        let basis = Basis::new(0., pitch, 0.);
        l.basis = basis;
        l.velocity = basis.forward.map(|axis| axis * l.speed_fps);
        match tick {
            300 => s.command(0, Command::DamagePlayer, l),
            360 | 420 => s.command(0, Command::Incoming, l),
            480 => s.cheats.easy_aiming = true,
            600 => s.cheats.easy_aiming = false,
            620 => s.command(0, Command::NextWeapon, l),
            650 => s.command(0, Command::Incoming, l),
            900 => s.command(0, Command::SelectNav, l),
            920 => s.command(0, Command::NextSelection, l),
            950 => {
                s.command(0, Command::ToggleArm, l);
                s.command(0, Command::ToggleArm, l);
            }
            1_000 => {
                let mut a = aircraft_target(30, [5_000., 3_000., 8_000.], [0., 0., 600.], 20);
                a.velocity = [0., 0., 600.];
                let mut b = aircraft_target(31, [5_000., 3_000., 8_035.], [0., 0., -600.], 20);
                b.velocity = [0., 0., -600.];
                s.targets.extend([a, b]);
            }
            1_350 => s.command(0, Command::FailStation, l),
            1_400 => {
                s.command(0, Command::SelectNav, l);
                s.command(0, Command::NextSelection, l);
                s.cheats.unlimited_ammo = true;
            }
            1_550 => s.command(0, Command::ReplaceTarget, l),
            1_650 => s.command(0, Command::CycleClass, l),
            1_700 => s.command(0, Command::TargetDistance(2_000), l),
            1_750 => s.command(0, Command::ClearRange, l),
            _ => {}
        }
        let held = matches!(
            tick,
            0..180 | 240..480 | 480..600 | 700..760 | 1_300..1_500 | 1_560..1_640
        );
        step(&mut s, held, l, &mut fp, &mut seen);
        fly(&mut l);
        if tick % 150 == 149 {
            probe.part(format!("tick {tick}"), fp.value());
        }
    }
    (fp.value(), seen)
}

// ---------------------------------------------------------------------------
// Scenario: guided missiles, decoys, jamming and AI-owned shots

fn guided_missiles(probe: &Probe) -> (u64, BTreeMap<&'static str, u32>) {
    let mut fp = Fingerprint::default();
    let mut seen = BTreeMap::new();
    let active = missile("AIM120.JT", 3, 60_000, 30);
    let supported = missile("R530.JT", 3, 40_000, 30);
    let infrared = missile("AIM9M.JT", 2, 20_000, 60);
    // No reviewed profile: the compatibility pursuit path.
    let compatibility = missile("SYNTHETIC.JT", 3, 40_000, 0);
    let mut s = State::new(
        configuration(vec![
            (gun(), 200, true),
            (active.clone(), 4, false),
            (supported, 2, false),
            (compatibility, 2, false),
            (infrared.clone(), 4, false),
        ]),
        true,
    )
    .expect("synthetic configuration");
    // Recorded with the full damage model, system faults included.
    s.cheats.damage = crate::cheats::Damage::Realistic;
    let mut l = launcher([0., 20_000., 0.], 0., 0., 800.);
    s.targets.push(aircraft_target(
        40,
        [3_000., 20_000., 42_000.],
        [0., 0., -700.],
        60,
    ));
    s.targets.push(aircraft_target(
        41,
        [-2_500., 19_500., 12_000.],
        [300., 0., 500.],
        60,
    ));
    let mut hot = aircraft_target(42, [0., 20_300., 7_000.], [0., 0., 650.], 60);
    hot.heat = seeker::Heat::Engine {
        on: true,
        throttle: 1.,
        afterburner: true,
    };
    s.targets.push(hot);
    let mut decoys = DecisionRandom::seeded(0x5eed_dec0);
    for tick in 0..2_400u64 {
        // The host flies the targets: one weaves, one notches, one turns.
        for target in &mut s.targets {
            let t = tick as f64 / 120.;
            match target.id {
                40 if tick >= 900 => target.velocity = [700., 0., 0.],
                41 => {
                    let angle = t * 0.15;
                    target.velocity = [300. * angle.cos(), 20. * (t * 0.5).sin(), 500.];
                }
                42 => target.velocity = [120. * (t * 0.4).sin(), 0., 650.],
                _ => {}
            }
        }
        match tick {
            2 => {
                s.command(0, Command::NextWeapon, l);
                s.command(0, Command::DesignateTarget(40), l);
            }
            320 => s.command(0, Command::DesignateTarget(41), l),
            600 => {
                s.command(0, Command::NextWeapon, l);
                s.command(0, Command::DesignateTarget(41), l);
            }
            720 => s.command(0, Command::NextWeapon, l),
            760 => l.radar = false,
            880 => l.radar = true,
            1_000 => {
                s.command(0, Command::NextWeapon, l);
                s.command(0, Command::ClearDesignation, l);
            }
            1_300 => s.command(0, Command::ToggleTargetJammer, l),
            1_320 => {
                s.command(0, Command::SelectNav, l);
                s.command(0, Command::NextSelection, l);
                s.command(0, Command::NextSelection, l);
                s.command(0, Command::DesignateTarget(40), l);
            }
            1_500 => {
                let aim = l.position;
                s.projectiles.push(owned_missile(
                    5_000,
                    7,
                    &active,
                    [2_000., 20_000., 15_000.],
                    OWN,
                    aim,
                    tick,
                ));
            }
            1_600 => {
                let aim = s
                    .targets
                    .iter()
                    .find(|t| t.id == 41)
                    .map_or([0.; 3], |t| t.position);
                s.projectiles.push(owned_missile(
                    5_001,
                    8,
                    &infrared,
                    [-6_000., 19_500., 30_000.],
                    41,
                    aim,
                    tick,
                ));
            }
            _ => {}
        }
        // Owner 7 keeps its radar on the player for its supported shot.
        s.set_actor_supports([ActorSupport {
            owner: 7,
            observation: Some(seeker::Observation {
                id: OWN,
                position: l.position,
                velocity: l.velocity,
                quality: 1.,
                off_axis: 0.,
                range: 15_000.,
            }),
            supported: true,
            radar_position: [2_000., 20_000., 15_000.],
            radar_emitting: true,
        }]);
        // Flares from the hot target, chaff from the player.
        if (1_150..1_300).contains(&tick) && tick.is_multiple_of(30) {
            release_decoy(
                &mut s,
                SeekerClass::Infrared,
                42,
                &mut decoys,
                &mut fp,
                &mut seen,
            );
        }
        if (1_700..1_900).contains(&tick) && tick.is_multiple_of(30) {
            release_decoy(
                &mut s,
                SeekerClass::Radar,
                OWN,
                &mut decoys,
                &mut fp,
                &mut seen,
            );
        }
        let held = matches!(tick, 200 | 450 | 700 | 740 | 1_100 | 1_140 | 1_350 | 1_380);
        step(&mut s, held, l, &mut fp, &mut seen);
        fly(&mut l);
        if tick % 200 == 199 {
            probe.part(format!("tick {tick}"), fp.value());
        }
    }
    (fp.value(), seen)
}

// ---------------------------------------------------------------------------
// Scenario: the player's own chaff and flares

/// Every released chaff cloud and flare, through their public reads.
fn record_devices(fp: &mut Fingerprint, devices: &Devices) {
    fp.count(devices.flares.len());
    for flare in &devices.flares {
        fp.vector(flare.position);
        fp.vector(flare.motion);
        fp.int(flare.age);
        fp.bool(flare.burning());
        fp.f64(flare.intensity());
        fp.int(flare.seed());
        fp.count(flare.puffs.len());
        for puff in &flare.puffs {
            fp.vector(puff.position);
            fp.int(puff.age);
            fp.f64(puff.radius());
            fp.f64(f64::from(puff.opacity()));
        }
    }
    fp.count(devices.chaff.len());
    for chaff in &devices.chaff {
        fp.vector(chaff.position);
        fp.int(chaff.age);
        fp.int(chaff.seed);
        fp.f64(chaff.seconds());
        fp.f64(f64::from(chaff.opacity()));
    }
}

/// One press of the chaff or flare key, and what it did: the counts, the
/// sounds, and which missiles still guide on the player.
fn press(
    s: &mut State,
    command: Command,
    launcher: Launcher,
    fp: &mut Fingerprint,
    seen: &mut BTreeMap<&'static str, u32>,
) {
    // Guiding on the player as the dispensers see it: a seeker that holds
    // the player, or a range fixture without guidance of its own.
    let guiding: Vec<(u32, u8)> = s
        .projectiles
        .iter()
        .filter(|p| {
            p.target == Some(OWN)
                && p.guidance.as_ref().is_none_or(|flight| {
                    flight.enabled && flight.seeker.acquired && flight.seeker.observation.is_some()
                })
        })
        .map(|p| (p.id, p.weapon(s.own().configuration()).seeker.signature))
        .collect();
    let (chaff, flares) = (s.own().chaff, s.own().flares);
    let devices = (s.devices.flares.len(), s.devices.chaff.len());
    s.command(0, command, launcher);
    fp.name(&command);
    fp.int(s.own().chaff);
    fp.int(s.own().flares);
    for sound in s.take_sound_events() {
        fp.name(&sound.kind);
        fp.vector(sound.position);
        fp.bool(sound.arrived);
        fp.bool(sound.own);
    }
    let added = (s.devices.flares.len(), s.devices.chaff.len()) != devices;
    let spent = (s.own().chaff, s.own().flares) != (chaff, flares);
    let label = match (added, spent) {
        (false, _) if !launcher.alive || s.own().hp <= 0 => "Disabled",
        (false, _) => "Empty",
        (true, false) => "Unlimited",
        (true, true) => "Spent",
    };
    *seen.entry(label).or_default() += 1;
    let signature = if command == Command::ReleaseChaff {
        3
    } else {
        2
    };
    for (id, class) in guiding.into_iter().filter(|(_, class)| *class == signature) {
        let still = s
            .projectiles
            .iter()
            .any(|p| p.id == id && p.target == Some(OWN));
        fp.int(id);
        fp.bool(still);
        if added {
            let outcome = match (still, class) {
                (true, _) => "Resisted",
                (false, 3) => "ChaffDecoyed",
                (false, _) => "FlareDecoyed",
            };
            *seen.entry(outcome).or_default() += 1;
        }
    }
}

fn player_countermeasures(probe: &Probe) -> (u64, BTreeMap<&'static str, u32>) {
    let mut fp = Fingerprint::default();
    let mut seen = BTreeMap::new();
    // Infrared missiles for the range's incoming fixture, which always and
    // sometimes follow flares.
    let mut config = configuration(vec![
        (gun(), 200, true),
        (missile("AIM9M.JT", 2, 20_000, 100), 2, false),
        (missile("AIM9M.JT", 2, 20_000, 70), 2, false),
    ]);
    // The dispensers: four of each, chaff 80 and flares 60 percent effective.
    config.ecm.chaff = [4, 80, 0, 0];
    config.ecm.flare = [4, 60, 0, 0];
    let mut s = State::new(config, true).expect("synthetic configuration");
    // Recorded with the full damage model, system faults included.
    s.cheats.damage = crate::cheats::Damage::Realistic;
    let mut l = launcher([0., 1_200., 0.], 0., 0., 600.);
    // Missiles at the player: radar ones that always, sometimes and never
    // follow chaff, and infrared ones that always or sometimes follow flares.
    let shots = [
        (6_000, 7, missile("AIM120.JT", 3, 60_000, 100), -1_500.),
        (6_001, 7, missile("AIM120.JT", 3, 60_000, 50), -500.),
        (6_002, 8, missile("AIM120.JT", 3, 60_000, 0), 500.),
        (6_003, 8, missile("AIM9M.JT", 2, 20_000, 100), 1_000.),
        (6_004, 9, missile("AIM9M.JT", 2, 20_000, 70), 1_500.),
    ];
    for (id, owner, weapon, offset) in &shots {
        let aim = l.position;
        s.projectiles.push(owned_missile(
            *id,
            *owner,
            weapon,
            [*offset, 1_300., 16_000.],
            OWN,
            aim,
            0,
        ));
    }
    let (mut landed, mut burned, mut chaff_seen) = (false, false, false);
    for tick in 0..4_300u64 {
        // The shooters keep their radars on the player.
        s.set_actor_supports([7, 8, 9].map(|owner| ActorSupport {
            owner,
            observation: Some(seeker::Observation {
                id: OWN,
                position: l.position,
                velocity: l.velocity,
                quality: 1.,
                off_axis: 0.,
                range: 16_000.,
            }),
            supported: true,
            radar_position: [0., 1_300., 16_000.],
            radar_emitting: true,
        }));
        if tick == 20 {
            for station in [1, 2, 1] {
                s.own_mut().selected = station;
                s.command(0, Command::Incoming, l);
            }
            s.own_mut().selected = 0;
        }
        let presses: &[Command] = match tick {
            30 => &[Command::ReleaseFlare],
            45 => &[Command::ReleaseChaff],
            60 => &[Command::ReleaseChaff, Command::ReleaseChaff],
            75 => &[
                Command::ReleaseFlare,
                Command::ReleaseFlare,
                Command::ReleaseFlare,
            ],
            90 => &[Command::ReleaseFlare, Command::ReleaseChaff],
            100 => &[Command::ReleaseChaff, Command::ReleaseFlare],
            110 => {
                // Refilled with unlimited ammunition on: nothing is spent.
                s.cheats.unlimited_ammo = true;
                (s.own_mut().chaff, s.own_mut().flares) = (2, 1);
                &[
                    Command::ReleaseChaff,
                    Command::ReleaseFlare,
                    Command::ReleaseChaff,
                ]
            }
            130 => {
                l.alive = false;
                &[Command::ReleaseFlare]
            }
            131 => {
                l.alive = true;
                &[Command::ReleaseFlare]
            }
            // Damage draws from the combat random stream, so the decoy
            // rolls before it are pinned too.
            200 | 220 | 240 | 260 => &[Command::DamagePlayer],
            300 => &[Command::ReleaseChaff],
            _ => &[],
        };
        for command in presses {
            if *command == Command::DamagePlayer {
                s.command(0, *command, l);
                fp.name(command);
            } else {
                press(&mut s, *command, l, &mut fp, &mut seen);
            }
        }
        step(&mut s, false, l, &mut fp, &mut seen);
        record_devices(&mut fp, &s.devices);
        fly(&mut l);
        chaff_seen |= !s.devices.chaff.is_empty();
        if chaff_seen && s.devices.chaff.is_empty() {
            *seen.entry("ChaffExpired").or_default() += 1;
            chaff_seen = false;
        }
        if !landed && s.devices.flares.iter().any(|f| f.position[1] < 2.) {
            landed = true;
            *seen.entry("FlareLanded").or_default() += 1;
        }
        if !burned && s.devices.flares.iter().any(|f| !f.burning()) {
            burned = true;
            *seen.entry("FlareBurnedOut").or_default() += 1;
        }
        if tick % 300 == 299 {
            probe.part(format!("tick {tick}"), fp.value());
        }
    }
    (fp.value(), seen)
}
