//! The local seat's HUD and sight, taken from the readout the client already
//! receives (nr-replayhud). Nothing new goes on the wire. Other players'
//! cockpits are not in a client capture, so a client recording has these
//! events for the local seat only. The range step is not in the readout:
//! a client recording uses the radar's default step.
//!
//! The host recorder uses the same samples. It can name the weapon and the
//! mount, which a capture fills in later from the mission loadout.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};

use tore_replay::Event;
use tore_replay::display::{
    DISPLAY_TICKS, HudSample, ImpactSample, RadarContact, SIGHT_GAP_TICKS, SightDetail,
    SightSample, TargetSample,
};
use tore_sim::combat::gunship::{self, Sight as GunSight};
use tore_sim::combat::missiles::LaunchMode;
use tore_sim::combat::sight::SightMode;
use tore_sim::sensors::DEFAULT_RANGE_INDEX;
use tore_world::readout::CockpitReadout;

/// Fixed-rate cockpit samples and legacy sparse samples, indexed by aircraft.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    last: BTreeMap<u32, Held>,
    /// Changes by the tick they belong on.
    changes: BTreeMap<u64, Vec<Change>>,
}

#[derive(Clone, Debug)]
struct Held {
    hud: HudSample,
    sight: Option<SightSample>,
    sight_at: u64,
}

#[derive(Clone, Debug)]
struct Change {
    aircraft: u32,
    hud: Option<HudSample>,
    sight: Option<SightSample>,
}

impl Seen {
    /// Sample a decoded readout once per five-tick interval. `nav` is the HUD's
    /// NAV flag (the readout's own, or the strike cockpit's armed bit
    /// inverted when the world cockpit was not part of the readout).
    /// `range_index` is the scope step; pass [`DEFAULT_RANGE_INDEX`] when
    /// the wire did not carry it.
    #[allow(clippy::too_many_arguments)]
    pub fn observe<S, M>(
        &mut self,
        aircraft: u32,
        tick: u64,
        readout: &CockpitReadout,
        nav: bool,
        range_index: u8,
        weapon_at: S,
        mount_at: M,
    ) where
        S: Fn(usize) -> Option<String>,
        M: Fn(usize) -> Option<[f64; 3]>,
    {
        if self
            .last
            .get(&aircraft)
            .is_some_and(|last| last.sight_at / DISPLAY_TICKS == tick / DISPLAY_TICKS)
        {
            return;
        }
        let (hud, sight) = from_readout(readout, nav, range_index, weapon_at, mount_at);
        self.changes.entry(tick).or_default().push(Change {
            aircraft,
            hud: Some(hud.clone()),
            sight: sight.clone(),
        });
        self.last.insert(
            aircraft,
            Held {
                hud,
                sight,
                sight_at: tick,
            },
        );
    }

    /// The HUD and sight changes written on `tick`, owned so a caller can
    /// name the weapon from a loadout it holds separately.
    pub fn rows(&self, tick: u64) -> Vec<(u32, Option<HudSample>, Option<SightSample>)> {
        self.changes
            .get(&tick)
            .map(|list| {
                list.iter()
                    .map(|change| (change.aircraft, change.hud.clone(), change.sight.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Takes the events whose tick is `tick`, with an empty weapon name
    /// filled from `store` (the mission loadout). Mount stays as sampled.
    /// A recorder takes each tick once, so the change list stays short.
    pub fn take_events(
        &mut self,
        tick: u64,
        mut store: impl FnMut(u32, u8) -> Option<(String, [f64; 3])>,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        let Some(list) = self.changes.remove(&tick) else {
            return events;
        };
        for change in list {
            if let Some(mut hud) = change.hud {
                if hud.weapon.is_empty()
                    && let Some((source, mount)) = store(change.aircraft, hud.station)
                {
                    hud.weapon = source;
                    if hud.mount == [0.; 3] {
                        hud.mount = mount;
                    }
                }
                events.push(hud.event(change.aircraft));
            }
            if let Some(sight) = change.sight {
                events.push(sight.event(change.aircraft));
            }
        }
        events
    }

    /// Record a sample built without a cockpit readout (an AI aircraft).
    pub fn note_samples(
        &mut self,
        aircraft: u32,
        tick: u64,
        hud: HudSample,
        sight: Option<SightSample>,
    ) {
        self.note(aircraft, tick, hud, sight);
    }

    fn note(&mut self, aircraft: u32, tick: u64, hud: HudSample, sight: Option<SightSample>) {
        let Some(prev) = self.last.get(&aircraft) else {
            let sight = sight.filter(SightSample::notable);
            self.changes.entry(tick).or_default().push(Change {
                aircraft,
                hud: Some(hud.clone()),
                sight: sight.clone(),
            });
            self.last.insert(
                aircraft,
                Held {
                    hud,
                    sight,
                    sight_at: tick,
                },
            );
            return;
        };
        let mut hud_event = None;
        let mut sight_event = None;
        let mut next = prev.clone();
        if hud.differs(&prev.hud) {
            hud_event = Some(hud.clone());
            next.hud = hud;
        }
        if let Some(sample) = sight.filter(SightSample::notable) {
            let emit = match &prev.sight {
                None => true,
                Some(old) => {
                    sample.differs(old)
                        && (sample.stepped(old)
                            || tick.saturating_sub(prev.sight_at) >= SIGHT_GAP_TICKS)
                }
            };
            if emit {
                sight_event = Some(sample.clone());
                next.sight = Some(sample);
                next.sight_at = tick;
            }
        }
        if hud_event.is_some() || sight_event.is_some() {
            self.changes.entry(tick).or_default().push(Change {
                aircraft,
                hud: hud_event,
                sight: sight_event,
            });
            self.last.insert(aircraft, next);
        }
    }
}

/// The default range step a client recording writes. The wire carries the
/// air-to-ground flag and not the step.
pub fn default_range_index() -> u8 {
    u8::try_from(DEFAULT_RANGE_INDEX).unwrap_or(1)
}

/// The HUD sample and, when the aircraft has a gunsight or a targeting
/// sight, the sight sample, including free look.
pub fn from_readout<S, M>(
    readout: &CockpitReadout,
    nav: bool,
    range_index: u8,
    weapon_at: S,
    mount_at: M,
) -> (HudSample, Option<SightSample>)
where
    S: Fn(usize) -> Option<String>,
    M: Fn(usize) -> Option<[f64; 3]>,
{
    let station = readout.stores.selected();
    let hud = HudSample {
        nav,
        armed: readout.stores.armed,
        station: readout.stores.selected,
        weapon: weapon_at(station).unwrap_or_default(),
        rounds: readout.stores.rounds(station),
        launch: match readout.stores.launch_mode {
            LaunchMode::Cued => "cued",
            LaunchMode::Boresight => "bore",
        }
        .into(),
        seeker: readout.seeker.status.label().into(),
        target: readout
            .targets
            .display
            .as_ref()
            .map(|row| row.id)
            .or(readout.targets.designated),
        display_target: readout.targets.display.as_ref().map(|row| TargetSample {
            position: row.position,
            velocity: row.velocity,
        }),
        delivery: delivery_word(readout),
        readiness: readout.estimates.readiness.label().into(),
        guidance: readout.estimates.guidance_available,
        in_range: readout.estimates.in_range,
        hit: readout.estimates.hit_percent,
        min_ft: readout.estimates.min_range.unwrap_or(0.),
        max_ft: readout.estimates.max_range.unwrap_or(0.),
        band_min: readout.estimates.band.map(|b| b.minimum).unwrap_or(0.),
        band_max: readout.estimates.band.map(|b| b.maximum).unwrap_or(0.),
        radar: readout.sensors.operating(tore_sim::sensors::Channel::Radar),
        mount: mount_at(station).unwrap_or([0.; 3]),
    };
    let sight = if let Some(guns) = &readout.gunsight {
        Some(gunship_sight(readout, guns, range_index, &weapon_at))
    } else {
        readout.sight.as_ref().map(|sight| SightSample {
            gunship: false,
            az: sight.look[0],
            el: sight.look[1],
            zoom: sight.zoom,
            mode: mode_word(sight.mode, sight.returning),
            // Keep the actual ground intersection even during free slew.
            point: match sight.mode {
                SightMode::Pinned(point) => Some(point),
                SightMode::Tracked(_) => sight.aim,
                SightMode::Free => sight.aim,
            },
            tracked: sight.mode.tracked(),
            detail: Some(SightDetail {
                gimbal_limit: match sight.sensor.gimbal {
                    tore_sim::combat::sight::Gimbal::Hemisphere => {
                        sight.look[1].to_degrees() >= -0.5
                    }
                    tore_sim::combat::sight::Gimbal::Cone { search, .. } => {
                        sight.look[0].hypot(sight.look[1]).to_degrees() >= search.to_degrees() - 0.5
                    }
                } || sight.notice.is_some_and(|n| {
                    n.notice == gunship::Notice::GimbalLimit
                        && n.tick <= readout.tick
                        && readout.tick - n.tick < 30
                }),
                mark: sight.aim,
                ..SightDetail::default()
            }),
            ..SightSample::default()
        })
    };
    (hud, sight)
}

fn delivery_word(readout: &CockpitReadout) -> String {
    if let Some(sight) = &readout.bomb_sight {
        return sight.mode.label().into();
    }
    if readout
        .guided
        .as_ref()
        .is_some_and(|sight| sight.level.is_some())
    {
        return "LVL".into();
    }
    if readout.guided.is_some() {
        return "CCIP".into();
    }
    String::new()
}

fn mode_word(mode: SightMode, returning: bool) -> String {
    if returning {
        return "return".into();
    }
    match mode {
        SightMode::Free => "free",
        SightMode::Pinned(_) => "point",
        SightMode::Tracked(_) => "track",
    }
    .into()
}

fn gunship_sight<S>(
    readout: &CockpitReadout,
    guns: &tore_world::readout::GunsightReadout,
    range_index: u8,
    weapon_at: &S,
) -> SightSample
where
    S: Fn(usize) -> Option<String>,
{
    let slot_of = |station: usize| {
        weapon_at(station).and_then(|source| {
            gunship::GUNS
                .iter()
                .position(|name| source.eq_ignore_ascii_case(name))
        })
    };
    let mut rounds = [0u16; 3];
    for station in 0..readout.stores.ammo.len() {
        if let Some(slot) = slot_of(station) {
            rounds[slot] = readout.stores.rounds(station);
        }
    }
    let link = readout.stores.gun_group;
    let selected = readout.stores.selected();
    let lead = slot_of(selected)
        .map(|slot| slot as u8)
        .or_else(|| {
            (0..3)
                .find(|slot| link & (1 << slot) != 0)
                .map(|slot| slot as u8)
        })
        .unwrap_or(255);
    let train = std::array::from_fn(|slot| {
        [
            readout.stores.gun_aim[slot * 2] * PI,
            readout.stores.gun_aim[slot * 2 + 1] * FRAC_PI_2,
        ]
    });
    let (mode, point, tracked) = match guns.sight {
        GunSight::Pinned(point) => ("point".into(), Some(point), None),
        GunSight::Tracked(id) => ("track".into(), guns.aim, Some(id)),
        GunSight::Free => ("free".into(), guns.aim, None),
    };
    SightSample {
        gunship: true,
        az: guns.look[0],
        el: guns.look[1],
        zoom: guns.zoom.max(1),
        mode: if guns.returning {
            "return".into()
        } else {
            mode
        },
        point,
        tracked,
        rounds,
        link,
        lead,
        status: std::array::from_fn(|slot| guns.status[slot].label().into()),
        train: Some(train),
        radar: readout.sensors.air_ground,
        range: range_index,
        gun_lock: guns.gun_lock,
        detail: Some(SightDetail {
            impacts: guns.impacts.map(|impact| {
                impact.map(|impact| ImpactSample {
                    point: impact.point(),
                    range_ft: impact.range_ft(),
                    spent: matches!(
                        impact,
                        tore_sim::combat::gunship_impact::Impact::Spent { .. }
                    ),
                })
            }),
            gimbal_limit: guns.look[1].to_degrees() >= gunship::GIMBAL_TOP.to_degrees() - 0.5
                || guns.notice.is_some_and(|notice| {
                    notice.notice == gunship::Notice::GimbalLimit
                        && notice.tick <= guns.impacts_tick + 1
                        && guns.impacts_tick + 1 - notice.tick < 30
                }),
            mark: match guns.sight {
                GunSight::Pinned(point) => Some(point),
                GunSight::Tracked(_) => readout.targets.display.as_ref().map(|row| row.position),
                GunSight::Free => None,
            },
            radar: readout
                .sensors
                .ground
                .iter()
                .filter(|g| {
                    readout.sensors.air_ground
                        && g.contact.channel == tore_sim::sensors::Channel::Radar
                })
                .take(128)
                .map(|g| RadarContact {
                    position: g.contact.position,
                    symbol: match g.symbol() {
                        tore_sim::sensors::track::GroundSymbol::Dot => 0,
                        tore_sim::sensors::track::GroundSymbol::Cross => 1,
                        tore_sim::sensors::track::GroundSymbol::Square => 2,
                    },
                    selected: readout.targets.designated == Some(g.id()),
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_replay::vocab::kind;

    #[test]
    fn human_display_samples_are_24_hz_idle_slewing_and_firing() {
        use tore_world::readout::GunsightReadout;
        let (combat, flight) = tore_world::combat::fixtures::loaded([200, 2]);
        let mut readout = combat
            .cockpit_readout(0, tore_world::combat::launcher(&flight), None, None)
            .unwrap();
        readout.gunsight = Some(GunsightReadout {
            sight: GunSight::Free,
            look: [-1.5, -0.5],
            returning: false,
            aim: Some([100., 0., 200.]),
            impacts: [None; 3],
            impacts_tick: 0,
            status: [tore_sim::combat::live::Readiness::Ready; 3],
            notice: None,
            zoom: 3,
            gun_lock: None,
        });
        let mut seen = Seen::default();
        let mut ticks = Vec::new();
        for tick in 0..360 {
            let guns = readout.gunsight.as_mut().unwrap();
            guns.impacts_tick = tick;
            if tick >= 120 {
                guns.look[0] += 0.0001;
            }
            if tick >= 240 {
                readout.stores.ammo[0] = 560 - tick as u16;
            }
            seen.observe(
                0,
                tick,
                &readout,
                false,
                1,
                |_| Some("C_25.JT".into()),
                |_| None,
            );
            let events = seen.take_events(tick, |_, _| None);
            if !events.is_empty() {
                assert_eq!(events.len(), 2);
                assert!(SightSample::parse(&events[1]).unwrap().detail.is_some());
                ticks.push(tick);
            }
        }
        assert_eq!(ticks, (0..360).step_by(5).collect::<Vec<_>>());
        assert!(seen.changes.is_empty());
    }

    #[test]
    fn a_human_free_fighter_sight_keeps_its_look_aim_and_gimbal_limit() {
        use tore_sim::combat::sight::{Gimbal, SightSensor};
        let (combat, flight) = tore_world::combat::fixtures::loaded([200, 2]);
        let mut readout = combat
            .cockpit_readout(0, tore_world::combat::launcher(&flight), None, None)
            .unwrap();
        readout.sight = Some(tore_world::readout::SightReadout {
            mode: SightMode::Free,
            look: [0.3, 0.],
            returning: false,
            aim: Some([1000., 0., 5000.]),
            notice: None,
            zoom: 4,
            sensor: SightSensor {
                laser: true,
                range_ft: 60_000.,
                gimbal: Gimbal::Hemisphere,
                eye: [0., -4., 0.],
                default_look: [0., -0.2],
            },
        });
        let mut seen = Seen::default();
        seen.observe(0, 0, &readout, false, 1, |_| None, |_| None);
        let events = seen.take_events(0, |_, _| None);
        let sight = events.iter().find_map(SightSample::parse).unwrap();
        assert_eq!(sight.mode, "free");
        assert_eq!([sight.az, sight.el], [0.3, 0.]);
        assert_eq!(sight.point, Some([1000., 0., 5000.]));
        assert_eq!(sight.zoom, 4);
        assert!(sight.detail.unwrap().gimbal_limit);
    }

    #[test]
    fn a_ground_display_target_survives_sampling_and_clears() {
        let (combat, flight) = tore_world::combat::fixtures::loaded([200, 2]);
        let mut readout = combat
            .cockpit_readout(0, tore_world::combat::launcher(&flight), None, None)
            .unwrap();
        let target = tore_world::readout::TargetRow {
            id: 0x50000000,
            aircraft: None,
            position: [12_345.5, 4., 6789.25],
            velocity: [2., 0., 1.],
            damage: tore_world::snapshot::Damage {
                hp: 10,
                initial_hp: 10,
                sections: [0; tore_sim::combat::live::DAMAGE_SECTIONS],
                structural: None,
            },
        };
        readout.targets.designated = Some(target.id);
        readout.targets.display = Some(target.clone());
        let (hud, _) = from_readout(&readout, false, 1, |_| None, |_| None);
        let back = HudSample::parse(&hud.event(0)).unwrap();
        assert_eq!(back.target, Some(target.id));
        assert_eq!(
            back.display_target.unwrap(),
            TargetSample {
                position: target.position,
                velocity: target.velocity
            }
        );
        readout.targets.designated = None;
        readout.targets.display = None;
        let (cleared, _) = from_readout(&readout, false, 1, |_| None, |_| None);
        assert!(cleared.display_target.is_none());
    }

    fn sights(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|event| event.kind == kind::AIRCRAFT_SIGHT)
            .count()
    }

    #[test]
    fn a_tracked_point_sliding_every_tick_writes_ten_times_a_second() {
        let mut seen = Seen::default();
        let mut written = 0;
        for tick in 0..120u64 {
            let sight = SightSample {
                mode: "track".into(),
                tracked: Some(5),
                point: Some([tick as f64 * 60., 0., 0.]),
                ..SightSample::default()
            };
            seen.note_samples(1, tick, HudSample::default(), Some(sight));
            written += sights(&seen.take_events(tick, |_, _| None));
        }
        assert_eq!(written, 10, "the first sample, then one every 12 ticks");
        assert!(seen.changes.is_empty(), "taken ticks are not kept");
    }

    #[test]
    fn a_sparse_non_cockpit_free_fighter_sight_is_not_written() {
        let mut seen = Seen::default();
        for tick in 0..24u64 {
            let sight = SightSample {
                az: tick as f64 * 0.01,
                ..SightSample::default()
            };
            seen.note_samples(1, tick, HudSample::default(), Some(sight));
            assert_eq!(sights(&seen.take_events(tick, |_, _| None)), 0);
        }
    }
}
