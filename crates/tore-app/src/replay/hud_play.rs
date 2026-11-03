//! The replay HUD camera and Sensor View, drawn by the live HUD and the live
//! sensor console from a recorded sample (nr-replayhud). Range, closure and
//! aspect are worked out from the target's recorded pose. The bomb sight and
//! the guided sight are worked out again from the recorded delivery mode.
//! A recording with no sample keeps the derived picture.

use crate::flight::State;
use crate::sensor_console::{self, GunLine, Pipper};
use crate::snapshot::RenderSnapshot;
use crate::weapon_hud;
use std::borrow::Cow;
use tore_formats::aircraft::AircraftId;
use tore_formats::font::Font;
use tore_replay::display::{HudSample, SightSample};
use tore_sim::attitude::Vector;
use tore_sim::combat::delivery::{self, Clock, DeliveryMode, Situation};
use tore_sim::combat::guided::{self, Advisory, GuidedLevel, GuidedSight};
use tore_sim::combat::gunship::NAMES;
use tore_sim::combat::gunship_impact::{self, Shot};
use tore_sim::combat::live::{self, Configuration, Launcher, Readiness};
use tore_sim::combat::missiles::{self, FiringBand, LaunchMode, seeker};
use tore_sim::sensors::{Channel, Contact};
use tore_world::frame::{FlightFrame, ReadoutSlot};
use tore_world::readout::{
    AirportReadout, BombSight, CockpitReadout, Countermeasures, DamageReadout, Estimates,
    LaserReadout, MusicReadout, RwrReadout, SeekerReadout, SensorReadout, Stores, TargetRow,
    Targets,
};
use tore_world::seats::{PlaneId, SeatId};
use tore_world::snapshot::Damage;

/// Where the recorded target is, for the box and the range line.
#[derive(Clone, Copy, Debug)]
pub struct TargetView {
    pub id: u32,
    pub position: Vector,
    pub velocity: Vector,
    pub aircraft: Option<AircraftId>,
}

/// Draws the live weapon HUD for `sample` over `pixels` (640 by 480).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixels: &mut [u8],
    state: &State,
    config: &Configuration,
    sample: &HudSample,
    target: Option<TargetView>,
    aim: Option<crate::aim_box::Marks>,
    font: &Font,
    color: [u8; 3],
    zoom: f64,
    tick: u64,
    falling: Option<f64>,
    laser: Option<LaserReadout>,
    ground: &impl Fn(f64, f64) -> f64,
) {
    let mut state = state.clone();
    if sample.radar {
        state.radar = true;
        state.engine = true;
        state.sensors.channel = Channel::Radar;
    }
    let picture = RenderSnapshot::default();
    let smoke = Default::default();
    let devices = Default::default();
    let readout = readout(&state, config, sample, target, tick, falling, laser, ground);
    let frame = FlightFrame {
        seat: SeatId(0),
        plane: PlaneId(0),
        flight: &state,
        previous: &state,
        presented: Cow::Borrowed(&state),
        picture: &picture,
        smoke: [&smoke, &smoke],
        devices: &devices,
        config,
        readout: ReadoutSlot::ready(readout),
        tick_cues: &[],
    };
    weapon_hud::draw(
        pixels,
        &frame,
        font,
        color,
        zoom,
        false,
        true,
        aim.as_ref(),
        false,
    );
}

/// The same free, pinned or tracked aim box and unavailable-gun diamond as flight.
pub fn aim_marks(sample: &SightSample) -> Option<crate::aim_box::Marks> {
    use crate::aim_box::{Marks, Style};
    let slot = (sample.lead < 3).then_some(usize::from(sample.lead));
    let impact = sample.detail.as_ref().and_then(|detail| {
        slot.filter(|i| detail.impacts[*i].is_some())
            .or_else(|| {
                (0..3).find(|i| sample.link & (1 << i) != 0 && detail.impacts[*i].is_some())
            })
            .filter(|i| crate::aim_box::cannot_hit(readiness(&sample.status[*i])))
            .and_then(|i| detail.impacts[i])
            .map(|i| i.point)
    });
    Some(Marks {
        style: match sample.mode.as_str() {
            "track" => Style::Tracked { friendly: false },
            "point" => Style::Pinned,
            _ => Style::Free,
        },
        aim: sample.point.or(sample.gun_lock)?,
        diamond: impact,
    })
}

/// The gun lines a recorded AC-130 sight shows. A slot with no status and no
/// rounds is not fitted.
pub fn gun_lines(sample: &SightSample) -> ([GunLine; 3], Option<usize>) {
    let lead = (sample.lead < 3).then_some(usize::from(sample.lead));
    let guns = std::array::from_fn(|slot| {
        let fitted = sample.rounds[slot] > 0
            || !sample.status[slot].is_empty()
            || sample.link & (1 << slot) != 0;
        GunLine {
            label: NAMES[slot],
            fitted,
            linked: sample.link & (1 << slot) != 0,
            candidate: lead == Some(slot),
            rounds: fitted.then_some(u32::from(sample.rounds[slot])),
            status: readiness(&sample.status[slot]),
        }
    });
    (guns, lead)
}

/// The lead gun's pipper, from the recorded train. `None` when the trains
/// were not recorded, which is how an older replay and an AI gunship stay.
pub fn pipper(
    sample: &SightSample,
    weapons: &[Option<&tore_formats::weapons::Weapon>; 3],
    launcher: &Launcher,
    tick: u64,
    ground: &impl Fn(f64, f64) -> f64,
) -> Option<Pipper> {
    let slot = (sample.lead < 3).then_some(usize::from(sample.lead))?;
    if let Some(detail) = &sample.detail {
        let impact = detail.impacts[slot]?;
        let weapon = weapons.get(slot)?.as_ref()?;
        let maximum = f64::from(weapon.seeker.zones.first().map_or(1, |z| z.maximum_range)).max(1.);
        let status = readiness(&sample.status[slot]);
        return Some(Pipper {
            point: impact.point,
            range_arc: if impact.spent {
                0.
            } else {
                tore_sim::combat::gunsight::range_arc_fraction(impact.range_ft, maximum)
            },
            dashed: impact.spent || matches!(status, Readiness::GunArc | Readiness::MaximumRange),
            dot: !(impact.spent || status == Readiness::MaximumRange),
        });
    }
    let train = sample.train?;
    let slot = (sample.lead < 3).then_some(usize::from(sample.lead))?;
    let weapon = weapons.get(slot)?.as_ref()?;
    let impact = gunship_impact::impact(
        slot,
        weapon,
        launcher,
        train[slot][0],
        train[slot][1],
        Shot {
            target: None,
            launch_tick: tick,
        },
        ground,
    )?;
    let status = readiness(&sample.status[slot]);
    let spent = matches!(impact, gunship_impact::Impact::Spent { .. });
    let maximum = f64::from(
        weapon
            .seeker
            .zones
            .first()
            .map(|z| z.maximum_range)
            .unwrap_or(1),
    )
    .max(1.);
    Some(Pipper {
        point: impact.point(),
        range_arc: if spent {
            0.
        } else {
            tore_sim::combat::gunsight::range_arc_fraction(impact.range_ft(), maximum)
        },
        dashed: status == Readiness::GunArc || status == Readiness::MaximumRange || spent,
        dot: !(spent || status == Readiness::MaximumRange),
    })
}

/// The pod's short mode word for a recorded sight.
pub fn pod_mode(sample: &SightSample) -> &'static str {
    match sample.mode.as_str() {
        "track" => "TRK",
        "free" | "return" => "FWD",
        _ => "PNT",
    }
}

/// The console's mode word for a recorded sight.
pub fn console_mode(sample: &SightSample) -> sensor_console::Mode {
    match sample.mode.as_str() {
        "track" => sensor_console::Mode::Track,
        "return" => sensor_console::Mode::Returning,
        "free" => sensor_console::Mode::Manual,
        _ => sensor_console::Mode::Point,
    }
}

/// Use the recorded look for the camera: a gunship always, a fighter when
/// the sight holds something. A free fighter sight keeps the derived line.
pub fn use_look(sample: &SightSample) -> bool {
    sample.detail.is_some() || sample.notable()
}

#[allow(clippy::too_many_arguments)]
fn readout(
    state: &State,
    config: &Configuration,
    sample: &HudSample,
    target: Option<TargetView>,
    tick: u64,
    falling: Option<f64>,
    laser: Option<LaserReadout>,
    ground: &impl Fn(f64, f64) -> f64,
) -> CockpitReadout {
    let station = usize::from(sample.station);
    let launcher = crate::combat::launcher(state);
    let observation = target.map(|target| {
        let range = distance(state.position, target.position);
        seeker::Observation {
            id: target.id,
            position: target.position,
            velocity: target.velocity,
            quality: 1.,
            off_axis: 0.,
            range,
        }
    });
    let mut ammo = vec![0u16; config.stations.len().max(station + 1)];
    if let Some(slot) = ammo.get_mut(station) {
        *slot = sample.rounds;
    }
    let guided_sight = config.stations.get(station).and_then(|station| {
        guided_of(&station.weapon, &launcher, target, &sample.delivery, ground)
    });
    let bomb = if guided_sight.is_none() {
        bomb_of(config, sample, &launcher, target, tick, falling, ground)
    } else {
        None
    };
    let contact = target.map(|target| Contact {
        id: target.id,
        channel: if sample.radar {
            Channel::Radar
        } else {
            Channel::Visual
        },
        bearing_rad: 0.,
        elevation_rad: 0.,
        distance_ft: distance(state.position, target.position),
        position: target.position,
        velocity: target.velocity,
        track_eligible: true,
        destroyed: false,
    });
    CockpitReadout {
        plane: 0,
        tick,
        stores: Stores {
            selected: sample.station,
            armed: sample.armed,
            launch_mode: if sample.launch == "bore" {
                LaunchMode::Boresight
            } else {
                LaunchMode::Cued
            },
            ammo,
            loaded: 0,
            gun_aim: [0.; 6],
            gun_group: 0,
        },
        gunsight: None,
        sight: None,
        laser,
        guided: guided_sight,
        weapon_tti: Vec::new(),
        guided_impact_tick: None,
        seeker: SeekerReadout {
            status: seeker_status(&sample.seeker),
            target: sample.target,
            observation,
            tone: None,
        },
        estimates: Estimates {
            readiness: readiness(&sample.readiness),
            guidance_available: sample.guidance,
            can_lock: matches!(sample.seeker.as_str(), "LOCK" | "ACQUIRING" | "PITBULL"),
            observation,
            min_range: (sample.min_ft > 0.).then_some(sample.min_ft),
            max_range: (sample.max_ft > 0.).then_some(sample.max_ft),
            band: (sample.band_max > 0.).then_some(FiringBand {
                minimum: sample.band_min,
                maximum: sample.band_max,
            }),
            in_range: sample.in_range,
            hit_percent: sample.hit,
            solution_seconds: None,
            harm: None,
        },
        bomb_sight: bomb,
        targets: Targets {
            designated: sample.target,
            display: target.map(target_row),
            view: target.map(target_row),
        },
        sensors: SensorReadout {
            tick,
            selected: sample.target,
            acquired: sample.target.filter(|_| sample.seeker == "LOCK"),
            selected_support: None,
            available: [true, false, true],
            operating: [sample.radar, false, true],
            radar_track_nmi: None,
            contacts: contact.into_iter().collect(),
            plots: Vec::new(),
            strobes: Vec::new(),
            trails: Vec::new(),
            trail_points: Vec::new(),
            air_ground: false,
            harm: false,
            ground: Vec::new(),
        },
        visual: Vec::new(),
        map: Vec::new(),
        rwr: RwrReadout::default(),
        damage: DamageReadout {
            hp: 1,
            damage: 0,
            subsystem_counts: [0; 45],
            last_subsystem: None,
            radar_failed: false,
            visual_failed: false,
            infrared_failed: false,
            rwr_failed: false,
            ecm_failed: false,
            shots: 0,
            hits: 0,
            kills: 0,
        },
        countermeasures: Countermeasures::default(),
        airport: AirportReadout {
            nav_mode: sample.nav,
            service: None,
        },
        target_window: None,
        music: MusicReadout::default(),
        link: Default::default(),
    }
}

fn target_row(target: TargetView) -> TargetRow {
    TargetRow {
        id: target.id,
        aircraft: target.aircraft,
        position: target.position,
        velocity: target.velocity,
        damage: Damage {
            hp: 1,
            initial_hp: 1,
            sections: [0; live::DAMAGE_SECTIONS],
            structural: None,
        },
    }
}

fn guided_of(
    weapon: &tore_formats::weapons::Weapon,
    launcher: &Launcher,
    target: Option<TargetView>,
    delivery: &str,
    ground: &impl Fn(f64, f64) -> f64,
) -> Option<GuidedSight> {
    let kind = guided::kind(weapon)?;
    let aim = target.map(|target| target.position);
    let ground_y = aim
        .map(|point| point[1])
        .unwrap_or_else(|| ground(launcher.position[0], launcher.position[2]));
    let reach = guided::reach(weapon, launcher, ground_y, 1.);
    let ground_range = aim.map(|point| guided::ground_range(point, launcher.position));
    let slant = aim.map(|point| distance(point, launcher.position));
    let in_reach = aim.is_some_and(|point| {
        guided::in_release_zone(weapon, launcher.position, launcher.basis, point, 1.)
    });
    let advisory = if aim.is_none() {
        Some(Advisory::NoTarget)
    } else if !in_reach {
        Some(Advisory::OutOfReach)
    } else {
        None
    };
    let level = (delivery == "LVL" && guided::is_glide_bomb(weapon) && aim.is_some())
        .then(|| {
            let steering_rad = guided::steering_to(launcher, aim?)?;
            Some(GuidedLevel {
                steering_rad,
                ground_speed_fps: launcher.velocity[0].hypot(launcher.velocity[2]),
            })
        })
        .flatten();
    Some(GuidedSight {
        energy_intercept: None,
        kind,
        aim,
        buddy: false,
        ground_range_ft: ground_range,
        slant_range_ft: slant,
        reach,
        in_reach,
        advisory,
        level,
    })
}

fn bomb_of(
    config: &Configuration,
    sample: &HudSample,
    launcher: &Launcher,
    target: Option<TargetView>,
    tick: u64,
    falling: Option<f64>,
    ground: &impl Fn(f64, f64) -> f64,
) -> Option<BombSight> {
    let index = usize::from(sample.station);
    let station = config.stations.get(index)?;
    let weapon = &station.weapon;
    if live::is_gun(weapon)
        || guided::kind(weapon).is_some()
        || missiles::Profile::for_weapon(weapon).is_some()
    {
        return None;
    }
    let mode = if sample.delivery == "LVL" {
        DeliveryMode::Level
    } else {
        DeliveryMode::Ccip
    };
    let solved = delivery::solve(
        Situation {
            index,
            station,
            mount: if sample.mount == [0.; 3] {
                station.mount
            } else {
                sample.mount
            },
            launcher,
            mode,
            target: target.map(|target| target.position),
            target_velocity: target.map(|target| target.velocity).unwrap_or([0.; 3]),
            clock: Clock {
                tick,
                service: 0,
                remainder: 0,
            },
            falling,
            ballistic: false,
            beyond_life: true,
        },
        ground,
    )?;
    Some(BombSight::of(&solved))
}

fn distance(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

fn seeker_status(label: &str) -> seeker::Status {
    use seeker::Status::*;
    match label {
        "DUMB" => Unguided,
        "MIDCOURSE" => Midcourse,
        "ACQUIRING" => Acquiring,
        "LOCK" => Locked,
        "PITBULL" => Pitbull,
        "MEMORY" => Memory,
        "LOST" => Lost,
        "EXPIRED" => Expired,
        _ => Search,
    }
}

pub(crate) fn readiness(label: &str) -> Readiness {
    use Readiness::*;
    match label {
        "SAFE" => Safe,
        "OPENING BAY" => BayClosed,
        "LAUNCHER LOST" => LauncherLost,
        "STATION FAILED" => StationFailed,
        "EMPTY" => Empty,
        "PROJECTILE LIMIT" => Capacity,
        "NO TARGET" => NoTarget,
        "TARGET TYPE" => WrongTarget,
        "TARGET DESTROYED" => TargetDestroyed,
        "NO RADAR" => NoRadar,
        "RADAR OFF" => RadarOff,
        "RADAR FAILED" => RadarFailed,
        "RADAR COVERAGE" => RadarCoverage,
        "RWS SEARCH ONLY" => RadarSearchOnly,
        "ACQUIRING" => RadarAcquiring,
        "MIN RANGE" => MinimumRange,
        "MAX RANGE" => MaximumRange,
        "ALTITUDE LIMIT" => Altitude,
        "SEEKER FOV" => FieldOfView,
        "CANNOT BEAR" => GunArc,
        "SLEWING" => GunSlewing,
        "GROUP EMPTY" => GroupEmpty,
        "NO LINE OF FIRE" => GunObscured,
        "TERRAIN MASK" => TerrainMask,
        "NO EMITTER" => NoEmitter,
        "NO TRACK" => TrackLost,
        _ => Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_formats::font::{Font, Glyph};
    use tore_sim::attitude::Basis;

    fn font() -> Font {
        Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| Glyph {
                    advance: 6,
                    pixels: vec![(0, 0), (1, 0), (0, 1)],
                })
                .collect(),
        }
    }

    fn ink(pixels: &[u8]) -> usize {
        pixels.chunks(4).filter(|p| p[3] > 0 || p[1] > 0).count()
    }

    #[test]
    fn an_a2a_lock_draws_the_weapon_hud_and_not_nav() {
        let (combat, flight) = tore_world::combat::fixtures::loaded([2, 0]);
        let mut config = combat.own().configuration().clone();
        let station = config.stations.len() - 1;
        let weapon = &mut config.stations[station].weapon;
        // The fixture's missile is synthetic. The live HUD only draws a
        // reviewed identity, so the test stamps the AIM-9M name and heat seeker.
        weapon.source = "AIM9M.JT".into();
        weapon.flags = 0x41;
        weapon.seeker.signature = 2;
        assert!(missiles::Profile::for_weapon(weapon).is_some());
        let weapon = config.stations[station].clone();
        let mut state = flight;
        state.position = [0., 10_000., 0.];
        state.velocity = [0., 0., 800.];
        state.speed = 800.;
        let sample = HudSample {
            nav: false,
            armed: true,
            station: station as u8,
            weapon: weapon.weapon.source.clone(),
            rounds: 2,
            launch: "cued".into(),
            seeker: "LOCK".into(),
            target: Some(7),
            display_target: None,
            delivery: String::new(),
            readiness: "READY".into(),
            guidance: true,
            in_range: true,
            hit: 40,
            min_ft: 1_500.,
            max_ft: 60_000.,
            band_min: 2_000.,
            band_max: 30_000.,
            radar: true,
            mount: weapon.mount,
        };
        let target = TargetView {
            id: 7,
            position: [0., 10_000., 12_000.],
            velocity: [0., 0., 400.],
            aircraft: None,
        };
        let mut pixels = vec![0u8; 640 * 480 * 4];
        draw(
            &mut pixels,
            &state,
            &config,
            &sample,
            Some(target),
            None,
            &font(),
            [0, 255, 0],
            1.,
            120,
            None,
            None,
            &|_, _| 0.,
        );
        let locked = ink(&pixels);
        let nav = HudSample {
            nav: true,
            armed: false,
            ..sample
        };
        let mut plain = vec![0u8; 640 * 480 * 4];
        draw(
            &mut plain,
            &state,
            &config,
            &nav,
            Some(target),
            None,
            &font(),
            [0, 255, 0],
            1.,
            120,
            None,
            None,
            &|_, _| 0.,
        );
        assert!(
            locked > ink(&plain),
            "the lock HUD draws the weapon, the range and the box"
        );
        assert!(
            locked > 80,
            "weapon text, the seeker circle and the target box"
        );
    }

    #[test]
    fn recorded_pipper_is_the_live_pipper_even_when_no_ballistic_recalculation_is_possible() {
        use tore_sim::combat::gunship_impact::Impact;
        let (combat, mut flight) = tore_world::combat::fixtures::loaded([200, 2]);
        flight.position = [1000., 8000., 2000.];
        let launcher = crate::combat::launcher(&flight);
        let mut config = combat.own().configuration().clone();
        config.stations[0].weapon.source = "C_25.JT".into();
        let mut readout = combat.cockpit_readout(0, launcher, None, None).unwrap();
        readout.stores.selected = 0;
        readout.stores.gun_group = 1;
        readout.gunsight = Some(tore_world::readout::GunsightReadout {
            sight: tore_sim::combat::gunship::Sight::Pinned([-9000., 10., 2000.]),
            look: [-1.5, -0.5],
            zoom: 5,
            returning: false,
            aim: Some([-9000., 10., 2000.]),
            impacts: [
                Some(Impact::Air {
                    point: [-9001.125, 10.25, 2002.75],
                    range_ft: 12000.5,
                    seconds: 1.25,
                }),
                None,
                None,
            ],
            impacts_tick: 123,
            status: [Readiness::GunSlewing; 3],
            notice: None,
            gun_lock: None,
        });
        let page = crate::instruments::gunsight::Page::new(&readout, &config, &flight).unwrap();
        let live = sensor_console::Symbology::live(
            &page,
            None,
            &readout,
            &config,
            &flight,
            None,
            0.,
            sensor_console::SensorMode::WhiteHot,
            1.,
            None,
            1,
        );
        let (_, sample) = tore_session::client::convert::display::from_readout(
            &readout,
            false,
            1,
            |i| config.stations.get(i).map(|s| s.weapon.source.clone()),
            |i| config.stations.get(i).map(|s| s.mount),
        );
        let sample = SightSample::parse(&sample.unwrap().event(0)).unwrap();
        let weapons = [Some(&config.stations[0].weapon), None, None];
        let replay = pipper(&sample, &weapons, &launcher, 9999, &|_, _| {
            panic!("a recorded impact must not be recomputed")
        });
        assert_eq!(replay, live.pipper);
        let marks = aim_marks(&sample).unwrap();
        assert_eq!(marks.style, crate::aim_box::Style::Pinned);
        assert_eq!(marks.diamond, live.pipper.map(|p| p.point));
        let camera = crate::gunsight_view::camera(&crate::gunsight_view::free_view(
            &launcher, page.look, page.zoom,
        ));
        assert_eq!(
            camera.locate([1920, 1080], replay.unwrap().point),
            camera.locate([1920, 1080], live.pipper.unwrap().point)
        );
        let mut absent = sample;
        absent.detail.as_mut().unwrap().impacts = [None; 3];
        assert!(
            pipper(&absent, &weapons, &launcher, 9999, &|_, _| panic!(
                "missing means absent"
            ))
            .is_none()
        );
    }

    #[test]
    fn an_ac130_sight_keeps_rounds_the_lead_gun_and_a_pipper() {
        let sample = SightSample {
            gunship: true,
            az: -1.2,
            el: -0.4,
            zoom: 2,
            mode: "track".into(),
            point: Some([100., 0., 200.]),
            tracked: Some(4),
            rounds: [200, 40, 10],
            link: 0b001,
            lead: 0,
            status: ["READY".into(), "READY".into(), "EMPTY".into()],
            train: Some([[-1.2, -0.6], [0., 0.], [0., 0.]]),
            radar: true,
            range: 1,
            gun_lock: None,
            detail: None,
        };
        let (guns, lead) = gun_lines(&sample);
        assert_eq!(lead, Some(0));
        assert_eq!(guns[0].rounds, Some(200));
        assert_eq!(guns[1].rounds, Some(40));
        assert_eq!(guns[2].rounds, Some(10));
        assert!(guns[0].linked && guns[0].candidate);
        assert_eq!(guns[0].label, "25MM");
        assert_eq!(console_mode(&sample), sensor_console::Mode::Track);
        assert!(use_look(&sample));
        let quiet = SightSample {
            train: None,
            ..sample.clone()
        };
        let launcher = Launcher {
            position: [0., 8_000., 0.],
            basis: Basis::new(0., 0., 0.),
            speed_fps: 300.,
            velocity: [0., 0., 300.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Default::default(),
        };
        assert!(pipper(&quiet, &[None, None, None], &launcher, 0, &|_, _| 0.).is_none());
    }
}
