//! `--missile-study OUT_DIR --ai`: the same shots against the game's own AI
//! (profile (e) of the study). Each shot builds a fresh Quick Mission world
//! with the player as the shooter and one enemy AI aircraft on weapons hold,
//! so it defends but never attacks. Both are placed at the shot's geometry
//! and held there until the radar and seeker settle; from the trigger on
//! the AI flies, perceives, maneuvers and dispenses as in a mission, and its
//! devices roll the decoy rule through the mission's own device path. The
//! player keeps flying straight and level at 0.9 Mach.
use super::{Altitudes, Aspect, Cell, Shot, length, status_name, sub, uniform, wrap};
use crate::AppResult;
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::{
    ai::controller::Activity,
    attitude::{Vector, dot},
    combat::{
        live::{self, Command, DeviceNote, Event, Ownship},
        missiles::{self, Guidance, Profile, seeker::Status},
    },
};
use tore_world::{
    ai_wings::Preset,
    mission::{MissionSpec, Skill, WingSpec},
    seats::{SeatCommand, SeatId, SeatInput},
    world::{Hooks, Seating, TickOutput, World},
};

const SEAT: SeatId = SeatId(0);
const DT: f64 = 1. / 120.;

pub struct AiSetup {
    /// Single player flies the Standard AI model, a networked mission the
    /// hybrid one.
    pub model: tore_world::ai_wings::AiFlightModel,
    pub theater: String,
    pub shooter: AircraftId,
    pub target: AircraftId,
    pub skill: Skill,
    /// Print the AI target's flight and defence every quarter second.
    pub trace: bool,
}

pub fn parse_skill(s: &str) -> AppResult<Skill> {
    Ok(match s {
        "novice" => Skill::Novice,
        "average" => Skill::Average,
        "experienced" => Skill::Experienced,
        "ace" => Skill::Ace,
        _ => return Err(format!("unknown skill {s}").into()),
    })
}

fn place(flight: &mut tore_sim::flight::State, position: Vector, heading: f64, speed: f64) {
    flight.position = position;
    flight.velocity = [heading.sin() * speed, 0., heading.cos() * speed];
    flight.speed = speed;
    flight.yaw = heading;
    flight.pitch = 0.;
    flight.bank = 0.;
    flight.vertical_speed = 0.;
    flight.roll_rate = 0.;
    flight.pitch_rate = 0.;
}

/// Flies one shot of `cell` against the AI with `seed`.
pub fn fly(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    setup: &AiSetup,
    weapon: &Weapon,
    cell: &Cell,
    seed: u64,
    rules: missiles::Rules,
) -> AppResult<Shot> {
    let profile =
        Profile::for_weapon(weapon).ok_or_else(|| format!("{} has no profile", weapon.source))?;
    let mut spec = MissionSpec::new(&setup.theater, setup.shooter);
    spec.wings[3] = WingSpec {
        aircraft: setup.target,
        count: 1,
        skill: setup.skill,
    };
    spec.preset = Preset::Hold;
    spec.ai_flight_model = setup.model;
    spec.surface_seed = (seed as u32).wrapping_add(1);
    let mut world = World::build(
        &spec,
        resources,
        Seating::SinglePlayer,
        &mut Hooks::default(),
    )?
    .world;
    // Group objectives are applied after the build preset. Isolate this trial
    // from offensive launches using the existing weapons-hold assignment.
    let player_position = world.cockpits[0].flight.position;
    world
        .ai_wings
        .as_mut()
        .ok_or("no AI")?
        .apply_mission_preset(Preset::Hold, player_position);
    world.combat.state.weapon_rules = rules;
    world.combat.state.set_energy_seed(seed);
    let owner = world.combat.own_id();
    // The study missile on the shooter's first missile station.
    {
        let state = &mut world.combat.state;
        let own = state.ownship(owner).ok_or("no ownship")?;
        let mut config = own.configuration().clone();
        let station = config
            .stations
            .iter()
            .position(|s| Profile::for_weapon(&s.weapon).is_some())
            .ok_or("the shooter has no missile station")?;
        config.stations[station].weapon = weapon.clone();
        config.stations[station].count = 2;
        let (side, friendlies) = (own.side, own.friendlies.clone());
        state.remove_ownship(owner);
        let mut own = Ownship::new(owner, side, config, true)?;
        own.friendlies = friendlies;
        own.selected = station;
        own.armed = true;
        state.add_ownship(own)?;
    }
    let enemy = world
        .combat
        .state
        .targets
        .iter()
        .find(|t| t.side != world.combat.state.own().side && t.aircraft == Some(setup.target))
        .map(|t| t.id)
        .ok_or("no enemy aircraft")?;
    // Geometry, jittered per seed as the scripted study does.
    let (shooter_alt, target_alt) = cell.altitudes.feet();
    let start = world.cockpits[0].flight.position;
    let ground = |w: &World, x: f64, z: f64| f64::from(w.terrain.height(x as f32, z as f32));
    let floor = ground(&world, start[0], start[2]);
    let (shooter_alt, target_alt) = match cell.altitudes {
        Altitudes::Down => (floor + shooter_alt, floor + target_alt),
        _ => (floor.max(0.) + shooter_alt, floor.max(0.) + target_alt),
    };
    let shooter_speed = 0.9 * super::sound_fps(shooter_alt);
    let range = cell.band.feet(weapon) * uniform(seed, 1, 0.93, 1.07);
    let side = if uniform(seed, 3, 0., 1.) < 0.5 {
        1.
    } else {
        -1.
    };
    let target_speed = 0.8 * super::sound_fps(target_alt) * uniform(seed, 4, 0.95, 1.05);
    let target_heading = wrap(
        match cell.aspect {
            Aspect::Quarter => side * std::f64::consts::FRAC_PI_4,
            Aspect::Head => std::f64::consts::PI,
            Aspect::Beam => side * std::f64::consts::FRAC_PI_2,
            Aspect::Tail => 0.,
        } + uniform(seed, 2, -10., 10.).to_radians(),
    );
    let mut shooter_at = [start[0], shooter_alt, start[2]];
    let target_at = [start[0], target_alt, start[2] + range];
    let mut out = TickOutput::default();
    let mut step = |world: &mut World, trigger: bool, commands: Vec<SeatCommand>| {
        let input = SeatInput {
            seat: SEAT,
            tick: world.tick(),
            trigger,
            commands,
            ..SeatInput::default()
        };
        world.step(std::slice::from_ref(&input), &mut out)?;
        AppResult::Ok((
            std::mem::take(&mut out.events),
            std::mem::take(&mut out.outcomes),
        ))
    };
    let hold = |world: &mut World, shooter_at: Vector| {
        place(&mut world.cockpits[0].flight, shooter_at, 0., shooter_speed);
        if let Some(f) = world
            .ai_wings
            .as_mut()
            .and_then(|w| w.actor_flight_mut(enemy))
        {
            place(f, target_at, target_heading, target_speed);
        }
    };
    hold(&mut world, shooter_at);
    step(
        &mut world,
        false,
        vec![SeatCommand::Combat(Command::DesignateTarget(enemy))],
    )?;
    let mut locked = false;
    for tick in 0..360 {
        hold(&mut world, shooter_at);
        // The radar needs a look before a contact can be designated.
        let designate = world.combat.state.own_view().designated() != Some(enemy);
        step(
            &mut world,
            false,
            if designate && tick % 15 == 0 {
                vec![SeatCommand::Combat(Command::DesignateTarget(enemy))]
            } else {
                Vec::new()
            },
        )?;
        let l = tore_world::combat::launcher(&world.cockpits[0].flight);
        let view = world.combat.state.own_view();
        let tracked = view.designated() == Some(enemy);
        let seeker_ok = match profile.guidance {
            Guidance::Infrared => view.seeker_tone(l).is_some_and(|t| t.locked),
            _ => view.ownship().sensors.supports(enemy),
        };
        if tracked && seeker_ok {
            locked = true;
            if tick >= 90 {
                break;
            }
        }
    }
    let mut shot = Shot {
        range_ft: range,
        locked_at_launch: locked,
        target_speed_fps: target_speed,
        ..Default::default()
    };
    let _ = world.combat.state.take_device_notes();
    let before: Vec<u32> = world
        .combat
        .state
        .projectiles
        .iter()
        .map(|p| p.id)
        .collect();
    hold(&mut world, shooter_at);
    step(&mut world, true, Vec::new())?;
    step(&mut world, false, vec![SeatCommand::ReleaseTrigger])?;
    let Some(id) = world
        .combat
        .state
        .projectiles
        .iter()
        .find(|p| p.owner == owner && !before.contains(&p.id))
        .map(|p| p.id)
    else {
        shot.outcome = "no-launch";
        shot.status_at_end = world.combat.state.own().release_readiness.label();
        return Ok(shot);
    };
    shot.fired = true;
    let limit = if rules == missiles::Rules::Energy {
        u64::from(missiles::energy_profile::body(weapon).life) + 240
    } else {
        u64::from(weapon.movement.remove_t) * 30 + 240
    };
    let mut miss = f64::INFINITY;
    let mut last_status = Status::Search;
    let mut last_speed = 0.;
    let mut decoyed = false;
    let mut defended = false;
    let mut selected_previous_tick = false;
    let mut inert_reported = false;
    let mut last_yaw: Option<f64> = None;
    let mut previous: Option<(Vector, Vector)> = None;
    for tick in 1..=limit {
        let t = tick as f64 * DT;
        shooter_at[2] += shooter_speed * DT;
        place(&mut world.cockpits[0].flight, shooter_at, 0., shooter_speed);
        let (events, outcomes) = step(&mut world, false, Vec::new())?;
        shot.time_s = t;
        let rolls: Vec<bool> = if rules == missiles::Rules::Energy {
            world
                .combat
                .state
                .take_decoy_rolls()
                .into_iter()
                .filter(|r| r.projectile == id)
                .map(|r| r.decoyed)
                .collect()
        } else {
            world.ai_wings.as_ref().map_or_else(Vec::new, |w| {
                w.decoy_rolls()
                    .iter()
                    .filter(|r| r.projectile == id)
                    .map(|r| r.decoyed)
                    .collect()
            })
        };
        shot.rolls += rolls.len() as u32;
        if rolls.contains(&true) && !decoyed {
            decoyed = true;
            shot.decoyed_at_s = Some(t);
        }

        for note in world.combat.state.take_device_notes() {
            if let DeviceNote::Released(r) = note
                && r.owner == enemy
            {
                if setup.trace {
                    let defense = world
                        .ai_wings
                        .as_ref()
                        .and_then(|w| w.mission().actor(enemy))
                        .and_then(|actor| actor.defense_decision());
                    println!(
                        "missile-study: device {} seed {seed} t={t:.3} release_tick={} kind={:?} defense={:?}",
                        weapon.source,
                        r.tick,
                        r.kind,
                        defense.map(|d| (
                            d.debug.estimated_threat_time_s,
                            d.motion.map(|m| m.maneuver),
                            d.debug.passed
                        )),
                    );
                }
                // Devices of the missile's own class count; the other class
                // (a mixed burst for an unclassified visual missile) is
                // counted apart.
                let flare = r.kind == live::EffectKind::Flare;
                if flare == (profile.guidance == Guidance::Infrared) {
                    shot.devices += 1;
                    shot.first_device_s.get_or_insert(t);
                } else {
                    shot.other_devices += 1;
                }
            }
        }
        let wings = world.ai_wings.as_ref().ok_or("no AI")?;
        if let Some(actor) = wings.mission().actor(enemy) {
            let selected = actor.defense_decision().is_some_and(|d| d.threat_id == id);
            if world.combat.state.ended_missile_threats().contains(&id) {
                if selected {
                    return Err("AI kept defending against a confirmed inert store".into());
                }
                if setup.trace && !inert_reported {
                    println!(
                        "missile-study: inert-defense {} seed {seed} t={t:.3} active_before={selected_previous_tick} cleared=true",
                        weapon.source,
                    );
                }
                inert_reported = true;
            }
            selected_previous_tick = selected;
            let yaw = actor.flight().yaw;
            if setup.trace && tick % 30 == 0 {
                let f = actor.flight();
                println!(
                    "missile-study: trace {} seed {seed} t={t:.2} yaw={:.1} bank={:.1} pitch={:.1} speed={:.0} g={:.2} alt={:.0} activity={:?} defense={:?}",
                    weapon.source,
                    f.yaw.to_degrees(),
                    f.bank.to_degrees(),
                    f.pitch.to_degrees(),
                    f.speed,
                    f.g,
                    f.position[1],
                    actor.activity(),
                    actor.defense_decision().map(|d| (
                        d.motion,
                        d.burst,
                        d.debug.estimated_threat_time_s,
                        d.debug.maneuver_now
                    ))
                );
            }
            shot.target_turn_deg += wrap(yaw - last_yaw.unwrap_or(yaw)).abs().to_degrees();
            last_yaw = Some(yaw);
            defended |= matches!(
                actor.activity(),
                Activity::Defending | Activity::Evading | Activity::Breaking
            );
        }
        let target = world
            .combat
            .state
            .targets
            .iter()
            .find(|t| t.id == enemy)
            .map(|t| (t.position, t.hp));
        let missile = world.combat.state.projectiles.iter().find(|p| p.id == id);
        if let (Some(p), Some((position, _))) = (missile, target) {
            if let Some(energy) = p.motion.and_then(|m| m.energy) {
                if energy.flight.lost {
                    shot.control_loss_s.get_or_insert(t);
                }
                if energy.flight.defeated {
                    shot.defeat_s.get_or_insert(t);
                }
                if energy.tracking.notched
                    && !energy.flight.lost
                    && p.guidance.as_ref().is_some_and(|f| f.enabled)
                {
                    shot.first_notch_s.get_or_insert(t);
                    shot.notched_s += DT;
                }
            }
            let now = (p.position, position);
            if let Some((pm, pt)) = previous {
                let a = sub(pm, pt);
                let b = sub(now.0, now.1);
                let d = sub(b, a);
                let u = (-dot(a, d) / dot(d, d).max(1e-12)).clamp(0., 1.);
                miss = miss.min(length(std::array::from_fn(|i| a[i] + d[i] * u)));
            }
            previous = Some(now);
            last_speed = p.motion.map_or(0., |m| length(m.velocity));
            match &p.guidance {
                Some(flight) => {
                    last_status = flight.seeker.status;
                    if flight.seeker.observation.is_some() {
                        shot.tracked_s += DT;
                        if shot.first_lock_s.is_none() && flight.seeker.acquired {
                            shot.first_lock_s = Some(t);
                        }
                    }
                }
                None => {
                    if !decoyed {
                        decoyed = true;
                        shot.decoyed_at_s = Some(t);
                    }
                    last_status = Status::Unguided;
                }
            }
        }
        if outcomes.iter().any(|o| {
            o.projectile == id
                && matches!(o.resolution, tore_sim::combat::ledger::Resolution::Hit(_))
        }) && events
            .iter()
            .any(|e| matches!(e, Event::Hit(v) if *v == enemy))
        {
            shot.hit = true;
            shot.kill = target.is_some_and(|(_, hp)| hp <= 0)
                && world
                    .combat
                    .state
                    .ledger
                    .credit(enemy)
                    .is_some_and(|k| k.owner == owner);
            break;
        }
        shot.time_s = t;
        if missile.is_none() {
            shot.ground_impact = events.contains(&Event::Ground);
            break;
        }
        // A decoyed missile flies straight on: once it is past and opening it
        // can no longer reach the target, so stop early.
        if let (Some(p), Some(row)) = (
            missile,
            world.combat.state.targets.iter().find(|t| t.id == enemy),
        ) && rules != missiles::Rules::Energy
            && p.guidance.is_none()
            && p.motion.is_some_and(|m| {
                dot(sub(p.position, row.position), sub(m.velocity, row.velocity)) > 0.
            })
            && length(sub(p.position, row.position)) > 3000.
        {
            break;
        }
    }
    shot.miss_ft = miss;
    shot.speed_at_end_fps = last_speed;
    shot.status_at_end = status_name(last_status);
    shot.defended = defended;
    shot.outcome = if shot.hit {
        if decoyed { "hit-after-decoy" } else { "hit" }
    } else if decoyed {
        "decoyed"
    } else if shot.ground_impact {
        "ground"
    } else if matches!(last_status, Status::Expired) {
        "expired"
    } else if matches!(
        last_status,
        Status::Memory | Status::Lost | Status::Search | Status::Midcourse
    ) {
        "seeker-lost"
    } else if last_speed < target_speed {
        "energy"
    } else {
        "guidance-miss"
    };
    Ok(shot)
}
