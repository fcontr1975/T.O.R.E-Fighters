//! The `--combat-smoke` command-line check: scripted ranges, source damage
//! replay and ballistic release, run through `Combat` and the live combat
//! state. It reads `TORE_COMBAT_EVIDENCE` and writes tape files, so it belongs
//! to the app and not to mission state.
use crate::{
    AppResult,
    aircraft_type::AircraftType,
    combat::{Combat, feedback, launcher},
    flight,
    terrain::Terrain,
};
use std::collections::BTreeMap;
use tore_sim::{
    attitude::Basis,
    combat::live::{self, EffectKind, Event, Launcher, OwnshipInput},
};

/// Where two Debug dumps first differ, with a little context from each, so a
/// failed replay comparison says which field moved.
fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &str| {
        let from = s.floor_char_boundary(at.saturating_sub(120));
        let to = s.floor_char_boundary((at + 120).min(s.len()));
        s[from..to].to_owned()
    };
    format!(
        "at byte {at}: replay ...{}... live ...{}...",
        window(a),
        window(b)
    )
}

pub fn smoke(h: &AircraftType, data: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    let world = Terrain::for_theater(data, "UKR")?;
    let mut combat = Combat::new(h, data, true)?;
    // A retained selection station (a store the default load leaves off,
    // which the loadout editor can fill) starts empty and can release
    // nothing, so the default load's stations are what this covers. Read at
    // the start: firing the earlier slots empties the later ones.
    let loaded: Vec<bool> = combat
        .state
        .own()
        .ammo
        .iter()
        .map(|rounds| rounds & 0x7fff != 0)
        .collect();
    wreck_contacts(
        combat.state.own().configuration(),
        &loaded,
        launcher(&h.start(&world)),
    )?;
    println!(
        "systems source {:?}: player capacity={} ECM={:?} weights={} repeat-limited=45",
        h.profile.id,
        combat.state.own().configuration().damage_capacity,
        combat.state.own().configuration().ecm,
        combat
            .state
            .own()
            .configuration()
            .system_damage
            .iter()
            .map(|v| u32::from(v & 15))
            .sum::<u32>()
    );
    let mut damaged = live::State::new(combat.state.own().configuration().clone(), true)?;
    // System faults are part of the check, so exercise Realistic damage.
    damaged.cheats.damage = tore_sim::cheats::Damage::Realistic;
    let l = launcher(&h.start(&world));
    // A source missile followed by gun hits exercises selection on a varied
    // damage history; a particular all-gun seed can legitimately select no fault.
    damaged.own_mut().selected = damaged
        .own()
        .configuration()
        .stations
        .iter()
        .position(|s| s.weapon.source == "AGM65G.JT")
        .or_else(|| {
            damaged
                .own()
                .configuration()
                .stations
                .iter()
                .position(|s| s.weapon.seeker.signature != 0)
        })
        .unwrap_or(0);
    damaged.command(0, live::Command::Incoming, l);
    let mut replica = damaged.clone();
    let mut systems = 0;
    let mut destroyed = 0;
    for _ in 0..1200 {
        let events = damaged.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        if events
            != replica.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            )
        {
            return Err("source incoming damage replay diverged".into());
        }
        systems += events
            .iter()
            .filter(|e| matches!(e, Event::SubsystemDamaged { .. }))
            .count();
        destroyed += events
            .iter()
            .filter(|e| matches!(e, Event::OwnshipDestroyed { .. }))
            .count();
        if damaged.projectiles.is_empty() {
            break;
        }
    }
    // Forty gun-sized hits kill most aircraft; the A-4E's guns are weaker, so
    // keep hitting until the aircraft is destroyed.
    for _ in 0..2000 {
        if damaged.own().hp == 0 {
            break;
        }
        damaged.command(0, live::Command::DamagePlayer, l);
        replica.command(0, live::Command::DamagePlayer, l);
        let events = damaged.step(
            &[OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }],
            |_, _| 0.,
        );
        if events
            != replica.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            )
            || format!("{damaged:?}") != format!("{replica:?}")
        {
            return Err("source damage replay diverged".into());
        }
        systems += events
            .iter()
            .filter(|e| matches!(e, Event::SubsystemDamaged { .. }))
            .count();
        destroyed += events
            .iter()
            .filter(|e| matches!(e, Event::OwnshipDestroyed { .. }))
            .count();
    }
    // Different source weapons produce different damage histories. A fatal
    // missile can legitimately select no subsystem. Cover each source station
    // plus gradual gun damage without changing aircraft damage values or RNG.
    for source in 0..combat.state.own().configuration().stations.len() {
        if systems > 0 {
            break;
        }
        let mut gradual = live::State::new(combat.state.own().configuration().clone(), true)?;
        gradual.cheats.damage = tore_sim::cheats::Damage::Realistic;
        gradual.own_mut().selected = source;
        if source > 0 {
            gradual.command(0, live::Command::Incoming, l);
        }
        let mut replay = gradual.clone();
        for tick in 0..1240 {
            if tick >= 1200 {
                gradual.command(0, live::Command::DamagePlayer, l);
                replay.command(0, live::Command::DamagePlayer, l);
            }
            let events = gradual.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            if events
                != replay.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: l,
                    }],
                    |_, _| 0.,
                )
                || format!("{gradual:?}") != format!("{replay:?}")
            {
                return Err("gradual source damage replay diverged".into());
            }
            systems += events
                .iter()
                .filter(|e| matches!(e, Event::SubsystemDamaged { .. }))
                .count();
        }
    }
    if (systems == 0
        && damaged
            .own()
            .configuration()
            .stations
            .iter()
            .any(|s| s.weapon.seeker.signature != 0))
        || destroyed != 1
        || damaged.own().hp != 0
    {
        return Err(format!("source automatic damage/destruction failed faults={systems} kills={destroyed} HP={} damage={} counts={:?} source={:?}",damaged.own().hp,damaged.own().damage,damaged.own().subsystem_counts,damaged.own().configuration().system_damage).into());
    }
    println!(
        "systems automatic {:?}: faults={systems} destruction={destroyed} indices={:?} PASS",
        h.profile.id,
        damaged.own().subsystem_counts
    );
    for index in 0..combat.state.own().ammo.len() {
        let station = &combat.state.own().configuration().stations[index];
        if !loaded.get(index).copied().unwrap_or(false)
            || (!station.internal && station.weapon.seeker.signature == 0)
        {
            continue;
        }
        for jammer in [false, true] {
            let mut state = live::State::new(combat.state.own().configuration().clone(), true)?;
            state.own_mut().selected = index;
            let mut l = launcher(&h.start(&world));
            l.jammer = jammer;
            state.command(0, live::Command::Incoming, l);
            let mut replay = state.clone();
            let initial = state.own().hp;
            let ammo = state.own().ammo.clone();
            let mut outcome = false;
            let mut mixer = tore_input::FeedbackMixer::default();
            let mut pulses = 0;
            for _ in 0..1200 {
                let events = state.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: l,
                    }],
                    |_, _| 0.,
                );
                if events
                    != replay.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: false,
                            launcher: l,
                        }],
                        |_, _| 0.,
                    )
                    || format!("{state:?}") != format!("{replay:?}")
                {
                    return Err("incoming replay diverged".into());
                }
                for event in &events {
                    if let Some(cue) =
                        feedback(event, state.own().aircraft, state.own().configuration())
                    {
                        mixer.event(cue);
                    }
                    outcome |= matches!(event, Event::OwnshipDamaged { .. } | Event::Defeated(0));
                }
                if matches!(mixer.tick(), Some(tore_input::FeedbackUpdate::Pulse { .. })) {
                    pulses += 1;
                }
                if outcome {
                    break;
                }
            }
            if !outcome || state.own().ammo != ammo || (state.own().hp < initial && pulses == 0) {
                return Err(format!(
                    "incoming lifecycle failed slot {} jammer={jammer}",
                    index + 1
                )
                .into());
            }
            for _ in 0..120 {
                mixer.tick();
            }
            if mixer.tick().is_some() {
                return Err("feedback failed to settle".into());
            }
            println!(
                "systems incoming {:?} slot={} jammer={jammer} HP={initial}->{} haptic-pulses={pulses} PASS",
                h.profile.id,
                index + 1,
                state.own().hp
            );
        }
    }
    // The tape file of the last slot that recorded one. A slot that stops early
    // (a surface weapon refused against the aircraft target) leaves its tape
    // open, and the records that follow go to that file until the next slot's
    // tape replaces it, as they did when combat held the file.
    let mut recorder: Option<crate::tape_file::Recorder> = None;
    for index in 0..combat.state.own().ammo.len() {
        if !loaded.get(index).copied().unwrap_or(false) {
            continue;
        }
        let station = &combat.state.own().configuration().stations[index];
        if !station.internal && station.weapon.seeker.signature == 0 {
            ballistic_smoke(combat.state.own().configuration(), index)?;
            continue;
        }
        for (class, category) in [
            combat.state.own().configuration().target_category,
            0x2000,
            0x100,
            0x400,
            0x40,
        ]
        .into_iter()
        .enumerate()
        {
            let mut flight = h.start(&world);
            let tape = std::env::var_os("TORE_COMBAT_EVIDENCE")
                .filter(|_| class == 0)
                .map(|root| {
                    std::path::PathBuf::from(root).join(format!(
                        "{:?}-slot-{}.tape",
                        h.profile.id,
                        index + 1
                    ))
                });
            if let Some(path) = &tape {
                std::fs::create_dir_all(path.parent().ok_or("tape directory missing")?)?;
                if combat.recording_tape() && recorder.is_some() {
                    write_tape(&mut combat, recorder.as_mut())?;
                }
                recorder = Some(crate::tape_file::Recorder::new(
                    path,
                    data,
                    combat.state.own().configuration(),
                    "UKR",
                )?);
                recorder.as_mut().unwrap().model(&combat.state)?;
                combat.start_tape();
            }
            combat.reset(&mut flight)?;
            // This stationary range fixture starts with an open available bay.
            // Live actuator delay and closed-bay release have separate tests.
            if flight.bay_available() {
                flight.bay = 1.;
                flight.bay_open = true;
            }
            for _ in 0..index {
                combat.command(live::Command::NextWeapon, launcher(&flight));
            }
            combat.state.range_category = category;
            combat.command(live::Command::ReplaceTarget, launcher(&flight));
            // Selection needs a current observation, and a radar weapon track
            // needs half a second of it, so observe before designating.
            observe(&mut combat, &mut flight, &world, 1)?;
            combat.command(live::Command::Designate, launcher(&flight));
            observe(&mut combat, &mut flight, &world, ACQUISITION)?;
            let initial = combat.state.own().ammo[index];
            let mut negative = combat.state.clone();
            let l = launcher(&flight);
            negative.command(0, live::Command::ToggleArm, l);
            negative.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            if negative.own().ammo[index] != initial
                || negative.own_view().readiness(l) != live::Readiness::Safe
            {
                return Err("safe inhibited shot consumed ammunition".into());
            }
            negative.command(0, live::Command::ToggleArm, l);
            negative.command(0, live::Command::FailStation, l);
            let mass = negative.own().payload_lbs();
            negative.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: true,
                    launcher: l,
                }],
                |_, _| 0.,
            );
            if negative.own().rounds(index) != initial || negative.own().payload_lbs() != mass {
                return Err("station failure changed ammunition/mass".into());
            }
            // A surface weapon cannot engage the practice aircraft
            // (docs/spec/missiles.md, target-role rules): the shot is refused,
            // not spent, and there is no surface target to fire it at yet.
            if tore_sim::combat::missiles::Profile::for_weapon(
                &combat.state.own().configuration().stations[index].weapon,
            )
            .is_some_and(|p| p.role == tore_sim::combat::missiles::TargetRole::Surface)
            {
                let mut refused = combat.state.clone();
                refused.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l,
                    }],
                    |_, _| 0.,
                );
                if refused.own_view().readiness(l) != live::Readiness::WrongTarget
                    || refused.own().ammo[index] != initial
                    || !refused.projectiles.is_empty()
                {
                    return Err(format!(
                        "surface weapon was not refused against the aircraft target: slot={} weapon={} readiness={:?}",
                        index + 1,
                        combat.state.own().configuration().stations[index].weapon.source,
                        refused.own_view().readiness(l)
                    )
                    .into());
                }
                println!(
                    "combat smoke {} slot={} {} class={class}: refused against aircraft target PASS",
                    h.profile.name,
                    index + 1,
                    combat.state.own().configuration().stations[index]
                        .weapon
                        .source,
                );
                combat.cancel();
                continue;
            }
            if index != 0 {
                let mut no_target = combat.state.clone();
                // Remove the fixture contact as well as its designation. A
                // bare ClearDesignation may reacquire the same aircraft in
                // boresight during this step, which is a valid launch rather
                // than an undesignated negative case.
                no_target.command(0, live::Command::ClearRange, l);
                no_target.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l,
                    }],
                    |_, _| 0.,
                );
                // Seeker service may validly switch a supported weapon into
                // boresight during the step. The captured release gate, not the
                // stale pre-service readiness, decides whether any debit was legal.
                if !no_target.own().release_readiness.player_missile_may_fire(
                    &no_target.own().configuration().stations[index].weapon,
                ) && no_target.own().ammo[index] != initial
                {
                    return Err("inhibited targetless launch consumed ammo".into());
                }
                let weapon = &combat.state.own().configuration().stations[index].weapon;
                let mut too_close = combat.state.clone();
                too_close.own_mut().launch_mode = tore_sim::combat::missiles::LaunchMode::Cued;
                if weapon.seeker.zones[1].minimum_range > 0
                    && combat.state.own_view().readiness(l) == live::Readiness::Ready
                {
                    // Leave enough margin for the 300 ft/s range target to
                    // advance during the observation refresh below.
                    let distance = f64::from(weapon.seeker.zones[1].minimum_range) - 100.;
                    too_close.targets[0].position =
                        std::array::from_fn(|k| l.position[k] + l.basis.forward[k] * distance);
                    too_close.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: false,
                            launcher: l,
                        }],
                        |_, _| 0.,
                    );
                    let close_reason = too_close.own_view().readiness(l);
                    if close_reason != live::Readiness::Ready {
                        return Err(format!(
                            "close-range launch was inhibited: slot={} minimum={} reason={close_reason:?} mode={:?} designated={:?}",
                            index + 1,
                            weapon.seeker.zones[1].minimum_range,
                            too_close.own().launch_mode,
                            too_close.own_view().designated()
                        ).into());
                    }
                    too_close.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: true,
                            launcher: l,
                        }],
                        |_, _| 0.,
                    );
                    if too_close.own().rounds(index) >= initial {
                        return Err("close-range shot did not consume ammunition".into());
                    }
                }
                let mut tracking = combat.state.clone();
                tracking.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: true,
                        launcher: l,
                    }],
                    |_, _| 0.,
                );
                if tracking.projectiles.is_empty() {
                    return Err(format!(
                        "source guidance probe did not launch: slot={} weapon={} readiness={:?} ammo={} initial={initial} mode={:?}",
                        index + 1,
                        weapon.source,
                        combat.state.own_view().readiness(l),
                        tracking.own().ammo[index],
                        tracking.own().launch_mode
                    )
                    .into());
                }
                tracking.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: Launcher { radar: false, ..l },
                    }],
                    |_, _| 0.,
                );
                let loses_track = weapon.seeker.signature == 3 && weapon.flags & 0x200 != 0;
                if tracking.projectiles.iter().any(|p| {
                    if let Some(guidance) = &p.guidance {
                        p.target.is_none()
                            || (loses_track
                                && !matches!(
                                    guidance.seeker.status,
                                    tore_sim::combat::missiles::seeker::Status::Memory
                                        | tore_sim::combat::missiles::seeker::Status::Lost
                                ))
                    } else {
                        p.target.is_none() != loses_track
                    }
                }) {
                    return Err("radar-off support and retained-identity contract failed".into());
                }
                let mut jettison = combat.state.clone();
                let internal = jettison.own().configuration().stations[index].internal;
                let expected_rounds = if internal { initial } else { 0 };
                let expected_mass = jettison.own().payload_lbs()
                    - if internal {
                        0.
                    } else {
                        weapon.carried_weight_lb() * f64::from(initial)
                    };
                jettison.command(0, live::Command::Jettison, l);
                if jettison.own().rounds(index) != expected_rounds
                    || jettison.own().payload_lbs() != expected_mass
                {
                    return Err("source jettison mass/ammunition contract failed".into());
                }
                if combat.state.own().configuration().stations[index]
                    .weapon
                    .seeker
                    .signature
                    == 3
                {
                    // Passive channel leaves the power switch on but gives no
                    // radar support. Player missiles may still leave the rail.
                    let mut radar_off = combat.state.clone();
                    let off = Launcher { radar: false, ..l };
                    radar_off.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: true,
                            launcher: off,
                        }],
                        |_, _| 0.,
                    );
                    let profile = tore_sim::combat::missiles::Profile::for_weapon(
                        &combat.state.own().configuration().stations[index].weapon,
                    );
                    if profile.is_some() && radar_off.own().ammo[index] != initial - 1 {
                        return Err("unsupported radar missile did not release".into());
                    }
                    // Power off: active missiles search immediately; supported
                    // missiles wait for illumination without permanent disablement.
                    if profile.is_some() {
                        let mut dumb = combat.state.clone();
                        let off = Launcher {
                            radar: false,
                            radar_power: false,
                            ..l
                        };
                        dumb.step(
                            &[OwnshipInput {
                                aircraft: 0,
                                held: true,
                                launcher: off,
                            }],
                            |_, _| 0.,
                        );
                        if dumb.own().ammo[index] != initial - 1
                            || dumb.projectiles.iter().any(|p| {
                                p.guidance.as_ref().is_none_or(|f| {
                                    f.unguided
                                        || (f.profile.guidance
                                            == tore_sim::combat::missiles::Guidance::Active
                                            && !f.enabled)
                                })
                            })
                        {
                            return Err(format!(
                                "radar-power-off release did not preserve seeker search: slot={} ammo={} initial={initial}",
                                index + 1,
                                dumb.own().ammo[index]
                            )
                            .into());
                        }
                    }
                }
            }
            // Replay the same authoritative host tick inputs in a second state.
            // Presentation/pause never calls this path and cannot advance either copy.
            let mut replay = combat.state.clone();
            if let Some(name) = tore_sim::combat::release_sound(
                &combat.state.own().configuration().stations[index].weapon,
            ) {
                let pcm = tore_formats::pcm::Pcm::parse(
                    name,
                    data.get(name).ok_or("missing firing PCM")?,
                )?;
                if !pcm.samples.windows(2).any(|s| s[0] != s[1]) {
                    return Err("firing PCM contains no signal".into());
                }
            }
            for name in ["&EXPL3.5K", "&EXPL12.5K"] {
                tore_formats::pcm::Pcm::parse(name, data.get(name).ok_or("missing impact PCM")?)?;
            }

            let mut fired = 0;
            let mut impacts = 0;
            let mut destroyed = 0;
            let mut collided = false;
            // Pulse for missiles, hold for gun. Two shots are available in the
            // smallest source station; damage remains source class-0 per hit.
            for tick in 0..6000 {
                if tick % 240 == 0 {
                    combat.own_trigger().input.space(true, false, false);
                }
                if index != 0 && tick % 240 == 1 {
                    combat.own_trigger().input.space(false, false, false);
                }
                flight.step(&flight::PilotInput::default(), |x, z| {
                    f64::from(world.height(x as f32, z as f32))
                });
                let replay_events = replay.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: combat.own_trigger().input.held,
                        launcher: launcher(&flight),
                    }],
                    |x, z| f64::from(world.height(x as f32, z as f32)),
                );
                let events = combat.step(&mut flight, &world)?;
                // The host hands a dry station's selection on (see `step`);
                // the second state applies the same rule from the tape.
                if !combat.own_trigger().input.held
                    && replay.own().armed
                    && !replay
                        .own()
                        .carries(replay.own().selected, replay.cheats.unlimited_ammo)
                {
                    replay.command(0, live::Command::AdvanceFromEmpty, launcher(&flight));
                }
                if events != replay_events
                    || combat.state.own().ammo != replay.own().ammo
                    || combat.state.projectiles != replay.projectiles
                    || combat.state.targets != replay.targets
                    || combat.state.history != replay.history
                    || combat.state.effects != replay.effects
                {
                    return Err("combat host replay diverged".into());
                }
                for event in events {
                    match event {
                        Event::Fired { .. } => fired += 1,
                        Event::Hit(_) => impacts += 1,
                        // The fixture flies at the player, so a slow gun can
                        // lose the race and the two aircraft collide; the
                        // collision then zeroes what the rounds had not.
                        Event::OwnshipDestroyed { .. } => collided = true,
                        Event::Destroyed(_) => destroyed += 1,
                        _ => {}
                    }
                }
                if destroyed > 0 || (class != 0 && impacts > 0) {
                    break;
                }
            }
            if fired == 0
                || impacts == 0
                || (class == 0 && destroyed != 1)
                || combat.state.own().ammo[index] >= initial
                || combat.state.history.iter().any(|hit| {
                    hit.class != class
                        || hit.nominal
                            != i32::from(
                                combat.state.own().configuration().stations[index]
                                    .weapon
                                    .damage
                                    .by_class[class],
                            )
                            .max(0)
                })
                || (!collided
                    && combat
                        .state
                        .history
                        .iter()
                        .map(|hit| hit.applied)
                        .sum::<i32>()
                        != combat.state.own().configuration().hit_points
                            - combat.state.targets[0].hp)
                || (class == 0
                    && !combat
                        .state
                        .effects
                        .iter()
                        .any(|e| e.kind == EffectKind::Destroyed))
            {
                let applied: i32 = combat.state.history.iter().map(|hit| hit.applied).sum();
                let wrong_class = combat.state.history.iter().find(|hit| hit.class != class);
                return Err(format!(
                    "combat smoke {} {} failed: fired={fired} hits={impacts} destroyed={destroyed} class={class} ammo={}->{} applied={applied} lost_hp={} wrong_class={:?}",
                    h.profile.name,
                    combat.state.own().configuration().stations[index].weapon.source,
                    initial,
                    combat.state.own().ammo[index],
                    combat.state.own().configuration().hit_points - combat.state.targets[0].hp,
                    wrong_class.map(|hit| (hit.class, hit.nominal, hit.applied))
                )
                .into());
            }
            println!(
                "combat smoke {} slot={} {} class={class}: shots={fired} hits={impacts} destroyed={destroyed} ammo={}->{} effects={} PASS",
                h.profile.name,
                index + 1,
                combat.state.own().configuration().stations[index]
                    .weapon
                    .source,
                initial,
                combat.state.own().ammo[index],
                combat.state.effects.len()
            );
            combat.cancel();
            let ammo = combat.state.own().ammo.clone();
            for _ in 0..120 {
                combat.step(&mut flight, &world)?;
            }
            if combat.state.own().ammo != ammo {
                return Err("firing continued after release".into());
            }
            if let Some(path) = &tape {
                write_tape(&mut combat, recorder.as_mut())?;
                let decoded = crate::tape_file::replay_without_airports(
                    path,
                    data,
                    combat.state.own().configuration().clone(),
                    "UKR",
                    &world,
                )?;
                let (replayed, live_state) =
                    (format!("{decoded:?}"), format!("{:?}", combat.state));
                if replayed != live_state {
                    return Err(format!(
                        "serialized live-fire replay diverged before reset: {}",
                        first_difference(&replayed, &live_state)
                    )
                    .into());
                }
                // Also replay manual state transitions, including a full reset.
                // A slow gun can lose the race with the fixture and collide, and
                // a tape does not record the host turning a crashed flight into
                // a dead player, so the manual commands start from a fresh
                // flight and a fresh combat state.
                flight = h.start(&world);
                combat.reset(&mut flight)?;
                for command in [
                    live::Command::ToggleArm,
                    live::Command::ClearDesignation,
                    live::Command::FailStation,
                    live::Command::Jettison,
                    live::Command::NextWeapon,
                    live::Command::CycleClass,
                    live::Command::DamagePlayer,
                    live::Command::ToggleTargetJammer,
                    live::Command::Incoming,
                    live::Command::ReleaseChaff,
                    live::Command::ReleaseFlare,
                ] {
                    combat.cancel();
                    combat.command(command, launcher(&flight));
                    combat.step(&mut flight, &world)?;
                }
                write_tape(&mut combat, recorder.as_mut())?;
                let decoded = crate::tape_file::replay_without_airports(
                    path,
                    data,
                    combat.state.own().configuration().clone(),
                    "UKR",
                    &world,
                )?;
                let (replayed, live_state) =
                    (format!("{decoded:?}"), format!("{:?}", combat.state));
                if replayed != live_state {
                    return Err(format!(
                        "serialized manual-command replay diverged before reset: {}",
                        first_difference(&replayed, &live_state)
                    )
                    .into());
                }
                combat.reset(&mut flight)?;
                combat.step(&mut flight, &world)?;
                write_tape(&mut combat, recorder.as_mut())?;
                combat.stop_tape();
                let decoded = crate::tape_file::replay_without_airports(
                    path,
                    data,
                    combat.state.own().configuration().clone(),
                    "UKR",
                    &world,
                )?;
                let (replayed, live_state) =
                    (format!("{decoded:?}"), format!("{:?}", combat.state));
                if replayed != live_state {
                    return Err(format!(
                        "serialized combat replay diverged after commands/reset: {}",
                        first_difference(&replayed, &live_state)
                    )
                    .into());
                }
                println!("serialized combat replay {} PASS", path.display());
            }
        }
    }
    if combat.recording_tape() && recorder.is_some() {
        write_tape(&mut combat, recorder.as_mut())?;
        combat.stop_tape();
    }
    Ok(())
}

/// Real imported gun and missile records must collide with a present ownship
/// wreck, without damage or a second kill. This runs in the headless battery.
fn wreck_contacts(
    config: &live::Configuration,
    loaded: &[bool],
    mut input: Launcher,
) -> AppResult<()> {
    use tore_sim::combat::missiles::{Profile, TargetRole};
    // Only the default load's stations: a retained selection station starts empty.
    let carried = |index: usize| loaded.get(index).copied().unwrap_or(false);
    let find = |wanted: &dyn Fn(&live::Station) -> bool| {
        config
            .stations
            .iter()
            .enumerate()
            .position(|(index, s)| carried(index) && wanted(s))
    };
    let gun = find(&|s| live::is_gun(&s.weapon));
    let missile = find(&|s| s.weapon.source == "AIM120.JT").or_else(|| {
        find(&|s| Profile::for_weapon(&s.weapon).is_some_and(|p| p.role == TargetRole::Aircraft))
    });
    input.position = [0., 10000., 0.];
    input.basis = Basis::new(0., 0., 0.);
    input.velocity = [0.; 3];
    input.jammer = false;
    let mut checked = 0;
    for index in gun.into_iter().chain(missile) {
        let mut state = live::State::new(config.clone(), true)?;
        state.own_mut().selected = index;
        input.alive = true;
        input.body_present = true;
        state.command(0, live::Command::Incoming, input);
        state.own_mut().hp = 0;
        input.alive = false;
        let mut hits = 0;
        for _ in 0..2400 {
            for event in state.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: false,
                    launcher: input,
                }],
                |_, _| 0.,
            ) {
                if event == Event::Hit(0) {
                    hits += 1;
                }
                if matches!(
                    event,
                    Event::OwnshipDamaged { .. }
                        | Event::OwnshipDestroyed { .. }
                        | Event::SubsystemDamaged { .. }
                        | Event::Jolt(_)
                ) {
                    return Err(
                        format!("wreck impact caused a second damage event: {event:?}").into(),
                    );
                }
            }
            if state.projectiles.is_empty() {
                break;
            }
        }
        if hits != 1 || !state.projectiles.is_empty() || !state.ledger.kills().is_empty() {
            return Err(format!(
                "wreck collision failed: {} hits={hits}",
                config.stations[index].weapon.source
            )
            .into());
        }
        checked += 1;
    }
    if checked == 0 {
        return Err("no wreck-contact weapon fixtures".into());
    }
    println!("wreck collision: weapons={checked} hits={checked} second_kills=0 PASS");
    Ok(())
}

/// Writes the records combat has collected to the smoke harness's tape file
/// and flushes it.
fn write_tape(
    combat: &mut Combat,
    recorder: Option<&mut crate::tape_file::Recorder>,
) -> AppResult<()> {
    let recorder = recorder.ok_or("missing smoke recorder")?;
    recorder.write_all(combat.take_tape());
    recorder.flush()
}

const ACQUISITION: usize = tore_sim::sensors::track::ACQUISITION_STEPS as usize;

/// Advance the shared sensors without firing, so a scripted probe designates
/// and tracks the same way a player does. It uses the recorded host step, so a
/// tape replays the same observations.
fn observe(
    combat: &mut Combat,
    flight: &mut flight::State,
    world: &Terrain,
    steps: usize,
) -> AppResult<()> {
    for _ in 0..steps {
        combat.step(flight, world)?;
    }
    Ok(())
}

/// Unguided external stores have release/contact checks, not a missile lock or
/// same-altitude interception requirement. Does not claim blast-radius parity.
fn ballistic_smoke(config: &live::Configuration, index: usize) -> AppResult<()> {
    let mut state = live::State::new(config.clone(), true)?;
    state.own_mut().selected = index;
    let l = Launcher {
        position: [0., 500., 0.],
        basis: Basis::new(0., -0.3, 0.),
        speed_fps: 500.,
        velocity: Basis::new(0., -0.3, 0.).forward.map(|v| v * 500.),
        bay_ready: true,
        radar_power: false,
        radar: false,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    let initial = state.own().ammo[index];
    let mut safe = state.clone();
    safe.command(0, live::Command::ToggleArm, l);
    safe.step(
        &[OwnshipInput {
            aircraft: 0,
            held: true,
            launcher: l,
        }],
        |_, _| 0.,
    );
    if safe.own().ammo[index] != initial {
        return Err("safe unguided store released".into());
    }
    let mut failed = state.clone();
    failed.command(0, live::Command::FailStation, l);
    failed.step(
        &[OwnshipInput {
            aircraft: 0,
            held: true,
            launcher: l,
        }],
        |_, _| 0.,
    );
    if failed.own().rounds(index) != initial {
        return Err("failed unguided station released".into());
    }
    let mut replay = state.clone();
    let mut ground = false;
    let mut fired = false;
    for tick in 0..7200 {
        let events = state.step(
            &[OwnshipInput {
                aircraft: 0,
                held: tick == 0,
                launcher: l,
            }],
            |_, _| 0.,
        );
        if events
            != replay.step(
                &[OwnshipInput {
                    aircraft: 0,
                    held: tick == 0,
                    launcher: l,
                }],
                |_, _| 0.,
            )
            || format!("{state:?}") != format!("{replay:?}")
        {
            return Err("unguided release replay diverged".into());
        }
        fired |= events.iter().any(|e| matches!(e, Event::Fired { .. }));
        ground |= events.contains(&Event::Ground);
        if ground {
            break;
        }
    }
    if !fired
        || !ground
        || state.own().ammo[index] >= initial
        || state.projectiles.iter().any(|p| p.target.is_some())
    {
        return Err(format!(
            "unguided release/contact failed {}",
            config.stations[index].weapon.source
        )
        .into());
    }
    state.command(0, live::Command::Jettison, l);
    if state.own().rounds(index) != 0 {
        return Err("unguided jettison failed".into());
    }
    println!(
        "ballistic smoke {:?} {}: safe/failure inhibition, no-lock release, deterministic ground contact and jettison PASS",
        config.aircraft, config.stations[index].weapon.source
    );
    Ok(())
}
