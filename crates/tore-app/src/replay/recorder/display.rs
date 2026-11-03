//! Human HUD and sight records at 24 Hz; existing sparse records for other aircraft
//! (nr-replayhud). Reads are of state the tick already computed.
//!
//! An ownship is sampled from the cockpit readout the live HUD draws. An AI
//! aircraft has no ownship, so its sample comes from its strike cockpit and
//! its weapon phase. An AI AC-130 has no gunsight of its own: the rounds and
//! the target are recorded, and the trains are not, so the replay does not
//! invent pippers. Opinionated feature requested by John on 2026-10-16; the
//! phase-to-seeker words are an agent decision.

use super::{Recorder, Tick};
use crate::combat;
use crate::snapshot::AircraftPose;
use std::collections::BTreeSet;
use tore_replay::Event;
use tore_replay::display::{HudSample, SightSample};
use tore_sim::ai::weapon_service::Phase;
use tore_sim::attitude::Basis;
use tore_sim::combat::delivery::DeliveryMode;
use tore_sim::combat::guided;
use tore_sim::combat::gunship;
use tore_sim::combat::live::{self, Launcher, Readiness};
use tore_sim::combat::missiles::{self, LaunchMode};
use tore_sim::combat::sight::{Sight, SightMode};

impl Recorder {
    /// `aircraft.hud` and `aircraft.sight` for this tick.
    pub(super) fn display_events(&mut self, tick: &Tick<'_>, number: u64) -> Vec<Event> {
        let mut sampled = BTreeSet::new();
        self.sample_seat(
            tick,
            number,
            self.player,
            combat::launcher(tick.flight),
            &mut sampled,
        );
        for crew in tick.others {
            self.sample_seat(
                tick,
                number,
                crew.plane,
                combat::launcher(crew.flight),
                &mut sampled,
            );
        }
        let ids: Vec<u32> = tick
            .combat
            .state
            .ownships()
            .iter()
            .map(|own| own.aircraft)
            .filter(|id| !sampled.contains(id))
            .collect();
        for id in ids {
            let Some(pose) = pose_of(tick, id) else {
                continue;
            };
            self.sample_seat(tick, number, id, launcher_of(pose), &mut sampled);
        }
        self.sample_ai(tick, number, &sampled);
        self.display.take_events(number, |_, _| None)
    }

    fn sample_seat(
        &mut self,
        tick: &Tick<'_>,
        number: u64,
        id: u32,
        launcher: Launcher,
        sampled: &mut BTreeSet<u32>,
    ) {
        let Some(load) = loadout(&tick.combat.state, id) else {
            return;
        };
        let ground = |x: f64, z: f64| f64::from(tick.world.height(x as f32, z as f32));
        let Some(readout) =
            tick.combat
                .cockpit_readout_with_terrain(id, launcher, tick.wings, None, Some(&ground))
        else {
            return;
        };
        self.display.observe(
            id,
            number,
            &readout,
            load.nav,
            load.range,
            |index| {
                load.stations
                    .get(index)
                    .map(|station| station.weapon.source.clone())
            },
            |index| load.stations.get(index).map(|station| station.mount),
        );
        sampled.insert(id);
    }

    fn sample_ai(&mut self, tick: &Tick<'_>, number: u64, sampled: &BTreeSet<u32>) {
        let Some(wings) = tick.wings else {
            return;
        };
        for actor in wings.mission().actors() {
            let id = actor.id();
            if sampled.contains(&id) || tick.combat.state.ownship(id).is_some() {
                continue;
            }
            let Some(config) = wings.configuration(id) else {
                continue;
            };
            let cockpit = tick.combat.state.strike_cockpit(id);
            let (hud, sight) = ai_samples(actor, config, cockpit);
            self.display.note_samples(id, number, hud, sight);
        }
    }
}

struct Loadout<'a> {
    nav: bool,
    range: u8,
    /// The stations, whose weapon names and mounts the sample asks for by
    /// index, only for the one it names.
    stations: &'a [live::Station],
}

fn loadout(state: &live::State, id: u32) -> Option<Loadout<'_>> {
    let own = state.ownship(id)?;
    Some(Loadout {
        nav: !own.cockpit.armed,
        range: u8::try_from(own.sensors.controls.range_index).unwrap_or(1),
        stations: &own.configuration().stations,
    })
}

fn pose_of<'a>(tick: &'a Tick<'_>, id: u32) -> Option<&'a AircraftPose> {
    if tick.snapshot.player.id == id {
        Some(&tick.snapshot.player)
    } else {
        tick.snapshot.target(id)
    }
}

/// A launcher for an ownship whose flight state was not on the tick. The
/// radar flag stays off: the readout's own sensor flags still say whether
/// the radar was operating.
fn launcher_of(pose: &AircraftPose) -> Launcher {
    let speed = pose.velocity[0]
        .hypot(pose.velocity[1])
        .hypot(pose.velocity[2]);
    Launcher {
        position: pose.position,
        basis: Basis::new(pose.attitude[0], pose.attitude[1], pose.attitude[2]),
        speed_fps: speed,
        velocity: pose.velocity,
        bay_ready: true,
        radar_power: false,
        radar: false,
        jammer: false,
        alive: !pose.crashed,
        body_present: true,
        controls: Default::default(),
    }
}

fn ai_samples(
    actor: &tore_sim::ai::mission::AiActor,
    config: &live::Configuration,
    cockpit: Option<&live::StrikeCockpit>,
) -> (HudSample, Option<SightSample>) {
    let controller = actor.controller();
    let phase = controller.weapon_phase();
    let trace = controller.trace().weapons.as_ref();
    let target = cockpit
        .and_then(|cockpit| cockpit.target_object())
        .or_else(|| controller.target());
    let armed = cockpit.is_some_and(|cockpit| cockpit.armed)
        || (target.is_some() && phase != Phase::Search);
    let station = cockpit
        .map(|cockpit| cockpit.selected)
        .or_else(|| {
            trace
                .and_then(|trace| trace.chosen)
                .map(|id| usize::from(id.0))
        })
        .unwrap_or(0);
    let carried = config.stations.get(station);
    let weapon = carried
        .map(|station| station.weapon.source.clone())
        .unwrap_or_default();
    let rounds = carried.map(|station| station.count).unwrap_or(0);
    let seeker = if trace.is_some_and(|trace| trace.lock.locked)
        || matches!(phase, Phase::Tracking | Phase::Fire)
    {
        "LOCK"
    } else if matches!(phase, Phase::Prepare | Phase::LockCheck) {
        "ACQUIRING"
    } else {
        "ACTIVE SEARCH"
    };
    let delivery = carried
        .map(|station| delivery_word(&station.weapon, cockpit.map(|c| c.delivery_mode)))
        .unwrap_or_default();
    let launch = match cockpit.map(|cockpit| cockpit.launch_mode) {
        Some(LaunchMode::Boresight) => "bore",
        _ => "cued",
    };
    let hud = HudSample {
        nav: !armed,
        armed,
        station: u8::try_from(station).unwrap_or(0),
        weapon,
        rounds,
        launch: launch.into(),
        seeker: seeker.into(),
        target,
        display_target: None,
        delivery,
        readiness: Readiness::Ready.label().into(),
        guidance: false,
        in_range: false,
        hit: 0,
        min_ft: 0.,
        max_ft: 0.,
        band_min: 0.,
        band_max: 0.,
        radar: false,
        mount: carried.map(|station| station.mount).unwrap_or([0.; 3]),
    };
    let sight = if gunship::has_linked_guns(config.aircraft) {
        Some(ai_gunship(config, cockpit, station))
    } else {
        cockpit
            .and_then(|cockpit| cockpit.sight.as_ref())
            .map(sight_of)
    };
    (hud, sight)
}

fn delivery_word(weapon: &tore_formats::weapons::Weapon, mode: Option<DeliveryMode>) -> String {
    if live::is_gun(weapon) {
        return String::new();
    }
    if guided::kind(weapon).is_some() || missiles::Profile::for_weapon(weapon).is_none() {
        return mode.unwrap_or_default().label().into();
    }
    String::new()
}

fn sight_of(sight: &Sight) -> SightSample {
    let mode = if sight.returning {
        "return".into()
    } else {
        match sight.mode {
            SightMode::Free => "free",
            SightMode::Pinned(_) => "point",
            SightMode::Tracked(_) => "track",
        }
        .into()
    };
    SightSample {
        az: sight.look[0],
        el: sight.look[1],
        zoom: sight.zoom().max(1),
        mode,
        point: match sight.mode {
            SightMode::Pinned(point) => Some(point),
            SightMode::Tracked(_) => sight.aim,
            SightMode::Free => None,
        },
        tracked: sight.mode.tracked(),
        ..SightSample::default()
    }
}

fn ai_gunship(
    config: &live::Configuration,
    cockpit: Option<&live::StrikeCockpit>,
    selected: usize,
) -> SightSample {
    let mut rounds = [0u16; 3];
    let mut lead = 255u8;
    for (index, station) in config.stations.iter().enumerate() {
        if let Some(slot) = gunship::GUNS
            .iter()
            .position(|name| station.weapon.source.eq_ignore_ascii_case(name))
        {
            rounds[slot] = station.count;
            if index == selected {
                lead = slot as u8;
            }
        }
    }
    let look = cockpit
        .and_then(|cockpit| cockpit.sight.as_ref())
        .map(|sight| sight.look)
        .unwrap_or(gunship::DEFAULT_LOOK);
    let mut sight = cockpit
        .and_then(|cockpit| cockpit.sight.as_ref())
        .map(sight_of)
        .unwrap_or_default();
    sight.gunship = true;
    sight.az = look[0];
    sight.el = look[1];
    sight.rounds = rounds;
    sight.lead = lead;
    sight.link = if lead == 255 { 0 } else { 1 << lead };
    sight.status = std::array::from_fn(|slot| {
        if rounds[slot] > 0 {
            Readiness::Ready.label().into()
        } else {
            Readiness::Empty.label().into()
        }
    });
    // No trains: an AI gunship does not keep the player's gunsight.
    sight.train = None;
    sight
}
