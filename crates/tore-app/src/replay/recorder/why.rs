//! The reasons behind a tick: AI decisions and flight-model effects as
//! reason events, and the display trees (`ai.thought`, `flight.telemetry`,
//! `weapon.guidance`) at their rates and whenever a decision or an effect
//! changes. Everything here reads records the tick already wrote; nothing
//! feeds back. Rates, triggers and wording are agent decisions
//! (2026-09-26); see docs/REPLAYS.md ("What is recorded now").

use super::{Recorder, Tick, distance, ground_height, who};
use crate::ai_wings::{AiWings, DecoyRoll};
use crate::flight;
use crate::replay::trees::{self, Change, Choice, EffectKey};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tore_replay::{
    AircraftState, Event, Frame, ProjectileState, TreeSample, Value,
    vocab::{channel, field, kind},
};
use tore_sim::ai::{
    airfield::Phase as AirfieldPhase,
    controller::Activity,
    fitted::Fallback,
    mission::AiActor,
    thought::{ActorTrace, ControllerTrace, MotionBranch, RejoinReason, StepPath, TargetPath},
    threat::SeekerClass,
    weapon_service::{Phase, StationId},
};
use tore_sim::combat::live;

/// Ticks between thought samples of one AI aircraft: 10 a second.
const THOUGHT_TICKS: u64 = 12;
/// Ticks between the player's telemetry samples: 30 a second.
const PLAYER_TELEMETRY_TICKS: u64 = 4;
/// Ticks between an AI aircraft's telemetry samples: 5 a second.
const AI_TELEMETRY_TICKS: u64 = 24;
/// Ticks between guidance samples of one missile: 10 a second.
const GUIDANCE_TICKS: u64 = 12;
/// An effect gone for this long has stopped; a shorter gap is flicker and
/// continues the same episode.
const EFFECT_HOLD_TICKS: u64 = 30;
/// Decision changes the thought tree lists.
const RECENT: usize = 4;
/// The pitch stick counts as at its stop from here.
const STICK_STOP: f64 = 0.98;
/// Ticks the stick must leave its stop before another `flight.g_limit`.
const G_LIMIT_REARM_TICKS: u64 = 120;
/// Numbers for AI messages start here, above any radio call's number.
pub(super) const AI_MESSAGE_BASE: i64 = 1 << 32;

/// What the reason events and trees remember between ticks.
#[derive(Default)]
pub(super) struct Why {
    minds: BTreeMap<u32, Mind>,
    bodies: BTreeMap<u32, Body>,
    guides: BTreeMap<u32, Guide>,
    /// This tick's decoy rolls, by projectile.
    decoys: BTreeMap<u32, DecoyRoll>,
    /// The roll that decoyed each missile still in flight, kept after its
    /// own tick so a late strike can name the device that fooled it.
    decoyed_by: BTreeMap<u32, DecoyRoll>,
    /// The player's decoy rolls made between ticks, already written; the
    /// next tick's reasons and guidance trees read them.
    pub(super) pending_rolls: Vec<DecoyRoll>,
    /// The last AI message number given out.
    next_message: i64,
    /// Attack reports by (attacker, defended aircraft, projectile), so a
    /// delivery finds the report it answers.
    pub(super) attacks: HashMap<(Option<u32>, u32, Option<u32>), i64>,
    /// What each AI aircraft heard from other aircraft this tick, for the
    /// reasons of its decision changes.
    pub(super) news: BTreeMap<u32, Vec<String>>,
    /// Communication journal entries lost to its bound, as last reported.
    pub(super) comms_lost: u64,
    /// Data link journal entries lost to its bound, as last reported.
    pub(super) datalink_lost: u64,
}

impl Why {
    /// This tick's decoy roll that decoyed `projectile`, if any.
    pub(super) fn decoyed(&self, projectile: u32) -> Option<&DecoyRoll> {
        self.decoys.get(&projectile).filter(|roll| roll.decoyed)
    }

    /// A new tick: the rolls of the last one are only kept for the missiles
    /// they decoyed.
    pub(super) fn next_tick(&mut self) {
        self.decoys.clear();
    }

    /// The roll that decoyed `projectile` at any earlier tick, while it
    /// flies, and the device it was against.
    pub(super) fn decoyed_earlier(&self, projectile: u32) -> Option<&DecoyRoll> {
        self.decoyed_by.get(&projectile)
    }

    /// A guided shot's closest approach to its target so far, and when.
    pub(super) fn closest(&self, projectile: u32) -> Option<(f64, u64)> {
        self.guides.get(&projectile).and_then(|guide| guide.closest)
    }

    /// A new number for an AI message.
    pub(super) fn message(&mut self) -> i64 {
        self.next_message += 1;
        AI_MESSAGE_BASE + self.next_message
    }

    /// Something aircraft `id` heard from another aircraft this tick.
    pub(super) fn hear(&mut self, id: u32, news: String) {
        let list = self.news.entry(id).or_default();
        if list.len() < 3 && !list.contains(&news) {
            list.push(news);
        }
    }
}

/// What one AI aircraft's reasons remember.
#[derive(Default)]
struct Mind {
    seen: bool,
    activity: Option<Activity>,
    activity_since: u64,
    activity_reason: String,
    target: Option<u32>,
    phase: Option<Phase>,
    airfield: Option<AirfieldPhase>,
    /// Threat and maneuver of the defense in force.
    defense: Option<(u32, &'static str)>,
    fire: Option<(tore_sim::ai::incoming_fire::Evidence, bool)>,
    ejection: Option<&'static str>,
    /// The no-laze decision in force (slice ST-G).
    no_laze: Option<(tore_sim::ai::strike::NoLaze, StationId)>,
    /// A helicopter's last threat decision, by decision and tick (slice H4).
    threat_call: Option<(tore_sim::ai::helicopter_threat::Decision, u64)>,
    /// Fitted fallbacks already reported.
    fallbacks: BTreeSet<&'static str>,
    choice: Option<Choice>,
    recent: Vec<Change>,
    skeleton: Option<Skeleton>,
}

impl Mind {
    fn remember(&mut self, change: Change) {
        if self.recent.len() == RECENT {
            self.recent.remove(0);
        }
        self.recent.push(change);
    }
}

/// The decisions whose change records a thought tree at once.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Skeleton {
    activity: Activity,
    target: Option<u32>,
    phase: Phase,
    branch: u8,
    maneuver: Option<u64>,
}

/// What one aircraft's flight-model reasons remember.
#[derive(Default)]
struct Body {
    seen: bool,
    effects: Vec<(EffectKey, Episode)>,
    /// The pitch stick is at its stop.
    pinned: bool,
    /// Tick from which another G-limit event may be reported.
    rearm: u64,
    failed: bool,
}

/// One effect in force.
struct Episode {
    last: u64,
    label: String,
    momentary: bool,
}

/// What one guided missile's tree remembers.
#[derive(Default)]
struct Guide {
    first: u64,
    closest: Option<(f64, u64)>,
    roll: Option<DecoyRoll>,
    /// Sample on the next tick whatever the rate: a decoy roll happened.
    force: bool,
}

/// `chaff` or `flare`, for the `decoy` field.
pub(super) fn decoy_kind(class: SeekerClass) -> &'static str {
    match class {
        SeekerClass::Radar => "chaff",
        SeekerClass::Infrared => "flare",
    }
}

/// The device a roll was against, for sentences: `chaff #7` or `flare #12`.
/// The number is the device's release number in the mission, as its
/// `combat.countermeasure` entry carries it; a roll recorded without one
/// (before the number was kept) names the kind alone.
pub(super) fn decoy_label(roll: &DecoyRoll) -> String {
    let kind = decoy_kind(roll.class);
    if roll.device == 0 {
        kind.to_owned()
    } else {
        format!("{kind} #{}", roll.device)
    }
}

/// The `weapon.decoyed` entry for a roll that decoyed its missile, fired by
/// `owner` when known.
pub(super) fn decoyed_event(roll: &DecoyRoll, owner: Option<u32>) -> Event {
    let mut event = Event::new(kind::WEAPON_DECOYED)
        .with_object(roll.releaser)
        .with(field::PROJECTILE, Value::Id(roll.projectile))
        .with(field::DECOY, decoy_kind(roll.class))
        .with(field::SUSCEPTIBILITY, i64::from(roll.susceptibility))
        .with(field::EFFECTIVENESS, i64::from(roll.effectiveness));
    if roll.device > 0 {
        // Which release fooled it: the number on its `combat.countermeasure`.
        event = event.with(field::NUMBER, roll.device as i64);
    }
    if let Some(owner) = owner {
        event = event.with_subject(owner);
    }
    if let Some(draw) = roll.draw {
        event = event
            .with(field::ROLL, i64::from(draw.value))
            .with(field::ROLL_BOUND, i64::from(draw.bound));
        if let Some(threshold) = draw.threshold {
            event = event.with(field::THRESHOLD, i64::from(threshold));
        }
    }
    event.with(field::REASON, roll_reason(roll))
}

fn roll_reason(roll: &DecoyRoll) -> String {
    let draw = roll
        .draw
        .map_or_else(|| "a roll".to_owned(), |d| trees::draw_text(&d));
    let units = roll.draw.map_or(String::new(), |d| {
        if d.bound == 10000 {
            " in basis points".to_owned()
        } else {
            String::new()
        }
    });
    format!(
        "{draw}{units} (susceptibility {}% x effectiveness {}%)",
        roll.susceptibility, roll.effectiveness
    )
}

/// Everything one AI aircraft's reasons read this tick.
struct Look<'a> {
    number: u64,
    id: u32,
    actor: &'a AiActor,
    trace: &'a ControllerTrace,
    act: &'a ActorTrace,
    /// The controller record describes this tick.
    fresh: bool,
    state: &'a AircraftState,
    info: Option<&'a tore_replay::AircraftInfo>,
    aircraft: &'a [AircraftState],
    projectiles: &'a [ProjectileState],
    who: &'a dyn Fn(u32) -> String,
    store: &'a dyn Fn(StationId) -> Option<String>,
    /// What it heard from other aircraft this tick.
    news: &'a [String],
}

impl Look<'_> {
    fn store_name(&self, station: StationId) -> String {
        (self.store)(station).unwrap_or_else(|| format!("station {}", station.0))
    }

    /// A mission clock time for an AI quarter-second count.
    fn at_quarter(&self, quarter: u64) -> String {
        let ai_now = self.act.tick.unwrap_or(self.number);
        let tick = (quarter * 30) as i64 + self.number as i64 - ai_now as i64;
        trees::clock(tick.max(0) as u64)
    }

    fn with_news(&self, reason: String) -> String {
        if self.news.is_empty() {
            reason
        } else if reason.is_empty() {
            format!("after {}", self.news.join("; "))
        } else {
            format!("{reason}; after {}", self.news.join("; "))
        }
    }

    fn state_of(&self, id: u32) -> Option<&AircraftState> {
        self.aircraft.iter().find(|a| a.id == id)
    }
}

/// Why an AI aircraft is doing what it does, from its records.
fn activity_reason(look: &Look, activity: Activity) -> String {
    let who = look.who;
    let trace = look.trace;
    let target = look.actor.controller().target();
    let fresh = look.fresh;
    let choice = || {
        trace
            .maneuver
            .as_ref()
            .map(|m| match &m.choice {
                Some(c) => trees::choice_label(c),
                None => trees::maneuver_path(&m.path),
            })
            .filter(|_| fresh)
    };
    let reason = match activity {
        Activity::Destroyed => {
            let f = look.actor.flight();
            if f.escape.is_some() {
                "the pilot ejected".to_owned()
            } else if f.systems.pilot.dead {
                "the pilot was killed".to_owned()
            } else if f.crashed {
                "it crashed".to_owned()
            } else {
                "destroyed".to_owned()
            }
        }
        Activity::Defending => match (fresh, trace.motion.branch) {
            (true, MotionBranch::IncomingFire { .. }) => look.act.fire.cue.map_or_else(
                || "incoming fire defense".to_owned(),
                |cue| format!("jink: {}", cue.evidence.label()),
            ),
            (true, MotionBranch::MissileDefense { .. }) => match look.actor.defense_decision() {
                Some(d) => {
                    let (maneuver, _) = trees::defense_maneuver(&d);
                    let why = trees::defense_reasons(&d, Some(look.actor.experience()));
                    if why.is_empty() {
                        format!("missile defense: {maneuver} against shot {}", d.threat_id)
                    } else {
                        format!(
                            "missile defense: {maneuver} against shot {} because {why}",
                            d.threat_id
                        )
                    }
                }
                None => "missile defense".to_owned(),
            },
            _ => choice().map_or_else(
                || "a missile reaction".to_owned(),
                |c| format!("tactics chose {c}"),
            ),
        },
        Activity::Evading => {
            if fresh && trace.events.hit {
                "it was hit this tick".to_owned()
            } else {
                choice().map_or_else(|| "evading".to_owned(), |c| format!("tactics chose {c}"))
            }
        }
        Activity::Attacking if look.actor.strike().is_some() => {
            "flying a ground attack pass; see Ground attack for the selection".to_owned()
        }
        Activity::Attacking => {
            let store = trace
                .weapons
                .as_ref()
                .and_then(|w| w.chosen)
                .map(|s| look.store_name(s))
                .unwrap_or_else(|| "a store".to_owned());
            match target {
                Some(t) => format!("firing {store} at {}", who(t)),
                None => format!("firing {store}"),
            }
        }
        Activity::Pursuing => match target {
            Some(t) => format!("closing on {} with a firing solution", who(t)),
            None => "pursuing".to_owned(),
        },
        Activity::Acquiring => {
            let why = trace
                .gun
                .filter(|_| fresh)
                .and_then(|g| g.view.solution)
                .filter(|s| !s.aligned)
                .map(|s| {
                    format!(
                        "fixed barrel misses the predicted target by {:.1} ft",
                        s.miss_ft
                    )
                })
                .or_else(|| {
                    trace.weapons.as_ref().filter(|_| fresh).and_then(|w| {
                        trees::fire_reason(w, target, &look.act.stations, &|q| look.at_quarter(q))
                    })
                });
            match (target, why) {
                (Some(t), Some(why)) => format!("{} chosen; no firing solution yet: {why}", who(t)),
                (Some(t), None) => format!("{} chosen; no firing solution yet", who(t)),
                (None, _) => "no firing solution yet".to_owned(),
            }
        }
        Activity::Formation => {
            if look.actor.is_neutral() {
                "holding formation until the leader releases the wing".to_owned()
            } else {
                "no target: flying its formation slot".to_owned()
            }
        }
        Activity::Searching => match (fresh, trace.motion.branch) {
            (
                true,
                MotionBranch::Search {
                    contact,
                    opportunity: true,
                    ..
                },
            ) => format!(
                "leading a mission of opportunity after the lead was lost: searching where {} was last seen",
                who(contact.id)
            ),
            (true, MotionBranch::WingRoute { .. }) => {
                "leading a mission of opportunity: nothing found, flying the wing's waypoints"
                    .to_owned()
            }
            (true, MotionBranch::Search { contact, .. }) => {
                format!("investigating {}, last seen earlier", who(contact.id))
            }
            (true, MotionBranch::SearchBearing { bearing_deg }) => format!(
                "searching along bearing {:.0} after an attack on its charge",
                bearing_deg
            ),
            _ => "no target in sight".to_owned(),
        },
        Activity::Rejoining => match look.act.rejoin.map(|r| r.reason) {
            Some(RejoinReason::EscortLeash) => "an escort beyond its 10 nm leash".to_owned(),
            Some(RejoinReason::OutsidePatrol) => "outside its patrol region".to_owned(),
            None => "flying back to a friendly".to_owned(),
        },
        Activity::ReturningToBase if look.actor.damage_return().is_some() => {
            format!(
                "{}: recovering with weapons held",
                look.actor.damage_return().unwrap().label()
            )
        }
        Activity::Waiting if look.act.damage.ground_hold => {
            "holding on the ground because of system damage".into()
        }
        Activity::ReturningToBase => {
            let state = trace.fuel.and_then(|f| f.state).filter(|_| fresh);
            match state {
                Some(s) => format!("fuel {}: heading home", trees::fuel_label(s)),
                None => "heading home".to_owned(),
            }
        }
        Activity::OutOfFuel => "its internal fuel is gone".to_owned(),
        Activity::Breaking => "a break order".to_owned(),
        Activity::Waiting
        | Activity::Taxiing
        | Activity::TakingOff
        | Activity::HoldingMarshal
        | Activity::Landing
        | Activity::Landed => match look.actor.airfield_phase() {
            Some(phase) => format!("airfield sequence: {}", trees::airfield_label(phase)),
            None => "airfield sequence".to_owned(),
        },
        Activity::Idle => "no target and no leader to follow".to_owned(),
    };
    look.with_news(reason)
}

/// Why the target changed: its priority, score and a sentence.
fn target_reason(
    look: &Look,
    previous: Option<u32>,
    target: Option<u32>,
) -> (Option<&'static str>, Option<f64>, String) {
    let who = look.who;
    let mut parts = Vec::new();
    if let Some(p) = previous
        && look.fresh
        && look.trace.events.lost_targets.contains(&p)
    {
        parts.push(format!("{} left the world or became unavailable", who(p)));
    }
    let mut priority = None;
    let mut score = None;
    if look.fresh {
        let explanation = look.act.engagement.as_ref().map(|e| &e.explanation);
        match (look.trace.target.path, explanation) {
            (TargetPath::Mission { .. }, Some(e)) if target.is_some() => {
                let rank = trees::ranking(e, target);
                priority = rank.priority.map(trees::priority_label);
                score = rank.score_ft;
                let mut line = format!("mission ranking: {}", priority.unwrap_or("no priority"));
                if let Some(s) = score {
                    line.push_str(&format!(", score {} ft", trees::thousands(s)));
                }
                if let Some((id, p, s)) = rank.runner_up {
                    line.push_str(&format!(
                        " (next: {}, {}, {} ft)",
                        who(id),
                        p.map_or("no priority", trees::priority_label),
                        trees::thousands(s)
                    ));
                }
                parts.push(line);
            }
            (
                TargetPath::Ranked {
                    candidates,
                    score_ft,
                },
                _,
            ) => {
                score = score_ft;
                let mut line = format!("nearest first of {candidates}");
                if let Some(s) = score_ft {
                    line.push_str(&format!(", score {} ft", trees::thousands(s)));
                }
                parts.push(line);
            }
            (path, _) => {
                if let Some((how, why)) = trees::target_path(path, who) {
                    parts.push(if why.is_empty() {
                        how.to_owned()
                    } else {
                        format!("{how}: {why}")
                    });
                }
            }
        }
    }
    if target.is_none() && parts.is_empty() {
        parts.push("no target in sight".to_owned());
    }
    (priority, score, look.with_news(parts.join("; ")))
}

/// Target geometry from the recorded states.
fn relative(own: &AircraftState, target: &AircraftState) -> trees::Relative {
    let line: [f64; 3] = std::array::from_fn(|i| target.position[i] - own.position[i]);
    let range = distance(target.position, own.position).max(1e-9);
    let forward = target.forward();
    let dot: f64 = (0..3).map(|i| forward[i] * line[i]).sum();
    let aspect = (dot / range).clamp(-1., 1.).acos().to_degrees();
    let closure = -(0..3)
        .map(|i| (target.velocity[i] - own.velocity[i]) * line[i])
        .sum::<f64>()
        / range;
    trees::Relative {
        aspect_deg: aspect,
        closure_kt: trees::knots(closure),
        height_ft: target.position[1] - own.position[1],
    }
}

impl Recorder {
    /// This tick's decoy rolls: a `weapon.decoyed` for each missile that
    /// followed a decoy, and the rolls kept for the outcome reasons and the
    /// guidance trees.
    pub(super) fn decoys(&mut self, tick: &Tick<'_>, frame: &Frame, events: &mut Vec<Event>) {
        self.why.next_tick();
        // The player's rolls since the last tick; their entries are written.
        for roll in std::mem::take(&mut self.why.pending_rolls) {
            self.keep_roll(roll);
        }
        let Some(wings) = tick.wings else {
            return;
        };
        for roll in wings.decoy_rolls() {
            self.keep_roll(*roll);
            if !roll.decoyed {
                continue;
            }
            let owner = self
                .shots
                .get(&roll.projectile)
                .map(|s| s.owner)
                .or_else(|| {
                    frame
                        .projectiles
                        .iter()
                        .find(|p| p.id == roll.projectile)
                        .map(|p| p.owner)
                });
            events.push(decoyed_event(roll, owner));
        }
    }

    /// A roll this tick's outcome reasons and the missile's guidance tree
    /// read.
    pub(super) fn keep_roll(&mut self, roll: DecoyRoll) {
        self.why.decoys.insert(roll.projectile, roll);
        if roll.decoyed {
            self.why.decoyed_by.insert(roll.projectile, roll);
        }
        if let Some(guide) = self.why.guides.get_mut(&roll.projectile) {
            guide.roll = Some(roll);
            guide.force = true;
        }
    }

    /// The reasons behind the tick and its display trees.
    pub(super) fn explain(&mut self, tick: &Tick<'_>, frame: &mut Frame, events: &mut Vec<Event>) {
        self.why.news.clear();
        if let Some(batch) = tick.journal {
            self.ai_journal(batch, frame, events);
        }
        if let Some(wings) = tick.wings {
            self.minds(wings, frame, events);
        }
        self.bodies(tick, frame, events);
        self.guidance(tick, frame);
    }

    /// Every AI aircraft's decision changes, with their reasons, and its
    /// thought tree.
    fn minds(&mut self, wings: &AiWings, frame: &mut Frame, events: &mut Vec<Event>) {
        let number = frame.tick;
        let output = wings.last_output();
        let infos = &self.infos;
        let why = &mut self.why;
        let named = |id: u32| who(infos, id);
        let Frame {
            aircraft,
            projectiles,
            trees: samples,
            ..
        } = frame;
        for state in aircraft.iter() {
            let id = state.id;
            let Some(actor) = wings.mission().actor(id) else {
                continue;
            };
            let mut mind = why.minds.remove(&id).unwrap_or_default();
            let news = why.news.remove(&id).unwrap_or_default();
            let trace = actor.controller().trace();
            let act = actor.trace();
            let store = |s: StationId| wings.station_weapon(id, s.0).map(str::to_owned);
            let look = Look {
                number,
                id,
                actor,
                trace,
                act,
                fresh: trace.path == StepPath::Decided
                    && trace.tick.is_some()
                    && trace.tick == act.tick,
                state,
                info: infos.get(&id),
                aircraft,
                projectiles,
                who: &named,
                store: &store,
                news: &news,
            };
            let fallbacks: Vec<Fallback> = output
                .fallbacks
                .iter()
                .filter(|(actor, _)| *actor == id)
                .map(|(_, f)| *f)
                .collect();
            decide(&look, &mut mind, &fallbacks, events);
            // The thought tree: 10 a second while alive, and at once when
            // the decision skeleton changes.
            let skeleton = Skeleton {
                activity: actor.activity(),
                target: actor.controller().target(),
                phase: actor.controller().weapon_phase(),
                branch: trees::branch_code(trace, act),
                maneuver: actor.controller().active_maneuver().map(|m| m.intent.id),
            };
            let due = (number + u64::from(id)).is_multiple_of(THOUGHT_TICKS);
            if mind.skeleton != Some(skeleton) || (due && state.flags.alive) {
                samples.push(TreeSample {
                    subject: id,
                    channel: channel::AI_THOUGHT.into(),
                    nodes: thought(&look, &mind, &fallbacks),
                });
            }
            mind.skeleton = Some(skeleton);
            why.minds.insert(id, mind);
        }
    }

    /// Every aircraft's flight-model effects, G-limit hits and structural
    /// failure, and its telemetry tree.
    fn bodies(&mut self, tick: &Tick<'_>, frame: &mut Frame, events: &mut Vec<Event>) {
        let number = frame.tick;
        let player = self.player;
        let infos = &self.infos;
        let why = &mut self.why;
        let Frame {
            aircraft,
            trees: samples,
            ..
        } = frame;
        for state in aircraft.iter() {
            let id = state.id;
            let Some(f) = super::flight_of(player, tick, id) else {
                continue;
            };
            let mut body = why.bodies.remove(&id).unwrap_or_default();
            let changed = effects(&mut body, id, number, f, events);
            limits(&mut body, id, number, f, events);
            failure(&mut body, id, player, state, f, tick, events);
            body.seen = true;
            let period = if id == player {
                PLAYER_TELEMETRY_TICKS
            } else {
                AI_TELEMETRY_TICKS
            };
            let due = (number + u64::from(id)).is_multiple_of(period);
            if changed || (due && state.flags.alive) {
                let air = tick.world.air_data(f).ok();
                let info = infos.get(&id);
                let telemetry = trees::Telemetry {
                    label: info.map_or("", |i| i.label.as_str()),
                    name: info.map_or("", |i| i.name.as_str()),
                    trace: f.trace(),
                    air: air.as_ref(),
                    altitude_ft: f.position[1],
                    agl_ft: f.position[1] - ground_height(tick.world, f.position),
                    g: f.g,
                    fuel_lb: f.fuel,
                    rates: f.maneuver.body_rates_rad_per_second,
                    throttle: f.throttle,
                    afterburner: f.afterburner_active(),
                };
                samples.push(TreeSample {
                    subject: id,
                    channel: channel::FLIGHT_TELEMETRY.into(),
                    nodes: trees::flight_telemetry(&telemetry),
                });
            }
            why.bodies.insert(id, body);
        }
    }

    /// Every guided missile's guidance tree, 10 a second, at launch and
    /// after a decoy roll; and its closest approach so far.
    fn guidance(&mut self, tick: &Tick<'_>, frame: &mut Frame) {
        let number = frame.tick;
        let player = self.player;
        let infos = &self.infos;
        let named = |id: u32| who(infos, id);
        let position = |id: u32| -> Option<[f64; 3]> {
            frame
                .aircraft
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.position)
                .or_else(|| {
                    tick.combat
                        .state
                        .targets
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| t.position)
                })
        };
        let mut live = BTreeSet::new();
        let mut samples = Vec::new();
        for p in &tick.combat.state.projectiles {
            let Some(g) = p.guidance.as_ref().filter(|g| !g.unguided) else {
                continue;
            };
            live.insert(p.id);
            let guide = self.why.guides.entry(p.id).or_insert_with(|| Guide {
                first: number,
                ..Guide::default()
            });
            let aim = p.target.or(g.seeker.target);
            let range = aim.and_then(position).map(|t| distance(p.position, t));
            if let Some(r) = range
                && guide.closest.is_none_or(|(c, _)| r < c)
            {
                guide.closest = Some((r, number));
            }
            let due = guide.first == number
                || guide.force
                || (number + u64::from(p.id)).is_multiple_of(GUIDANCE_TICKS);
            if !due {
                continue;
            }
            guide.force = false;
            let weapon = p.weapon(super::stores_of(tick, player, p.owner));
            use tore_sim::combat::missiles::Guidance as Kind;
            let kind_name = match g.profile.guidance {
                Kind::Supported => "semi-active radar",
                Kind::Active => "active radar",
                Kind::Infrared => "infrared",
                Kind::Emitter => "anti-radiation",
                Kind::Laser => "laser",
                Kind::Coordinate => "coordinate",
                Kind::Command => "command",
            };
            let decoy = guide.roll.map(|roll| {
                format!(
                    "{} {} from {}: {}",
                    if roll.decoyed {
                        "decoyed by"
                    } else {
                        "resisted"
                    },
                    decoy_label(&roll),
                    named(roll.releaser),
                    roll_reason(&roll)
                )
            });
            let guidance = trees::Guidance {
                weapon: &weapon.name,
                shooter: p.owner,
                target: aim,
                mode: if g.mode == tore_sim::combat::missiles::LaunchMode::Boresight {
                    "boresight"
                } else {
                    "cued"
                },
                guidance: kind_name,
                seeker: Some(trees::SeekerView {
                    status: g.seeker.status.label(),
                    quality: g.seeker.quality,
                    acquired: g.seeker.acquired,
                    tracking: g.seeker.target.filter(|_| g.seeker.acquired),
                }),
                enabled: g.enabled,
                age: p.age,
                speed_fps: f64::from(p.speed_f8) / 256.,
                range_ft: range,
                closest: guide.closest,
                intercept_s: g.solution.map(|s| s.seconds),
                decoy,
                who: &named,
            };
            samples.push(TreeSample {
                subject: p.id,
                channel: channel::WEAPON_GUIDANCE.into(),
                nodes: trees::weapon_guidance(&guidance),
            });
        }
        frame.trees.extend(samples);
        self.why.guides.retain(|id, _| live.contains(id));
        self.why.decoyed_by.retain(|id, _| live.contains(id));
    }
}

/// One AI aircraft's decision changes as reason events.
fn decide(look: &Look, mind: &mut Mind, fallbacks: &[Fallback], events: &mut Vec<Event>) {
    let (id, number, actor, trace) = (look.id, look.number, look.actor, look.trace);
    let who = look.who;
    let first = !mind.seen;
    mind.seen = true;
    if look.fresh
        && let Some(maneuver) = trace.maneuver
    {
        mind.choice = Some(Choice {
            tick: number,
            maneuver,
            resolve: trace.resolve,
            draws: actor.controller().draws().draws().copied().collect(),
        });
    }

    // Activity.
    let activity = actor.activity();
    if first {
        mind.activity = Some(activity);
        mind.activity_since = number;
        mind.activity_reason = activity_reason(look, activity);
    } else if mind.activity != Some(activity) {
        let reason = activity_reason(look, activity);
        let from = mind.activity.map_or("-", Activity::label);
        events.push(
            Event::new(kind::AI_ACTIVITY)
                .with_subject(id)
                .with(field::FROM, from)
                .with(field::TO, activity.label())
                .with(
                    field::FOR_S,
                    trees::round((number - mind.activity_since) as f64 / 120., 1),
                )
                .with(field::REASON, reason.as_str()),
        );
        mind.remember(Change {
            tick: number,
            what: format!("{from} -> {}", activity.label()),
            why: reason.clone(),
        });
        mind.activity = Some(activity);
        mind.activity_since = number;
        mind.activity_reason = reason;
    }

    // Target.
    let target = actor.controller().target();
    if !first && target != mind.target {
        let (priority, score, reason) = target_reason(look, mind.target, target);
        let mut event = Event::new(kind::AI_TARGET).with_subject(id);
        if let Some(from) = mind.target {
            event = event.with(field::FROM, Value::Id(from));
        }
        if let Some(to) = target {
            event = event.with_object(to).with(field::TO, Value::Id(to));
        }
        if let Some(p) = priority {
            event = event.with(field::PRIORITY, p);
        }
        if let Some(s) = score {
            event = event.with(field::SCORE, trees::round(s, 0));
        }
        events.push(event.with(field::REASON, reason.as_str()));
        let name = |t: Option<u32>| t.map_or_else(|| "none".to_owned(), who);
        mind.remember(Change {
            tick: number,
            what: format!("target {} -> {}", name(mind.target), name(target)),
            why: reason,
        });
    }
    mind.target = target;

    // Weapon service phase.
    let phase = actor.controller().weapon_phase();
    if !first && mind.phase != Some(phase) {
        let weapons = trace.weapons.as_ref().filter(|_| look.fresh);
        let mut event = Event::new(kind::AI_WEAPON_PHASE)
            .with_subject(id)
            .with(field::FROM, mind.phase.map_or("-", trees::phase_label))
            .with(field::TO, trees::phase_label(phase));
        if let Some(t) = target {
            event = event.with_object(t);
        }
        if let Some(station) = weapons.and_then(|w| w.chosen) {
            event = event
                .with(field::STATION, i64::from(station.0))
                .with(field::WEAPON, look.store_name(station));
        }
        let reason = weapons
            .and_then(|w| w.outcome.as_ref())
            .map(|o| trees::service_text(o, &|q| look.at_quarter(q)));
        if let Some(reason) = reason {
            event = event.with(field::REASON, reason);
        }
        events.push(event);
    }
    mind.phase = Some(phase);

    // A strike decision: laser stores and no laser support (slice ST-G).
    let no_laze = actor
        .strike()
        .and_then(|strike| strike.no_laze())
        .map(|call| (call.kind, call.station, call.target));
    if let Some((decision, station, target)) = no_laze
        && mind.no_laze != Some((decision, station))
    {
        let event = strike_decision_event(id, decision, station, target, look.store_name(station));
        events.push(event);
        mind.remember(Change {
            tick: number,
            what: format!("strike: {}", decision.reason()),
            why: decision.words().to_owned(),
        });
    }
    mind.no_laze = no_laze.map(|(decision, station, _)| (decision, station));

    // A helicopter's threat decision (slice H4): hold off, resume, switch
    // targets, home, or leave an ambush's ring.
    let threat_call = actor
        .strike()
        .and_then(|strike| strike.threat_call())
        .copied();
    if let Some(call) = threat_call
        && mind.threat_call != Some((call.decision, call.tick))
    {
        events.push(threat_decision_event(id, &call));
        mind.remember(Change {
            tick: number,
            what: format!("strike: {}", call.decision.reason()),
            why: call.decision.words().to_owned(),
        });
    }
    mind.threat_call = threat_call.map(|call| (call.decision, call.tick));

    // Airfield sequence.
    let airfield = actor.airfield_phase();
    if !first && airfield != mind.airfield {
        let name = |p: Option<AirfieldPhase>| p.map_or("-", trees::airfield_label);
        let mut event = Event::new(kind::AI_AIRFIELD_PHASE)
            .with_subject(id)
            .with(field::FROM, name(mind.airfield))
            .with(field::TO, name(airfield));
        if let Some(reason) = airfield_reason(look) {
            event = event.with(field::REASON, reason);
        }
        events.push(event);
    }
    mind.airfield = airfield;

    // Missile defense.
    let decision = actor.defense_decision();
    let now = decision.map(|d| (d.threat_id, trees::defense_maneuver(&d).0));
    if !first && now != mind.defense {
        match decision {
            Some(d) => events.push(defense_event(look, &d, true)),
            None => {
                let mut event = Event::new(kind::AI_DEFENSE)
                    .with_subject(id)
                    .with(field::REACTION, "ended")
                    .with(field::REASON, "no missile threat left");
                if let Some((threat, _)) = mind.defense {
                    event = event.with(field::THREAT, Value::Id(threat));
                }
                events.push(event);
            }
        }
    } else if let Some(d) = decision.filter(|d| d.burst.is_some()) {
        events.push(defense_event(look, &d, false));
    }
    mind.defense = now;

    // Anonymous gun/hit evidence is independent of the missile warning record.
    let fire = look.act.fire;
    let fire_now = fire.cue.map(|cue| (cue.evidence, fire.selected));
    if fire_now != mind.fire {
        let mut event = Event::new(kind::AI_DEFENSE).with_subject(id).with(
            field::REACTION,
            if fire.selected {
                "jink under fire"
            } else if fire.cue.is_some() {
                "gunfire known; missile defense selected"
            } else {
                "incoming fire cleared"
            },
        );
        if let Some(cue) = fire.cue {
            event = event.with(field::REASON, cue.evidence.label());
            if let Some(round) = cue.round {
                event = event.with(field::THREAT, Value::Id(round));
            }
        } else {
            event = event.with(
                field::REASON,
                "two seconds without new fire evidence; missile defense and mission restrictions still apply",
            );
        }
        events.push(event);
    }
    mind.fire = fire_now;

    // Fitted fallbacks, the first time each is used.
    let mut used: Vec<Fallback> = fallbacks.to_vec();
    if let Some(adapter) = look.act.fly.as_ref().and_then(|f| f.adapter.as_ref()) {
        used.extend(adapter.fallbacks.iter().copied());
    }
    for fallback in used {
        if mind.fallbacks.insert(fallback.name()) {
            events.push(
                Event::new(kind::AI_FALLBACK)
                    .with_subject(id)
                    .with(field::FROM, fallback.spec_branch())
                    .with(field::TO, fallback.name())
                    .with(field::REASON, fallback.rule()),
            );
        }
    }

    // Ejection checks.
    let ejection = look.act.ejection.and_then(|e| {
        if e.escape_running || e.ejected {
            Some("ejected")
        } else if e.go_around == Some(true) {
            Some("go-around instead")
        } else if e.assessment.is_some() {
            Some("hazard found")
        } else {
            None
        }
    });
    if !first && ejection != mind.ejection {
        let hazard = look
            .act
            .ejection
            .and_then(|e| e.assessment)
            .map(|a| trees::hazard_text(&a));
        let (decision, reason) = match ejection {
            Some("ejected") => ("ejected", hazard.unwrap_or_default()),
            Some("go-around instead") => (
                "go-around instead",
                format!(
                    "{} during {}, not a catastrophe",
                    hazard.unwrap_or_else(|| "a hazard".into()),
                    look.act
                        .ejection
                        .and_then(|e| e.guarded_phase)
                        .map_or("an airfield sequence", trees::airfield_label)
                ),
            ),
            Some(_) => (
                "hazard found",
                format!(
                    "{}; the pilot decides once a second",
                    hazard.unwrap_or_default()
                ),
            ),
            None => ("stayed", "the hazard passed".to_owned()),
        };
        events.push(
            Event::new(kind::AI_EJECTION)
                .with_subject(id)
                .with(field::DECISION, decision)
                .with(field::REASON, reason),
        );
    }
    mind.ejection = ejection;
}

/// An `ai.strike` event for a no-laze decision (slice ST-G): the aircraft,
/// the target it would attack, the decision in words, the fixed `reason` key
/// (`no_laze_manual` or `no_laze_rtb`) and the laser store.
fn strike_decision_event(
    id: u32,
    decision: tore_sim::ai::strike::NoLaze,
    station: StationId,
    target: Option<u32>,
    weapon: String,
) -> Event {
    use tore_sim::ai::strike::NoLaze;
    let mut event = Event::new(kind::AI_STRIKE)
        .with_subject(id)
        .with(
            field::DECISION,
            match decision {
                NoLaze::Manual => "no laze support, dropping manual",
                NoLaze::Rtb => "no laze support, RTB",
            },
        )
        .with(field::REASON, decision.reason())
        .with(field::STATION, i64::from(station.0))
        .with(field::WEAPON, weapon);
    if let Some(target) = target {
        event = event.with_object(target);
    }
    event
}

/// An `ai.strike` event for a helicopter's threat decision (slice H4): the
/// aircraft, the target it concerns, the decision in words, the fixed
/// `reason` key (`threat_hold_off`, `threat_resume`, `threat_retarget`,
/// `threat_home`, `threat_leave`).
fn threat_decision_event(id: u32, call: &tore_sim::ai::helicopter_threat::Call) -> Event {
    let mut event = Event::new(kind::AI_STRIKE)
        .with_subject(id)
        .with(field::DECISION, call.decision.words())
        .with(field::REASON, call.decision.reason());
    if let Some(target) = call.target {
        event = event.with_object(target);
    }
    event
}

/// An `ai.defense` event for the missile defense in force: its maneuver
/// and devices when `change`, otherwise the devices released now.
fn defense_event(look: &Look, d: &tore_sim::ai::defense::DefenseDecision, change: bool) -> Event {
    let (maneuver, heading) = trees::defense_maneuver(d);
    let burst = trees::burst_text(d.burst);
    let reaction = match (change, d.burst.is_some()) {
        (true, true) => format!("{maneuver}, {burst}"),
        (true, false) => maneuver.to_owned(),
        (false, _) => burst,
    };
    let mut reason = trees::defense_reasons(d, Some(look.actor.experience()));
    if look.act.fire.selected {
        reason.push_str("; gun/hit cue takes motion priority, missile devices still apply");
    }
    if change && d.motion.is_some() {
        reason = if reason.is_empty() {
            heading
        } else {
            format!("{reason}; {heading}")
        };
    }
    let mut event = Event::new(kind::AI_DEFENSE)
        .with_subject(look.id)
        .with(field::THREAT, Value::Id(d.threat_id))
        .with(field::REACTION, reaction);
    if let Some(p) = look.projectiles.iter().find(|p| p.id == d.threat_id) {
        event = event.with_object(p.owner).with(
            field::RANGE_FT,
            trees::round(distance(p.position, look.state.position), 0),
        );
    }
    if !reason.is_empty() {
        event = event.with(field::REASON, reason);
    }
    event
}

fn airfield_reason(look: &Look) -> Option<String> {
    let act = look.act;
    if let Some(order) = act.bingo_landing {
        return Some(format!("bingo fuel: landing ({:?})", order.reason).to_lowercase());
    }
    if let Some(order) = act.join_landing {
        return Some(format!("joining its leader's landing ({:?})", order.reason).to_lowercase());
    }
    let sequence = act.airfield?;
    if let Some(exit) = sequence.left {
        return Some(match exit {
            tore_sim::ai::thought::AirfieldExit::WingAbort => {
                "its leader is neither landing nor on the ground".into()
            }
            tore_sim::ai::thought::AirfieldExit::MissileThreat { .. } => {
                "a missile threat interrupted the approach".into()
            }
        });
    }
    if sequence.began_landing {
        let reason = look
            .actor
            .landing_order()
            .map(|o| format!("{:?}", o.reason).to_lowercase());
        return Some(match reason {
            Some(r) => format!("a landing began ({r})"),
            None => "a landing began".into(),
        });
    }
    if sequence.step.is_some_and(|s| s.go_around) {
        return Some("it abandoned the final".into());
    }
    None
}

/// The thought tree for one AI aircraft.
fn thought(look: &Look, mind: &Mind, fallbacks: &[Fallback]) -> Vec<tore_replay::Node> {
    let actor = look.actor;
    let target = actor.controller().target();
    let label = (look.who)(look.id);
    let experience = actor.experience();
    let defense = actor.defense_decision();
    let threat = defense.map(|d| {
        let p = look.projectiles.iter().find(|p| p.id == d.threat_id);
        trees::Threat {
            shooter: p.map(|p| p.owner),
            weapon: None,
            range_ft: p.map(|p| distance(p.position, look.state.position)),
        }
    });
    let f = actor.flight();
    let own = trees::Own {
        position: look.state.position,
        speed_fps: look.state.airspeed,
        g: look.state.g,
        g_limit: f.trace().adapter.map(|a| a.envelope.limits_g[1]),
        fuel_lb: look.state.fuel_lb,
        afterburner: look.state.flags.afterburner,
    };
    let relative = target
        .and_then(|t| look.state_of(t))
        .map(|t| relative(look.state, t));
    let identity = actor.identity();
    let draws: Vec<tore_sim::ai::Draw> = actor.controller().draws().draws().copied().collect();
    let t = trees::Thought {
        tick: look.number,
        label: &label,
        name: look
            .info
            .map_or_else(|| identity.aircraft.label(), |i| i.name.as_str()),
        side: look.info.map_or("", |i| i.side.name()),
        wing: look.info.map_or(0, |i| i.wing),
        member: look.info.map_or(0, |i| i.member),
        leader: identity.is_leader(),
        experience: Some(experience),
        activity: actor.activity(),
        activity_since: mind.activity_since,
        activity_reason: &mind.activity_reason,
        target,
        weapon_phase: actor.controller().weapon_phase(),
        neutral: actor.is_neutral(),
        airfield: actor.airfield_phase(),
        defense,
        threat,
        controller: look.trace,
        actor: look.act,
        draws: &draws,
        choice: mind.choice.as_ref(),
        relative,
        own,
        fallbacks,
        recent: &mind.recent,
        who: look.who,
        store: look.store,
    };
    let mut nodes = trees::ai_thought(&t);
    if let Some(strike) = actor.strike() {
        nodes.extend(trees::strike_thought(strike, look.store));
    }
    nodes
}

/// The effects in force, with debounced on and off events. Returns whether
/// the set of effects changed, so a telemetry tree is recorded at once.
fn effects(
    body: &mut Body,
    id: u32,
    number: u64,
    f: &flight::State,
    events: &mut Vec<Event>,
) -> bool {
    effect_changes(body, id, number, &f.trace().effects(), events)
}

/// [`effects`] over a list of this tick's effects: an "on" event when an
/// effect starts, an "off" event when it has been gone for
/// [`EFFECT_HOLD_TICKS`] (momentary effects have none), nothing for flicker.
fn effect_changes(
    body: &mut Body,
    id: u32,
    number: u64,
    effects: &[tore_sim::flight::trace::Effect],
    events: &mut Vec<Event>,
) -> bool {
    let mut changed = false;
    for effect in effects {
        let key = trees::effect_key(effect);
        if let Some((_, episode)) = body.effects.iter_mut().find(|(k, _)| *k == key) {
            episode.last = number;
            continue;
        }
        let line = trees::effect_line(effect);
        let momentary = trees::momentary(effect);
        let mut event = Event::new(kind::FLIGHT_EFFECT)
            .with_subject(id)
            .with(field::EFFECT, line.label.as_str())
            .with(field::ON, true)
            .with(field::FACTOR, line.factor.as_str());
        if !line.because.is_empty() {
            event = event.with(field::REASON, line.because.as_str());
        }
        if momentary {
            event = event.with(field::MOMENTARY, true);
        }
        events.push(event);
        body.effects.push((
            key,
            Episode {
                last: number,
                label: line.label,
                momentary,
            },
        ));
        changed = true;
    }
    body.effects.retain(|(_, episode)| {
        if number.saturating_sub(episode.last) < EFFECT_HOLD_TICKS {
            return true;
        }
        if !episode.momentary {
            events.push(
                Event::new(kind::FLIGHT_EFFECT)
                    .with_subject(id)
                    .with(field::EFFECT, episode.label.as_str())
                    .with(field::ON, false),
            );
            changed = true;
        }
        false
    });
    changed
}

/// A `flight.g_limit` when the pitch stick reaches its stop: the pilot asks
/// for all the aircraft offers and gets the limit.
fn limits(body: &mut Body, id: u32, number: u64, f: &flight::State, events: &mut Vec<Event>) {
    let trace = f.trace();
    let Some(a) = trace.adapter.as_ref().filter(|_| trace.flew()) else {
        body.pinned = false;
        return;
    };
    let e = a.envelope;
    let pinned = e.stick.abs() >= STICK_STOP;
    if pinned && !body.pinned && number >= body.rearm && body.seen {
        let positive = e.stick > 0.;
        let (asked, limit) = if positive {
            (e.envelope_g[1], e.limits_g[1])
        } else {
            (e.envelope_g[0], e.limits_g[0])
        };
        let reason = if positive {
            trees::g_limit_reason(trace)
        } else {
            format!(
                "negative envelope {} G, fuel and stores divide by {}",
                trees::fixed(e.envelope_g[0], 0),
                trees::fixed(e.load_divisor, 2)
            )
        };
        events.push(
            Event::new(kind::FLIGHT_G_LIMIT)
                .with_subject(id)
                .with(field::ASKED, trees::round(asked, 2))
                .with(field::LIMIT, trees::round(limit, 2))
                .with(field::G, trees::round(f.g, 2))
                .with(field::REASON, reason),
        );
    }
    if !pinned && body.pinned {
        body.rearm = number + G_LIMIT_REARM_TICKS;
    }
    body.pinned = pinned;
}

/// A `flight.structural_failure` when the structure fails.
fn failure(
    body: &mut Body,
    id: u32,
    player: u32,
    state: &AircraftState,
    f: &flight::State,
    tick: &Tick<'_>,
    events: &mut Vec<Event>,
) {
    let failed = f.systems.structure.failed;
    if failed && !body.failed && body.seen {
        let hit = id == player
            && tick.events.iter().any(|e| {
                matches!(e, live::Event::SubsystemDamaged { aircraft, index: 26 } if *aircraft == id)
            });
        let reason = if hit {
            "a hit destroyed the structure"
        } else if f.systems.structure.burning() {
            "a fire burned through"
        } else if f.g.abs() > (9. * (1. - f.damage_fraction)).max(2.) {
            "G load on a weakened airframe"
        } else {
            "damage"
        };
        let mut event = Event::new(kind::FLIGHT_STRUCTURAL_FAILURE)
            .with_subject(id)
            .with(field::G, trees::round(f.g, 2))
            .with(field::REASON, reason);
        if let Some(section) = state.structural_section {
            event = event.with(field::SECTION, i64::from(section));
        }
        events.push(event);
    }
    body.failed = failed;
}

#[cfg(test)]
#[path = "why_tests.rs"]
mod tests;
