//! HUD and sight samples. Human cockpits use a five-tick (24 Hz) grid.
//! Older recordings and aircraft without a cockpit retain sparse change events.
//! Camera pointing interpolates between adjacent new samples; symbols hold.

use std::collections::BTreeMap;

use crate::TimedEvent;
pub use crate::display_detail::{ImpactSample, RadarContact, SightDetail, TargetSample};

/// One display sample per five ticks at 120 Hz.
pub const DISPLAY_TICKS: u64 = 5;
use crate::model::{Event, Value};
use crate::vocab::{field, kind};

/// Hit percent is written again when it moves by this many points.
pub const HIT_STEP: u8 = 5;
/// A range or a band edge is written again when it moves by this many feet.
pub const RANGE_STEP_FT: f64 = 100.;
/// A sight angle is written again when it moves by this many radians.
pub const ANGLE_STEP: f64 = 0.004;
/// A held point is written again when it moves by this many feet.
pub const POINT_STEP_FT: f64 = 50.;
/// Sight slews are written at most this often, unless something discrete changed.
pub const SIGHT_GAP_TICKS: u64 = 12;

/// What the HUD was showing for one aircraft.
#[derive(Clone, Debug, PartialEq)]
pub struct HudSample {
    pub nav: bool,
    pub armed: bool,
    pub station: u8,
    /// The selected weapon's source name. Empty when nothing is selected.
    pub weapon: String,
    pub rounds: u16,
    /// `cued` or `bore`.
    pub launch: String,
    /// The seeker status label (`LOCK`, `ACTIVE SEARCH`, and the rest).
    pub seeker: String,
    pub target: Option<u32>,
    pub display_target: Option<TargetSample>,
    /// Empty, `CCIP` or `LVL`.
    pub delivery: String,
    pub readiness: String,
    pub guidance: bool,
    pub in_range: bool,
    pub hit: u8,
    /// Feet. Zero preserves the decoded minimum in older recordings.
    pub min_ft: f64,
    /// Feet. Zero means no maximum was estimated.
    pub max_ft: f64,
    pub band_min: f64,
    /// Feet. Zero means there is no band.
    pub band_max: f64,
    /// The radar was operating, for the gun's RADAR / 1000 FT word.
    pub radar: bool,
    /// The selected station's mount, feet right, up and forward.
    pub mount: [f64; 3],
}

impl Default for HudSample {
    fn default() -> Self {
        Self {
            nav: true,
            armed: false,
            station: 0,
            weapon: String::new(),
            rounds: 0,
            launch: "cued".into(),
            seeker: "ACTIVE SEARCH".into(),
            target: None,
            display_target: None,
            delivery: String::new(),
            readiness: "READY".into(),
            guidance: false,
            in_range: false,
            hit: 0,
            min_ft: 0.,
            max_ft: 0.,
            band_min: 0.,
            band_max: 0.,
            radar: false,
            mount: [0.; 3],
        }
    }
}

impl HudSample {
    /// Whether a player would see a different HUD. Ranges move in steps.
    pub fn differs(&self, other: &Self) -> bool {
        self.nav != other.nav
            || self.armed != other.armed
            || self.station != other.station
            || self.weapon != other.weapon
            || self.rounds != other.rounds
            || self.launch != other.launch
            || self.seeker != other.seeker
            || self.target != other.target
            || self.delivery != other.delivery
            || self.readiness != other.readiness
            || self.guidance != other.guidance
            || self.in_range != other.in_range
            || self.radar != other.radar
            || self.hit.abs_diff(other.hit) >= HIT_STEP
            || stepped(self.min_ft, other.min_ft, RANGE_STEP_FT)
            || stepped(self.max_ft, other.max_ft, RANGE_STEP_FT)
            || stepped(self.band_min, other.band_min, RANGE_STEP_FT)
            || stepped(self.band_max, other.band_max, RANGE_STEP_FT)
            || self
                .mount
                .iter()
                .zip(other.mount)
                .any(|(a, b)| (a - b).abs() >= 1.)
    }

    pub fn event(&self, aircraft: u32) -> Event {
        let mut event = Event::new(kind::AIRCRAFT_HUD)
            .with_subject(aircraft)
            .with(field::NAV, self.nav)
            .with(field::ARMED, self.armed)
            .with(field::STATION, i64::from(self.station))
            .with(field::WEAPON, self.weapon.as_str())
            .with(field::ROUNDS, i64::from(self.rounds))
            .with(field::LAUNCH, self.launch.as_str())
            .with(field::SEEKER, self.seeker.as_str())
            .with(field::DELIVERY, self.delivery.as_str())
            .with(field::READINESS, self.readiness.as_str())
            .with(field::GUIDANCE, self.guidance)
            .with(field::IN_RANGE, self.in_range)
            .with("hit", i64::from(self.hit))
            .with(field::MIN_FT, self.min_ft)
            .with(field::MAX_FT, self.max_ft)
            .with(field::BAND_MIN, self.band_min)
            .with(field::BAND_MAX, self.band_max)
            .with(field::RADAR, self.radar);
        if let Some(id) = self.target {
            event = event.with_object(id);
        }
        if self.mount.iter().any(|v| v.abs() >= 0.5) {
            event = event
                .with(field::MOUNT_X, self.mount[0])
                .with(field::MOUNT_Y, self.mount[1])
                .with(field::MOUNT_Z, self.mount[2]);
        }
        if let Some(target) = &self.display_target {
            event = target.write(event);
        }
        event
    }

    pub fn parse(event: &Event) -> Option<Self> {
        if event.kind != kind::AIRCRAFT_HUD {
            return None;
        }
        Some(Self {
            nav: event.flag(field::NAV).unwrap_or(true),
            armed: event.flag(field::ARMED).unwrap_or(false),
            station: u8_of(event, field::STATION),
            weapon: event.string(field::WEAPON).unwrap_or("").to_owned(),
            rounds: u16_of(event, field::ROUNDS),
            launch: event.string(field::LAUNCH).unwrap_or("cued").to_owned(),
            seeker: event
                .string(field::SEEKER)
                .unwrap_or("ACTIVE SEARCH")
                .to_owned(),
            target: event.object,
            display_target: TargetSample::parse(event),
            delivery: event.string(field::DELIVERY).unwrap_or("").to_owned(),
            readiness: event.string(field::READINESS).unwrap_or("READY").to_owned(),
            guidance: event.flag(field::GUIDANCE).unwrap_or(false),
            in_range: event.flag(field::IN_RANGE).unwrap_or(false),
            hit: u8_of(event, "hit"),
            min_ft: event.num(field::MIN_FT).unwrap_or(0.),
            max_ft: event.num(field::MAX_FT).unwrap_or(0.),
            band_min: event.num(field::BAND_MIN).unwrap_or(0.),
            band_max: event.num(field::BAND_MAX).unwrap_or(0.),
            radar: event.flag(field::RADAR).unwrap_or(false),
            mount: [
                event.num(field::MOUNT_X).unwrap_or(0.),
                event.num(field::MOUNT_Y).unwrap_or(0.),
                event.num(field::MOUNT_Z).unwrap_or(0.),
            ],
        })
    }
}

/// The targeting sight, or the AC-130's gunsight, for one aircraft.
#[derive(Clone, Debug, PartialEq)]
pub struct SightSample {
    pub gunship: bool,
    pub az: f64,
    pub el: f64,
    pub zoom: u8,
    /// `free`, `point`, `track` or `return`.
    pub mode: String,
    pub point: Option<[f64; 3]>,
    pub tracked: Option<u32>,
    pub rounds: [u16; 3],
    pub link: u8,
    /// 255 when no gun is the lead.
    pub lead: u8,
    pub status: [String; 3],
    /// Heading and elevation of each gun, radians. Absent when the trains
    /// were not known (an AI gunship has no gunsight of its own).
    pub train: Option<[[f64; 2]; 3]>,
    pub radar: bool,
    pub range: u8,
    pub gun_lock: Option<[f64; 3]>,
    /// Absent in older, sparse recordings.
    pub detail: Option<SightDetail>,
}

impl Default for SightSample {
    fn default() -> Self {
        Self {
            gunship: false,
            az: 0.,
            el: 0.,
            zoom: 1,
            mode: "free".into(),
            point: None,
            tracked: None,
            rounds: [0; 3],
            link: 0,
            lead: 255,
            status: [String::new(), String::new(), String::new()],
            train: None,
            radar: false,
            range: 1,
            gun_lock: None,
            detail: None,
        }
    }
}

impl SightSample {
    /// A free fighter sight says nothing the derived line of sight does not.
    /// A gunship, a pin, a track or a return does.
    pub fn notable(&self) -> bool {
        self.gunship
            || self.mode != "free"
            || self.point.is_some()
            || self.gun_lock.is_some()
            || self.tracked.is_some()
    }

    /// A change of mode, lock, rounds or radar, or a new pin: written at
    /// once. A tracked aim point that moves is a slew.
    pub fn stepped(&self, other: &Self) -> bool {
        self.gunship != other.gunship
            || self.zoom != other.zoom
            || self.mode != other.mode
            || self.tracked != other.tracked
            || self.rounds != other.rounds
            || self.link != other.link
            || self.lead != other.lead
            || self.status != other.status
            || self.radar != other.radar
            || self.range != other.range
            || self.train.is_some() != other.train.is_some()
            || self.point.is_some() != other.point.is_some()
            || (self.mode == "point" && point_stepped(self.point, other.point))
            || point_stepped(self.gun_lock, other.gun_lock)
    }

    /// A stepped change, or the sight, its aim point and the trains having
    /// slewed.
    pub fn differs(&self, other: &Self) -> bool {
        self.stepped(other)
            || stepped(self.az, other.az, ANGLE_STEP)
            || stepped(self.el, other.el, ANGLE_STEP)
            || point_stepped(self.point, other.point)
            || match (self.train, other.train) {
                (Some(a), Some(b)) => a.iter().zip(b).any(|(p, q)| {
                    stepped(p[0], q[0], ANGLE_STEP) || stepped(p[1], q[1], ANGLE_STEP)
                }),
                _ => false,
            }
    }

    pub fn event(&self, aircraft: u32) -> Event {
        let mut event = Event::new(kind::AIRCRAFT_SIGHT)
            .with_subject(aircraft)
            .with(field::GUNSHIP, self.gunship)
            .with(field::AZ, self.az)
            .with(field::EL, self.el)
            .with(field::ZOOM, i64::from(self.zoom))
            .with(field::MODE, self.mode.as_str());
        if let Some(id) = self.tracked {
            event = event.with_object(id);
        }
        if let Some(point) = self.point {
            event = event
                .with(field::X_FT, point[0])
                .with(field::Y_FT, point[1])
                .with(field::Z_FT, point[2]);
        }
        if self.gunship {
            event = event
                .with("r0", i64::from(self.rounds[0]))
                .with("r1", i64::from(self.rounds[1]))
                .with("r2", i64::from(self.rounds[2]))
                .with(field::LINK, i64::from(self.link))
                .with(field::LEAD, i64::from(self.lead))
                .with("s0", self.status[0].as_str())
                .with("s1", self.status[1].as_str())
                .with("s2", self.status[2].as_str())
                .with(field::RADAR, self.radar)
                .with(field::RANGE_STEP, i64::from(self.range));
            if let Some(train) = self.train {
                for (i, pair) in train.iter().enumerate() {
                    event = event
                        .with(&format!("h{i}"), pair[0])
                        .with(&format!("e{i}"), pair[1]);
                }
            }
            if let Some(lock) = self.gun_lock {
                event = event
                    .with("lock_x", lock[0])
                    .with("lock_y", lock[1])
                    .with("lock_z", lock[2]);
            }
        }
        if let Some(detail) = &self.detail {
            event = detail.write(event);
        }
        event
    }

    pub fn parse(event: &Event) -> Option<Self> {
        if event.kind != kind::AIRCRAFT_SIGHT {
            return None;
        }
        let point = match (
            event.num(field::X_FT),
            event.num(field::Y_FT),
            event.num(field::Z_FT),
        ) {
            (Some(x), Some(y), Some(z)) => Some([x, y, z]),
            _ => None,
        };
        let gunship = event.flag(field::GUNSHIP).unwrap_or(false);
        let train = gunship
            .then(|| {
                let heading = |i| event.num(&format!("h{i}"));
                if (0..3).all(|i| heading(i).is_some()) {
                    Some(std::array::from_fn(|i| {
                        [
                            heading(i).unwrap_or(0.),
                            event.num(&format!("e{i}")).unwrap_or(0.),
                        ]
                    }))
                } else {
                    None
                }
            })
            .flatten();
        let gun_lock = match (
            event.num("lock_x"),
            event.num("lock_y"),
            event.num("lock_z"),
        ) {
            (Some(x), Some(y), Some(z)) => Some([x, y, z]),
            _ => None,
        };
        Some(Self {
            gunship,
            az: event.num(field::AZ).unwrap_or(0.),
            el: event.num(field::EL).unwrap_or(0.),
            zoom: u8_of(event, field::ZOOM).max(1),
            mode: event.string(field::MODE).unwrap_or("free").to_owned(),
            point,
            tracked: event.object,
            rounds: [
                u16_of(event, "r0"),
                u16_of(event, "r1"),
                u16_of(event, "r2"),
            ],
            link: u8_of(event, field::LINK),
            lead: event
                .get(field::LEAD)
                .and_then(Value::as_i64)
                .and_then(|n| u8::try_from(n).ok())
                .unwrap_or(255),
            status: [
                event.string("s0").unwrap_or("").to_owned(),
                event.string("s1").unwrap_or("").to_owned(),
                event.string("s2").unwrap_or("").to_owned(),
            ],
            train,
            radar: event.flag(field::RADAR).unwrap_or(false),
            range: u8_of(event, field::RANGE_STEP),
            gun_lock,
            detail: SightDetail::parse(event),
        })
    }
}

/// The last HUD and sight written for each aircraft, in tick order.
#[derive(Clone, Debug, Default)]
pub struct Track {
    hud: BTreeMap<u32, Vec<(u64, HudSample)>>,
    sight: BTreeMap<u32, Vec<(u64, SightSample)>>,
}

impl Track {
    pub fn from_events(events: &[TimedEvent]) -> Self {
        let mut track = Self::default();
        for timed in events {
            let Some(id) = timed.event.subject else {
                continue;
            };
            if let Some(sample) = HudSample::parse(&timed.event) {
                track.hud.entry(id).or_default().push((timed.tick, sample));
            } else if let Some(sample) = SightSample::parse(&timed.event) {
                track
                    .sight
                    .entry(id)
                    .or_default()
                    .push((timed.tick, sample));
            }
        }
        track
    }

    /// The HUD in force at `tick`: the last sample at or before it.
    pub fn hud(&self, aircraft: u32, tick: u64) -> Option<&HudSample> {
        at(self.hud.get(&aircraft)?, tick)
    }

    /// The sight in force at `tick`.
    pub fn sight(&self, aircraft: u32, tick: u64) -> Option<&SightSample> {
        at(self.sight.get(&aircraft)?, tick)
    }

    /// Camera-only interpolation. Symbols and discrete changes remain on their sample.
    /// Old sparse recordings hold their last look, as before.
    pub fn look(&self, aircraft: u32, tick: u64, alpha: f64) -> Option<[f64; 2]> {
        let list = self.sight.get(&aircraft)?;
        let index = list.partition_point(|(at, _)| *at <= tick).checked_sub(1)?;
        let (at, a) = &list[index];
        let look = [a.az, a.el];
        let Some((next, b)) = list.get(index + 1) else {
            return Some(look);
        };
        if a.detail.is_none()
            || b.detail.is_none()
            || next - at > DISPLAY_TICKS
            || a.mode != b.mode
            || a.tracked != b.tracked
            || a.zoom != b.zoom
        {
            return Some(look);
        }
        let mix = ((tick - at) as f64 + alpha.clamp(0., 1.)) / (next - at) as f64;
        let dh = (b.az - a.az + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        // Large discontinuities are a reset, not a slew.
        if dh.abs().max((b.el - a.el).abs()) > 0.5 {
            return Some(look);
        }
        Some([a.az + dh * mix, a.el + (b.el - a.el) * mix])
    }

    pub fn is_empty(&self) -> bool {
        self.hud.is_empty() && self.sight.is_empty()
    }
}

fn at<T>(list: &[(u64, T)], tick: u64) -> Option<&T> {
    let index = list.partition_point(|(at, _)| *at <= tick);
    list.get(index.checked_sub(1)?).map(|(_, sample)| sample)
}

fn stepped(a: f64, b: f64, step: f64) -> bool {
    (a - b).abs() >= step
}

fn point_stepped(a: Option<[f64; 3]>, b: Option<[f64; 3]>) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(a), Some(b)) => a.iter().zip(b).any(|(p, q)| (p - q).abs() >= POINT_STEP_FT),
        _ => true,
    }
}

fn u8_of(event: &Event, name: &str) -> u8 {
    event
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|n| u8::try_from(n.max(0)).ok())
        .unwrap_or(0)
}

fn u16_of(event: &Event, name: &str) -> u16 {
    event
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|n| u16::try_from(n.max(0)).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Footer, Frame, Header, Recording, World, Writer};

    fn sample() -> HudSample {
        HudSample {
            nav: false,
            armed: true,
            station: 2,
            weapon: "AIM9M.WPN".into(),
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
            min_ft: 500.,
            max_ft: 36_000.,
            band_min: 2_000.,
            band_max: 18_000.,
            radar: true,
            mount: [0., -2., 4.],
        }
    }

    fn sight() -> SightSample {
        SightSample {
            gunship: true,
            az: -1.25,
            el: -0.5,
            zoom: 3,
            mode: "track".into(),
            point: Some([1_000., 20., 2_000.]),
            tracked: Some(9),
            rounds: [250, 80, 20],
            link: 0b101,
            lead: 0,
            status: ["READY".into(), "CANNOT BEAR".into(), "READY".into()],
            train: Some([[-1.2, -0.4], [-1.1, -0.3], [-1.0, -0.2]]),
            radar: true,
            range: 2,
            gun_lock: Some([1_010., 18., 2_004.]),
            detail: None,
        }
    }

    #[test]
    fn samples_round_trip_through_a_file_and_an_old_file_stays_empty() {
        let dir = std::env::temp_dir().join(format!("tore-replay-display-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hud.tore-replay");
        let _ = std::fs::remove_file(&path);
        let header = Header {
            format_version: crate::FORMAT_VERSION,
            world: World {
                theater: "UKR".into(),
                ..World::default()
            },
            ..Header::default()
        };
        let mut writer = Writer::create(&path, &header).unwrap();
        let hud = sample();
        let mut guns = sight();
        guns.detail = Some(SightDetail {
            impacts: [Some(ImpactSample {
                point: [1000.12345, 5.5, 2000.98765],
                range_ft: 4000.25,
                spent: false,
            }); 3],
            gimbal_limit: true,
            mark: Some([1000., 5., 2000.]),
            radar: vec![RadarContact {
                position: [1234.5, 6., 8000.25],
                symbol: 1,
                selected: true,
            }],
        });
        writer
            .push(&Frame {
                tick: 10,
                events: vec![hud.event(4), guns.event(4)],
                ..Frame::default()
            })
            .unwrap();
        writer
            .push(&Frame {
                tick: 20,
                events: vec![Event::new(kind::SYSTEM_NOTE).with_text("still flying")],
                ..Frame::default()
            })
            .unwrap();
        let path = writer.finish(&Footer::default()).unwrap();
        let recording = Recording::open(&path).unwrap();
        assert_eq!(recording.header().format_version, 4);
        let track = Track::from_events(recording.events());
        let back = track.hud(4, 20).unwrap();
        assert_eq!(back.weapon, hud.weapon);
        assert_eq!(back.seeker, "LOCK");
        assert_eq!(back.target, Some(7));
        assert!(!back.nav && back.armed && back.radar && back.in_range);
        assert_eq!(back.rounds, 2);
        assert_eq!(back.hit, 40);
        assert!((back.max_ft - 36_000.).abs() < 1.);
        assert!((back.min_ft - 500.).abs() < 1.);
        assert!((back.mount[2] - 4.).abs() < 0.01);
        let seen = track.sight(4, 10).unwrap();
        assert!(seen.gunship);
        assert_eq!(seen.rounds, [250, 80, 20]);
        assert_eq!(seen.lead, 0);
        assert_eq!(seen.tracked, Some(9));
        assert_eq!(seen.status[1], "CANNOT BEAR");
        assert!(seen.train.is_some());
        assert_eq!(seen.detail, guns.detail);
        assert!(guns.event(4).fields.len() <= crate::limits::MAX_FIELDS_PER_EVENT);
        assert!((seen.az - guns.az).abs() < 0.001);
        assert!(track.hud(4, 9).is_none());

        let bare = dir.join("old.tore-replay");
        let _ = std::fs::remove_file(&bare);
        let old_header = Header {
            format_version: 2,
            ..header
        };
        let mut writer = Writer::create(&bare, &old_header).unwrap();
        writer
            .push(&Frame {
                tick: 1,
                ..Frame::default()
            })
            .unwrap();
        let bare = writer.finish(&Footer::default()).unwrap();
        let old = Recording::open(&bare).unwrap();
        assert_eq!(old.header().format_version, 2);
        assert!(Track::from_events(old.events()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn camera_interpolates_shortest_arc_but_never_blends_modes_or_old_sparse_samples() {
        let a = SightSample {
            az: 3.13,
            el: -0.5,
            detail: Some(SightDetail::default()),
            ..sight()
        };
        let b = SightSample {
            az: -3.13,
            el: -0.4,
            ..a.clone()
        };
        let track = Track::from_events(&[
            TimedEvent {
                tick: 10,
                event: a.event(4),
            },
            TimedEvent {
                tick: 15,
                event: b.event(4),
            },
        ]);
        let middle = track.look(4, 12, 0.5).unwrap();
        assert!((middle[0] - std::f64::consts::PI).abs() < 1e-9);
        assert!((middle[1] + 0.45).abs() < 1e-9);
        assert_eq!(track.sight(4, 14).unwrap().el, -0.5);
        assert_eq!(track.look(4, 15, 0.).unwrap(), [b.az, b.el]);
        for next in [
            SightSample {
                mode: "free".into(),
                ..b.clone()
            },
            SightSample {
                detail: None,
                ..b.clone()
            },
        ] {
            let track = Track::from_events(&[
                TimedEvent {
                    tick: 10,
                    event: a.event(4),
                },
                TimedEvent {
                    tick: 15,
                    event: next.event(4),
                },
            ]);
            assert_eq!(track.look(4, 14, 0.5).unwrap(), [a.az, a.el]);
        }
        let gap = Track::from_events(&[
            TimedEvent {
                tick: 10,
                event: a.event(4),
            },
            TimedEvent {
                tick: 30,
                event: b.event(4),
            },
        ]);
        assert_eq!(gap.look(4, 14, 0.5).unwrap(), [a.az, a.el]);
    }

    #[test]
    fn a_small_range_change_does_not_count_and_a_lock_does() {
        let mut hud = sample();
        let same = HudSample {
            hit: 42,
            max_ft: 36_050.,
            ..hud.clone()
        };
        assert!(!same.differs(&hud));
        hud.seeker = "ACTIVE SEARCH".into();
        assert!(hud.differs(&same));
        let mut guns = sight();
        let slew = SightSample {
            az: guns.az + 0.001,
            ..guns.clone()
        };
        assert!(!slew.differs(&guns));
        assert!(!slew.stepped(&guns));
        guns.rounds[0] = 249;
        assert!(guns.stepped(&slew));
    }

    #[test]
    fn a_moving_track_point_is_a_slew_and_a_moved_pin_is_not() {
        let track = sight();
        let moved = SightSample {
            point: track.point.map(|p| [p[0] + 60., p[1], p[2]]),
            ..track.clone()
        };
        assert!(moved.differs(&track));
        assert!(!moved.stepped(&track));
        let pin = SightSample {
            mode: "point".into(),
            ..track.clone()
        };
        let repinned = SightSample {
            mode: "point".into(),
            ..moved
        };
        assert!(repinned.stepped(&pin));
        let free = SightSample {
            gunship: false,
            mode: "free".into(),
            point: None,
            tracked: None,
            gun_lock: None,
            detail: None,
            ..SightSample::default()
        };
        assert!(!free.notable());
    }
}
