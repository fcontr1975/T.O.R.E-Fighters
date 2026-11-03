//! The bridge half of the strike tactics (docs/spec/ai-strike.md, slice
//! ST-T). The tactics live with each striker
//! ([`tore_sim::ai::strike::Strike`]); this module, before every mission step:
//!
//! - describes each striker's stores to it (what kind each is, its cadence
//!   and fire range), with its bombing class and whether it has a laser
//!   designator of its own;
//! - sets its strike cockpit to what the tactics selected (target, store,
//!   CCIP or LVL, the laser on by hand for a laser store), so the cockpit's
//!   sight, laser and solutions run as a human's do;
//! - works out the solution from that cockpit (the bomb or rocket cue's
//!   impact, the shared release test with the pass's aim and gate, the
//!   guided reach, a seeker's lock) and hands it to the striker.
//!
//! The striker's release then leaves through the shared release
//! ([`super::release`]) for bombs, rockets and guided bombs, and through
//! [`AiWings::release_seeker`] for its seeker missiles. With no striker it
//! costs one scan of the actors and changes nothing.
use super::{AiWings, Chatter};
use crate::comms::journal::WingLine;
use tore_sim::ai::strike::{self, Inputs, LaseInputs, LaseSensor, Solution, Store, StoreKind};
use tore_sim::ai::weapon_service::{Rounds, StationId};
use tore_sim::attitude::{Vector, unit};
use tore_sim::combat::delivery::Warning;
use tore_sim::combat::live::{self, GuidedMode, ReleaseAim, StrikeCockpit, StrikeTarget};
use tore_sim::combat::missiles::{self, Flight, LaunchMode, Motion, seeker};
use tore_sim::combat::sight::SightMode;
use tore_sim::combat::{FallState, ballistics, guided, launch_speed};

/// The anti-radiation and radar seeker signatures (docs/formats/radar.md).
const ANTI_RADIATION: u8 = 4;
const RADAR: u8 = 3;

/// One step's strike work, before the mission steps.
pub(super) fn step(
    wings: &mut AiWings,
    state: &mut live::State,
    ground: &(dyn Fn(f64, f64) -> f64 + Sync),
) {
    let strikers: Vec<u32> = wings
        .mission
        .actors()
        .iter()
        .filter(|a| a.strikes() && a.airfield_phase().is_none())
        .map(|a| a.id())
        .collect();
    state.release_ai_lasers(&strikers);
    // The AI-flown aircraft in a cockpit a human left whose tactics want
    // the laser on this step (John's decision 11; ST-INT).
    let mut kept_lasers = std::collections::BTreeSet::new();
    // Aircraft whose no-laze decision still wants saying on the radio.
    let mut no_laze = Vec::new();
    for id in strikers {
        let Some(config) = wings.configs.get(&id) else {
            continue;
        };
        let Some(actor) = wings.mission.actor(id) else {
            continue;
        };
        let mut stores = strike_stores(config, actor.stations());
        if config.strike_class == tore_sim::models::strike_class::StrikeClass::Helicopter {
            // A helicopter's stand-off needs its guided store's flight time
            // (John, 2026-10-29).
            for store in stores.iter_mut().filter(|st| !st.kind.unguided()) {
                if let Some(station) = config.stations.get(usize::from(store.station.0)) {
                    store.flight = wings
                        .flight_times
                        .entry((
                            station.weapon.source.clone(),
                            state.weapon_rules == missiles::Rules::Energy,
                        ))
                        .or_insert_with(|| flight_times(&station.weapon, state.weapon_rules))
                        .to_owned();
                }
            }
        }
        // The answers a stand-off release outside known rings is planned
        // with (John, 2026-10-29). A helicopter plans with them under the
        // energy rules only: under the others its guided round leaves no
        // farther out than its sight's bar, its usual range.
        let helicopter =
            config.strike_class == tore_sim::models::strike_class::StrikeClass::Helicopter;
        let gunship = config.strike_class == tore_sim::models::strike_class::StrikeClass::Gunship;
        let shots = (!gunship && (!helicopter || state.weapon_rules == missiles::Rules::Energy))
            .then(|| {
                super::strike_reach::StrikerShots::new(
                    &wings.shot_cache,
                    state.weapon_rules,
                    stores.iter().filter_map(|st| {
                        config
                            .stations
                            .get(usize::from(st.station.0))
                            .map(|station| (st.station, st.kind, &station.weapon))
                    }),
                )
            })
            .flatten();
        let shots = strike::Shots(
            shots.map(|table| std::sync::Arc::new(table) as std::sync::Arc<dyn strike::ShotTable>),
        );
        let launcher = crate::combat::launcher(actor.flight());
        let selection = actor.strike().and_then(strike::Strike::selection);
        if let Some(call) = actor
            .strike()
            .and_then(strike::Strike::no_laze)
            .filter(|call| !call.told)
        {
            no_laze.push((id, call.kind == strike::NoLaze::Rtb));
        }
        let side = state
            .targets
            .iter()
            .find(|t| t.id == id)
            .map_or(live::NO_SIDE, |t| t.side);
        let designator = config.sight_sensor().is_some_and(|s| s.laser);
        // A cockpit a human left keeps the human's bomb sight mode.
        let kept = state.kept(id).is_some();
        // The laser: held on the target the tactics name (a laser pass, the
        // hold after the release, a lase duty), else left to the automatic
        // rule (slice ST-L).
        let laser_target = actor.strike().and_then(strike::Strike::laser_target);
        let lasing = laser_target.or_else(|| actor.strike().and_then(strike::Strike::lasing));
        let mut laser_set = actor.strike().is_some_and(strike::Strike::laser_set);
        if let Some(cockpit) = state.ai_cockpit_mut(id, side, config) {
            laser_set = hold_laser(cockpit, kept, laser_target, laser_set);
        }
        if kept && laser_target.is_some() {
            kept_lasers.insert(id);
        }
        let lase = lase_inputs(state, id, side, config, lasing);
        let solution = selection.and_then(|sel| {
            let station = usize::from(sel.station.0);
            let w = &config.stations.get(station)?.weapon;
            let target_standing = state.targets.iter().any(|t| t.id == sel.target && t.hp > 0);
            let cockpit = state.ai_cockpit_mut(id, side, config)?;
            cockpit.target = Some(StrikeTarget::Object(sel.target));
            cockpit.selected = station;
            cockpit.armed = true;
            // The AI never changes the mode of a cockpit a human left.
            if !kept {
                cockpit.delivery_mode = sel.mode;
            }
            // A kept sight that is not pinned takes the AI's target as its
            // track, as an AI striker's own sight does; a human's pin stays
            // where it was put (slice ST-K).
            if kept
                && target_standing
                && let Some(sight) = cockpit.sight.as_mut()
                && !matches!(sight.mode, SightMode::Pinned(_))
                && sight.target() != Some(sel.target)
            {
                sight.track(sel.target);
            }
            let row = state.targets.iter().find(|t| t.id == sel.target)?;
            let (position, velocity, alive) = (row.position, row.velocity, row.hp > 0);
            let emitting = row.radar_emitting;
            // A seeker's flight, or a command-guided round (the AT-2, which
            // flies its own guidance with no projectile target) from the
            // selected station (slice ST-F), or a laser weapon of its own in
            // flight (slice ST-K: a takeover holds its target in the gimbal
            // until impact).
            let guided_in_flight = state.projectiles.iter().any(|p| {
                p.owner == id
                    && ((p.target == Some(sel.target) && p.guidance.is_some())
                        || (sel.kind == StoreKind::Command && p.station == station))
            }) || state
                .guided_flights()
                .any(|(_, f)| f.owner == id && f.kind == missiles::Guidance::Laser);
            // `reach`: whether a guided bomb's glide reaches at all, and its
            // time to the release (the AI's committed-to-release window).
            // `stand_off`: whether the store would reach from here whatever
            // the AI's own release range, and a guided bomb's bar.
            let mut stand_off = (false, None);
            let (impact, seconds, warning, due, reach) = if sel.kind == StoreKind::Gun && gunship {
                // The gunship's guns are trained and fired by their own
                // bridge (`super::gunship`, slice ST-F).
                (None, None, false, false, (true, None))
            } else if sel.kind == StoreKind::Gun {
                // The gun's ground cue (slice ST-T2); the striker judges its
                // own bursts.
                let mount = config.stations[station].gun_mount();
                let cue = tore_sim::combat::gunsight::ground_cue(w, &launcher, mount, ground)
                    .ok()
                    .flatten();
                (
                    cue.map(|c| c.0),
                    cue.map(|c| c.1),
                    false,
                    false,
                    (true, None),
                )
            } else if sel.kind.seeker() {
                let lock = seeker_lock(
                    sel.kind,
                    w,
                    config.stations[station].mount,
                    &launcher,
                    position,
                    emitting,
                );
                stand_off = (lock.locked, None);
                (None, None, false, lock.due(), (true, None))
            } else {
                // A laser bomb the tactics let go by hand is solved on the
                // bomb equation (slice ST-G).
                let ballistic = sel.kind == StoreKind::Bomb && guided_kind(w);
                // The sight predicts from the pylon the next store leaves
                // (B3 follow-up), as the release will put it.
                let mount = wings
                    .next_release_point(id, station)
                    .unwrap_or(config.stations[station].mount);
                let delivery =
                    state.strike_delivery(id, config, &launcher, mount, ballistic, &ground);
                // The sight's cue as the release test judges it: the pipper
                // where the path meets the terrain, or in LVL the path at the
                // target's ground (slices ST-LV and ST-G).
                let cue = delivery
                    .as_ref()
                    .and_then(|d| d.cue(ground(position[0], position[2])));
                let warning = delivery.as_ref().is_some_and(|d| {
                    d.warnings
                        .iter()
                        .any(|w| matches!(w, Warning::Low | Warning::Frag))
                });
                let aim = ReleaseAim {
                    offset_ft: sel.aim_ft,
                    gate_ft: sel.gate_ft,
                    ballistic,
                };
                let sight = (guided_kind(w) && !ballistic)
                    .then(|| state.strike_guided_sight(id, config, &launcher, &ground))
                    .flatten();
                stand_off = sight.as_ref().map_or((false, None), |sight| {
                    (
                        sight.in_reach && sight.advisory.is_none(),
                        sight.release_range_ft(),
                    )
                });
                let (due, reach_ok) = match &sight {
                    // A guided bomb: due on its sight's range cue; and whether
                    // its glide can reach from here at all (it cannot from
                    // too high or too slow).
                    Some(sight) => (live::guided_due(sight), sight.reach.is_some()),
                    None => (
                        state.strike_release_due(id, config, &launcher, mount, aim, &ground),
                        true,
                    ),
                };
                // A guided bomb's time to its release (T.O.R.E, John,
                // 2026-10-28): the level T's own, else the distance to the
                // T's bar over the ground speed.
                let release_seconds = sight.as_ref().and_then(|sight| {
                    sight.release_seconds().or_else(|| {
                        let speed = launcher.velocity[0].hypot(launcher.velocity[2]);
                        sight
                            .along_ft()
                            .filter(|_| speed > 1.)
                            .map(|along| along / speed)
                    })
                });
                (
                    cue.map(|c| c.position),
                    cue.map(|c| c.seconds),
                    warning,
                    due,
                    (reach_ok, release_seconds),
                )
            };
            Some(Solution {
                station: sel.station,
                target: sel.target,
                target_position: position,
                target_velocity: velocity,
                target_alive: alive,
                impact,
                impact_seconds: seconds,
                due,
                warning,
                guided_in_flight,
                reach_ok: reach.0,
                release_seconds: reach.1,
                reaches: stand_off.0,
                bar_ft: stand_off.1,
            })
        });
        let inputs = Inputs {
            class: config.strike_class,
            stores,
            designator,
            mission_height_ft: wings.mission_height_ft,
            solution,
            lase,
            shots,
        };
        if let Some(actor) = wings.mission.actor_mut(id) {
            actor.set_strike_inputs(inputs);
            actor.set_laser_set(laser_set);
        }
    }
    // A helicopter's threat decision (slice H4): a text line to its lead,
    // in the wing-line style ("Threat on target, holding off").
    let heli_calls: Vec<u32> = wings
        .mission
        .actors()
        .iter()
        .filter(|a| {
            a.strike()
                .and_then(|s| s.threat_call())
                .is_some_and(|c| !c.told)
        })
        .map(|a| a.id())
        .collect();
    for id in heli_calls {
        let Some(decision) = wings
            .mission
            .actor_mut(id)
            .and_then(|actor| actor.told_threat_call())
        else {
            continue;
        };
        use tore_sim::ai::helicopter_threat::Decision;
        let line = match decision {
            Decision::HoldOff => WingLine::HoldingOff,
            Decision::Resume => WingLine::Resuming,
            Decision::Retarget => WingLine::SwitchingTargets,
            Decision::Home => WingLine::DefendedRtb,
            Decision::Leave => WingLine::LeavingRing,
        };
        wings.chat(Chatter::Line { speaker: id, line });
    }
    for (id, rtb) in no_laze {
        if wings
            .mission
            .actor_mut(id)
            .is_some_and(|actor| actor.told_no_laze())
        {
            wings.chat(Chatter::NoLaze { speaker: id, rtb });
        }
    }
    // Every AI-flown cockpit with a laser weapon selected and armed runs the
    // laser's automatic rule as a player's does, striker or not (a kept
    // cockpit, an AI striker's own): the spot is up before the release.
    for (id, config) in &wings.configs {
        let armed = state.ownship(*id).is_none()
            && state
                .strike_cockpit(*id)
                .is_some_and(|c| c.laser_weapon_armed(config));
        state.set_ai_laser_armed(*id, armed);
        let ai_on = state.ownship(*id).is_none() && kept_lasers.contains(id);
        state.set_ai_laser_on(*id, ai_on);
    }
}

/// A guided bomb or missile whose reach the cockpit's guided sight works out.
fn guided_kind(w: &tore_formats::weapons::Weapon) -> bool {
    guided::kind(w).is_some()
}

/// Holds an AI cockpit's laser on `target`, or lets it go: the sight tracks
/// the object and, in the AI's own cockpit, the laser is on by hand while
/// the tactics want a spot (the pass of a laser store, the hold after its
/// release, a lase duty) and off again when they do not (`set`, which it
/// returns the new value of). In a cockpit a human left the AI never writes
/// the human's hand setting (John's decision 11; ST-INT): its want goes to
/// combat as [`live::State::set_ai_laser_on`] instead, a human's hand on
/// lases anyway and a hand off stays off, and the human finds the setting
/// as left.
fn hold_laser(cockpit: &mut StrikeCockpit, kept: bool, target: Option<u32>, set: bool) -> bool {
    match target {
        Some(id) => {
            if cockpit.target != Some(StrikeTarget::Object(id)) {
                cockpit.target = Some(StrikeTarget::Object(id));
            }
            if let Some(sight) = cockpit.sight.as_mut()
                && sight.target() != Some(id)
            {
                sight.track(id);
            }
            if kept {
                return false;
            }
            if cockpit.laser.manual != Some(true) {
                cockpit.laser.manual = Some(true);
                return true;
            }
            set
        }
        None => {
            if cockpit.laser.manual == Some(true) && !kept {
                cockpit.laser.manual = None;
            }
            false
        }
    }
}

/// What the tactics read of the laser: the designator, the mask last step,
/// the aircraft's own laser weapons in flight, weapons guiding on its spot,
/// the objects other aircraft's spots of its side lie on, and where the lased
/// target stands.
fn lase_inputs(
    state: &live::State,
    id: u32,
    side: live::Side,
    config: &live::Configuration,
    lasing: Option<u32>,
) -> LaseInputs {
    let cockpit = state.strike_cockpit(id);
    let sensor = config
        .sight_sensor()
        .filter(|s| s.laser)
        .map(|s| LaseSensor {
            gimbal: s.gimbal,
            reach_ft: cockpit
                .map(|c| c.laser.limit_ft)
                .filter(|reach| *reach > 0.)
                .unwrap_or(s.range_ft),
        });
    let own_flight = state.guided_flights().any(|(_, f)| {
        f.owner == id && f.kind == missiles::Guidance::Laser && f.mode != GuidedMode::Blind
    });
    let on_spot = state.guided_flights().any(|(_, f)| {
        f.spot_owner == Some(id)
            && f.kind == missiles::Guidance::Laser
            && f.mode == GuidedMode::Guiding
    });
    let mut spot_targets: Vec<u32> = if side == live::NO_SIDE {
        Vec::new()
    } else {
        state
            .laser_spots()
            .into_iter()
            .filter(|spot| spot.owner != id && spot.side == side)
            .filter_map(|spot| spot.target)
            .collect()
    };
    spot_targets.sort_unstable();
    spot_targets.dedup();
    LaseInputs {
        sensor,
        mask: cockpit.and_then(|c| c.laser.mask),
        own_flight,
        on_spot,
        spot_targets,
        target: lasing.and_then(|id| {
            state
                .targets
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.position, t.hp > 0))
        }),
    }
}

/// A striker's stores as the tactics see them: [`describe`], except that
/// the gunship's guns are its linked guns as its own bridge trains them
/// (slice ST-F), not the strafing gun (slice ST-T2).
pub(super) fn strike_stores(
    config: &live::Configuration,
    specs: &[tore_sim::ai::mission::StationSpec],
) -> Vec<Store> {
    let mut stores = describe(config, specs);
    if config.strike_class == tore_sim::models::strike_class::StrikeClass::Gunship {
        stores.retain(|s| s.kind != StoreKind::Gun);
        stores.extend(super::gunship::gun_stores(config, specs));
    }
    stores
}

/// What each of a striker's ground stores is to the tactics: its gun too
/// (slice ST-T2); air-to-air stores are left out.
pub(super) fn describe(
    config: &live::Configuration,
    specs: &[tore_sim::ai::mission::StationSpec],
) -> Vec<Store> {
    config
        .stations
        .iter()
        .enumerate()
        .filter_map(|(index, station)| {
            let w = &station.weapon;
            let gun = live::is_gun(w);
            let kind = if gun { StoreKind::Gun } else { kind(w)? };
            let spec = specs.iter().find(|s| usize::from(s.station.0) == index)?;
            let rounds = match spec.store.rounds {
                Rounds::Finite(n) => n,
                Rounds::Unlimited => u32::MAX,
            };
            let zone = w.seeker.zones[1];
            let period =
                u64::from(w.burst.game_burst_t.max(1)) * tore_sim::ai::QUARTER_SECOND_TICKS;
            // A gun fires its rounds evenly through the record's period, the
            // AI gun employment's cadence (docs/spec/ai-gun-employment.md).
            let cadence_ticks = if gun {
                let rounds = u64::from(w.burst.actual_rounds_per_game.max(1))
                    * u64::from(w.burst.game_rounds_in_burst.max(1));
                (period / rounds).max(1)
            } else {
                period
            };
            Some(Store {
                station: StationId(index as u8),
                kind,
                rounds: if spec.store.inhibited { 0 } else { rounds },
                cadence_ticks,
                fire_range_ft: strike::GUIDED_CLOSE_FT.max(f64::from(zone.maximum_range) / 2.),
                minimum_range_ft: f64::from(zone.minimum_range.max(0)),
                maximum_range_ft: f64::from(zone.maximum_range.max(0)),
                round_speed_fps: (kind == StoreKind::Rocket)
                    .then(|| f64::from(w.movement.maximum_speed)),
                bomb_body: tore_sim::combat::bomb_flight::Body::of(w),
                release_speed_share: match w.movement.launch_retard {
                    0 => 1.,
                    retard => f64::from(retard) / 100.,
                },
                fuze_arm_s: f64::from(w.damage.fuze_arm_t) / 4.,
                frag_radius_ft: f64::from(w.damage.collateral_radius.max(0)),
                falls_free: kind == StoreKind::Laser && guided::is_glide_bomb(w),
                flight: None,
            })
        })
        .collect()
}

/// The shares of a guided store's zone-1 range its flight time is worked
/// out at ([`flight_times`]).
const FLIGHT_SHARES: [f64; 4] = [0.25, 0.5, 0.75, 1.];
/// The launch a flight time is worked out for: this high over a target on
/// flat ground at sea level (the stand-off's masked height and its unmask
/// climb), level, at this speed (about a helicopter's strike speed);
/// fitted, agent.
const FLIGHT_LAUNCH_AGL_FT: f64 = 700.;
const FLIGHT_LAUNCH_FPS: f64 = 120. * tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT;
/// The target's centre over the ground, feet (a vehicle's).
const FLIGHT_TARGET_FT: f64 = 8.;

/// A guided store's time of flight to a ground target at
/// [`FLIGHT_SHARES`] of its zone-1 range (a helicopter's stand-off
/// advance, docs/spec/ai-strike.md, "Helicopter threat awareness"; John,
/// 2026-10-29): under the energy rules the energy model's flight
/// ([`live::State::energy_trial`], the question the AI's guided sight
/// asks), a range it does not reach taken at the pace of the last one it
/// does; else, or with none reached, the range at the record's top speed.
/// `None` with no zone-1 range or speed.
pub(super) fn flight_times(
    w: &tore_formats::weapons::Weapon,
    rules: missiles::Rules,
) -> Option<strike::FlightTimes> {
    let zone = f64::from(w.seeker.zones[1].maximum_range.max(0));
    let top = f64::from(w.movement.maximum_speed.max(0));
    if zone <= 0. {
        return None;
    }
    let energy = |range: f64| {
        let launcher = live::Launcher {
            radar_power: true,
            position: [0., FLIGHT_LAUNCH_AGL_FT, 0.],
            basis: tore_sim::attitude::Basis::new(0., 0., 0.),
            speed_fps: FLIGHT_LAUNCH_FPS,
            velocity: [0., 0., FLIGHT_LAUNCH_FPS],
            bay_ready: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Default::default(),
        };
        let target = [0., FLIGHT_TARGET_FT, range];
        let observed = seeker::Observation {
            id: u32::MAX,
            position: target,
            velocity: [0.; 3],
            quality: 1.,
            range: (range * range + FLIGHT_LAUNCH_AGL_FT * FLIGHT_LAUNCH_AGL_FT).sqrt(),
            off_axis: 0.,
        };
        live::State::energy_trial(w, launcher, observed, [0.; 3], &|_, _| 0.)
            .filter(|answer| answer.hit)
            .map(|answer| answer.seconds)
    };
    let mut points = [(0., 0.); 4];
    let mut pace: Option<f64> = None;
    for (point, share) in points.iter_mut().zip(FLIGHT_SHARES) {
        let range = share * zone;
        let seconds = (rules == missiles::Rules::Energy)
            .then(|| energy(range))
            .flatten()
            .or_else(|| pace.map(|p| p * range))
            .or_else(|| (top > 0.).then(|| range / top))?;
        pace = Some(seconds / range);
        *point = (range, seconds);
    }
    Some(strike::FlightTimes { points })
}

/// A store's kind, or `None` for a gun or an air-to-air store.
pub(super) fn kind(w: &tore_formats::weapons::Weapon) -> Option<StoreKind> {
    if live::is_gun(w) || w.flags & 0x20000 == 0 {
        return None;
    }
    if let Some(guidance) = guided::kind(w) {
        return Some(match guidance {
            missiles::Guidance::Laser => StoreKind::Laser,
            missiles::Guidance::Coordinate => StoreKind::Coordinate,
            _ => StoreKind::Command,
        });
    }
    if ballistics::is_bomb(w) {
        return Some(StoreKind::Bomb);
    }
    if ballistics::is_pod(w) && w.seeker.signature == 0 {
        return Some(StoreKind::Rocket);
    }
    match w.seeker.signature {
        0 => None,
        ANTI_RADIATION => Some(StoreKind::AntiRadiation),
        RADAR => Some(StoreKind::RadarAgm),
        _ => Some(StoreKind::Seeker),
    }
}

/// A seeker missile's lock on the target (docs/formats/weapons.md, "AI
/// release against ground targets"): inside zone 1 from its mount, and for
/// an anti-radiation missile a target that emits; and whether that lies
/// within the AI's fire range (10,000 ft or half the zone-1 maximum,
/// whichever is farther), where it is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SeekerLock {
    locked: bool,
    in_fire_range: bool,
}

impl SeekerLock {
    fn due(self) -> bool {
        self.locked && self.in_fire_range
    }
}

fn seeker_lock(
    kind: StoreKind,
    w: &tore_formats::weapons::Weapon,
    mount: Vector,
    launcher: &live::Launcher,
    target: Vector,
    emitting: bool,
) -> SeekerLock {
    let zone = w.seeker.zones[1];
    let basis = launcher.basis;
    let origin: Vector = std::array::from_fn(|i| {
        launcher.position[i]
            + basis.right[i] * mount[0]
            + basis.up[i] * mount[1]
            + basis.forward[i] * mount[2]
    });
    let range = missiles::length(missiles::sub(target, origin));
    let fire_range = strike::GUIDED_CLOSE_FT.max(f64::from(zone.maximum_range) / 2.);
    SeekerLock {
        locked: missiles::geometry(&zone, origin, basis, target, None)
            && (kind != StoreKind::AntiRadiation || emitting),
        in_fire_range: range <= fire_range,
    }
}

/// What an AI aircraft can do on a flight lead's ground order now (slice
/// ST-O).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Capability {
    /// It has a store that can attack a ground target (every bombing class
    /// strikes since slice ST-F): a refusal "no usable surface store"
    /// otherwise.
    pub strike: bool,
    /// It carries a laser designator of its own and flies the lasing hold
    /// ([`AiWings::can_lase`]), so it takes "Lase my target".
    pub designator: bool,
}

impl AiWings {
    /// What AI aircraft `id` can do on a ground order. All false for an
    /// aircraft without a combat configuration. A laser store counts with a
    /// spot assured, slice ST-L's rule: its own designator, a designator in
    /// its wing, or `spot_on` (a same-side spot on the ordered object; the
    /// rule moved here at the integration, ST-INT).
    pub(super) fn capability(&self, id: u32, spot_on: bool) -> Capability {
        let (Some(config), Some(actor)) = (self.configs.get(&id), self.mission.actor(id)) else {
            return Capability::default();
        };
        let designator = config.sight_sensor().is_some_and(|s| s.laser);
        let identity = actor.identity();
        let wing_designator = self.mission.actors().iter().any(|mate| {
            mate.id() != id
                && mate.alive()
                && mate.identity().side == identity.side
                && mate.identity().wing == identity.wing
                && self.can_lase(mate.id())
        });
        // Every bombing class strikes (slice ST-F); the gunship with its
        // linked guns.
        let stores = strike_stores(config, actor.stations());
        let spot_assured = designator || wing_designator || spot_on;
        Capability {
            strike: strike::has_usable_store(&stores, spot_assured, config.strike_class),
            designator: self.can_lase(id),
        }
    }

    /// Whether AI aircraft `id` carries a store that could attack a ground
    /// target, assuming a laser spot is found for a laser store (slice H3):
    /// the test of who may take the Strike role. All false for an aircraft
    /// without a combat configuration. See [`strike::has_usable_store`].
    pub fn armed_for_ground(&self, id: u32) -> bool {
        let (Some(config), Some(actor)) = (self.configs.get(&id), self.mission.actor(id)) else {
            return false;
        };
        let stores = strike_stores(config, actor.stations());
        strike::has_usable_store(&stores, true, config.strike_class)
    }

    /// AI aircraft `id` carries a laser designator (its own, or a pod) and
    /// flies the lasing hold, so it can lase for another's weapon (slice
    /// ST-L). Helicopters and the gunship do not: their families (slice
    /// ST-F) fly no hold, and of them only the V-22 carries a laser (the
    /// AC-130's sight is a FLIR); agent decision at the integration.
    pub fn can_lase(&self, id: u32) -> bool {
        self.configs.get(&id).is_some_and(|c| {
            !matches!(
                c.strike_class,
                tore_sim::models::strike_class::StrikeClass::Helicopter
                    | tore_sim::models::strike_class::StrikeClass::Gunship
            ) && c.sight_sensor().is_some_and(|s| s.laser)
        })
    }

    /// Orders AI aircraft `id` to lase ground object `target` for as long as
    /// `until` says (docs/spec/ai-strike.md, "Lasing"; the wing order "Lase
    /// my target" calls this). It flies an offset turn of radius max(3 nm,
    /// 1.2 turn radii) about the target above the release floor, with the
    /// spot on it, so a same-side laser weapon, a player's or an AI's, can
    /// home on it; the player's readout names the aircraft as the buddy. The
    /// duty ends at `until`, when the target dies, when the aircraft has to
    /// defend, or at [`Self::cancel_lase`]. An aircraft with no ground
    /// picture is ordered the target to make one; the order is cleared with
    /// the duty. Returns false for an aircraft that is not an AI aircraft
    /// with a designator.
    pub fn order_lase(&mut self, id: u32, target: u32, until: strike::LaseUntil) -> bool {
        if !self.can_lase(id) {
            return false;
        }
        self.mission
            .actor_mut(id)
            .is_some_and(|actor| actor.lase_for(target, until))
    }

    /// Ends AI aircraft `id`'s lase duty. Returns whether one ended.
    pub fn cancel_lase(&mut self, id: u32) -> bool {
        self.mission
            .actor_mut(id)
            .is_some_and(|actor| actor.stop_lase())
    }

    /// The callsign the readout names AI aircraft `id` by, "Friendly 2-1":
    /// the buddy spot's owner on a player's targeting page (slice ST-L).
    pub fn callsign(&self, id: u32) -> Option<String> {
        self.slot(id).map(|slot| slot.label())
    }

    /// The object AI aircraft `id` is lasing, if it is.
    pub fn lasing_for(&self, id: u32) -> Option<u32> {
        self.mission.actor(id).and_then(|actor| actor.lasing())
    }

    /// The mission's airborne height, feet (the Quick Mission altitude
    /// setting), for a heavy striker's release floor; the host sets it
    /// before every step.
    pub fn set_mission_height(&mut self, height_ft: Option<f64>) {
        self.mission_height_ft = height_ft;
    }

    /// The surface radars painting aircraft this tick, as (aircraft,
    /// emitter) pairs (`SurfaceState::painting`); the host sets it before
    /// every step (slice H4).
    pub fn set_painting(&mut self, painting: Vec<(u32, u32)>) {
        self.painting = painting;
    }

    /// Whether `event` is a striker's gun burst at its strike target (a gun
    /// run, slice ST-T2), which [`Self::release_strafe`] makes.
    pub(super) fn striker_gun(&self, event: &tore_sim::ai::mission::LaunchEvent) -> bool {
        self.mission
            .actor(event.actor)
            .and_then(|a| a.strike())
            .and_then(strike::Strike::selection)
            .is_some_and(|sel| {
                sel.kind == StoreKind::Gun
                    && sel.station == event.station
                    && sel.target == event.target
            })
    }

    /// One round of a striker's gun run (slice ST-T2): fired along the
    /// barrel from the gun's mount as every AI gun round is (the round, its
    /// tracer and its debit as [`Self::realise`] makes them), with no air
    /// target to track: the striker flew the gun's ground cue to the target
    /// itself. The round is laid on the burst's walked aim (slice ST-G):
    /// the barrel's line turned by the angle between the cue and the target
    /// (and its lead), within the pilot's fine-tracking reach, plus the
    /// burst's pointing offset, which walks in onto the target. Returns how
    /// many it made (0 or 1).
    pub(super) fn release_strafe(
        &mut self,
        event: &tore_sim::ai::mission::LaunchEvent,
        projectiles: &mut Vec<live::Projectile>,
        targets: &[live::Target],
        weapon: &tore_formats::weapons::Weapon,
        ordinal: u64,
        ground: &(dyn Fn(f64, f64) -> f64 + Sync),
    ) -> u32 {
        let Some(actor) = self.mission.actor(event.actor) else {
            return 0;
        };
        if !actor.alive() || projectiles.len() >= live::MAX_PROJECTILES {
            self.dropped_launches += 1;
            return 0;
        }
        let flight = actor.flight();
        let basis = tore_sim::attitude::Basis::new(flight.yaw, flight.pitch, flight.bank);
        let mount = actor
            .stations()
            .iter()
            .find(|s| s.station == event.station)
            .map_or([0.; 3], |s| s.mount);
        let origin: Vector = std::array::from_fn(|i| {
            flight.position[i]
                + basis.right[i] * mount[0]
                + basis.up[i] * mount[1]
                + basis.forward[i] * mount[2]
        });
        let Ok(speed) = launch_speed(&weapon.movement, (flight.speed * 256.) as i32) else {
            self.dropped_launches += 1;
            return 0;
        };
        let launched = (self.mission.tick() / 30) as u16;
        if !self
            .mission
            .actor_mut(event.actor)
            .is_some_and(|a| a.debit_gun_round(event.station))
        {
            return 0;
        }
        let direction = self.strafe_direction(event, targets, weapon, mount, origin, ground);
        let id = self.next_projectile_id;
        self.next_projectile_id = self.next_projectile_id.wrapping_add(1);
        projectiles.push(live::Projectile {
            id,
            owner: event.actor,
            weapon: Some(weapon.clone()),
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 0,
            incoming: None,
            station: usize::from(event.station.0),
            position: origin,
            previous: origin,
            direction,
            speed_f8: speed * 256,
            launched_t: launched,
            target: None,
            fall: FallState::default(),
            gun_round: Some(
                (ordinal % u64::from(weapon.burst.actual_rounds_per_game.max(1))) as u8,
            ),
            tracer: ordinal.is_multiple_of(3),
        });
        self.ai_shots.insert(id, event.actor);
        self.realised_launches += 1;
        1
    }

    /// The direction a striker's gun round leaves in (slice ST-G): the
    /// barrel's, turned toward the burst's walked aim. The turn is the
    /// angle, seen from the muzzle, from the gun's ground cue to the target
    /// (with its lead), held to the pilot's fine-tracking reach
    /// ([`strike::GUN_TRACK_DEG`]), plus the angle from the target to the
    /// walked aim (the burst's offset, walking in to nothing over the
    /// skill's walk-in time). Unturned when the cue, the target or the strike
    /// cannot be had.
    fn strafe_direction(
        &self,
        event: &tore_sim::ai::mission::LaunchEvent,
        targets: &[live::Target],
        weapon: &tore_formats::weapons::Weapon,
        mount: Vector,
        origin: Vector,
        ground: &(dyn Fn(f64, f64) -> f64 + Sync),
    ) -> Vector {
        let actor = self.mission.actor(event.actor);
        let flight = actor.map(|a| a.flight());
        let barrel = flight.map_or([0., 0., 1.], |f| {
            tore_sim::attitude::Basis::new(f.yaw, f.pitch, f.bank).forward
        });
        let (Some(actor), Some(flight)) = (actor, flight) else {
            return barrel;
        };
        let Some(row) = targets.iter().find(|t| t.id == event.target && t.hp > 0) else {
            return barrel;
        };
        let Some(strike) = actor.strike() else {
            return barrel;
        };
        let launcher = crate::combat::launcher(flight);
        let Ok(Some((cue, _, _))) =
            tore_sim::combat::gunsight::ground_cue(weapon, &launcher, mount, ground)
        else {
            return barrel;
        };
        let level = actor.experience().level;
        let walked = strike.burst_aim(self.mission.tick(), level);
        let lead = strike.lead();
        // The target where the rounds will find it, and the walked aim.
        let target = [
            row.position[0] + lead[0],
            row.position[1],
            row.position[2] + lead[1],
        ];
        let aim = [target[0] + walked[0], target[1], target[2] + walked[1]];
        let line = |to: Vector| unit(missiles::sub(to, origin));
        let (to_cue, to_target, to_aim) = (line(cue), line(target), line(aim));
        // Fine tracking: the cue-to-target turn, held to the reach.
        let mut track = missiles::sub(to_target, to_cue);
        let reach = strike::GUN_TRACK_DEG.to_radians();
        let size = missiles::length(track);
        if size > reach {
            track = track.map(|v| v * reach / size);
        }
        let pointing = missiles::sub(to_aim, to_target);
        let turned: Vector = std::array::from_fn(|i| barrel[i] + track[i] + pointing[i]);
        let length = missiles::length(turned);
        if length.is_finite() && length > 0. {
            turned.map(|v| v / length)
        } else {
            barrel
        }
    }

    /// Forces AI aircraft `id`'s pass and store (the headless driver's
    /// coverage runs, docs/testing/lane-flight.md). Returns false for an id
    /// that is not a striker.
    pub fn force_strike(
        &mut self,
        id: u32,
        pass: Option<strike::Pass>,
        station: Option<usize>,
    ) -> bool {
        match self.mission.actor_mut(id) {
            Some(actor) if actor.strikes() => {
                actor.force_strike(pass, station.map(|s| StationId(s as u8)));
                true
            }
            _ => false,
        }
    }

    /// Whether `event` is a striker's seeker missile at its strike target,
    /// which [`Self::release_seeker`] makes.
    pub(super) fn striker_seeker(
        &self,
        event: &tore_sim::ai::mission::LaunchEvent,
        weapon: &tore_formats::weapons::Weapon,
    ) -> bool {
        kind(weapon).is_some_and(StoreKind::seeker)
            && self
                .mission
                .actor(event.actor)
                .and_then(|a| a.strike())
                .and_then(|s| s.target())
                == Some(event.target)
    }

    /// A striker's seeker missile (Maverick, radar AGM, HARM) at its ground
    /// target: the AI's own missile release, as [`Self::realise`] makes it,
    /// with the launch observation taken from the target as the striker's
    /// cockpit holds it (its ground picture's target, which its seeker
    /// locked) rather than from the air picture. Returns how many it made.
    pub(super) fn release_seeker(
        &mut self,
        event: &tore_sim::ai::mission::LaunchEvent,
        state: &mut live::State,
        weapon: &tore_formats::weapons::Weapon,
        station: usize,
        humans: &[(u32, Vector)],
    ) -> u32 {
        let Some(actor) = self.mission.actor(event.actor) else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let Some(row) = state.targets.iter().find(|t| t.id == event.target) else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let (aim, velocity) = (row.position, row.velocity);
        let flight = actor.flight();
        // From its own side of its station (B3).
        let origin = self
            .release_mount(event.actor, station)
            .map_or(flight.position, |mount| {
                let basis = tore_sim::attitude::Basis::new(flight.yaw, flight.pitch, flight.bank);
                std::array::from_fn(|i| {
                    flight.position[i]
                        + basis.right[i] * mount[0]
                        + basis.up[i] * mount[1]
                        + basis.forward[i] * mount[2]
                })
            });
        let direction = missiles::sub(aim, origin);
        let length = missiles::length(direction);
        if !length.is_finite() || length <= 0. {
            self.dropped_launches += event.projectiles;
            return 0;
        }
        let direction = direction.map(|v| v / length);
        let Ok(speed) = launch_speed(&weapon.movement, (flight.speed * 256.) as i32) else {
            self.dropped_launches += event.projectiles;
            return 0;
        };
        let profile = (self.weapon_rules == missiles::Rules::Spec)
            .then(|| missiles::Profile::for_weapon(weapon))
            .flatten();
        let guidance = profile.map(|profile| {
            Flight::from_supported_launch(
                profile,
                LaunchMode::Cued,
                seeker::Observation {
                    id: event.target,
                    position: aim,
                    velocity,
                    quality: 1.0,
                    off_axis: 0.0,
                    range: length,
                },
                origin,
            )
        });
        let launch_velocity = flight.velocity;
        let launched = (self.mission.tick() / 30) as u16;
        let incoming = humans
            .iter()
            .any(|(id, _)| *id == event.target)
            .then_some(event.target);
        let mut emitted = 0;
        for _ in 0..event.projectiles {
            if state.projectiles.len() >= live::MAX_PROJECTILES {
                self.dropped_launches += 1;
                continue;
            }
            let id = self.next_projectile_id;
            self.next_projectile_id = self.next_projectile_id.wrapping_add(1);
            state.projectiles.push(live::Projectile {
                id,
                owner: event.actor,
                weapon: Some(weapon.clone()),
                guidance: guidance.clone(),
                motion: profile.map(|_| Motion::launch(weapon, launch_velocity, origin[1])),
                guidance_ticks: profile.map(|p| p.guidance_ticks),
                age: 0,
                incoming,
                station,
                position: origin,
                previous: origin,
                direction,
                speed_f8: speed * 256,
                launched_t: launched,
                target: Some(event.target),
                fall: FallState::default(),
                gun_round: None,
                tracer: false,
            });
            self.ai_shots.insert(id, event.actor);
            self.realised_launches += 1;
            emitted += 1;
        }
        emitted
    }
}

#[cfg(test)]
#[path = "strike_tests.rs"]
mod tests;
