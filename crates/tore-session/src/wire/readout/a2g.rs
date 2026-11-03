//! The air-to-ground round's share of the cockpit readout (protocol 23,
//! slice NA; docs/formats/net-protocol.md, "Air-to-ground readout"): the
//! targeting sight and its sensor, the laser and the spots, the bomb or
//! rocket sight, the guided weapon sight, the HARM boresight cue and the
//! guided weapons in flight, each a scalar group, plus the ground pages'
//! flags (two values at the end of the Sensors group) and their list.
//!
//! Every group here holds at least one value once it has arrived: its first
//! value says whether the cockpit has the thing at all (0 none, 1 present),
//! so a group that went away (a bomb no longer armed, a laser that left with
//! its plane) reads as none rather than as "waiting for room", which would
//! show the last values that arrived (`ReadoutReceiver::presented`).
//!
//! *Agent decisions:* world points the HUD draws close to the plane (the
//! sight's aim and pin, the bomb sight's impact, release and target) are in
//! 1/8 ft like the AC-130's gunsight; points drawn far off or only as text
//! (laser spots, the guided weapon's aim, the HARM cue's emitters, the
//! weapons in flight) are in whole feet like the scope's; ranges are whole
//! feet; times 1/64 s; look and gimbal angles 2^-20 of a turn; the release
//! velocity 1/64 ft/s; the level release's steering 1/4096 rad.

use super::{COARSE_FT, FINE_FPS, LOOK, SECONDS, SIGHT_FT, SMALL_ANGLE, flag, q, v};
use crate::wire::{WireError, WireResult};
use tore_sim::attitude::Vector;
use tore_sim::combat::ballistics::ReleaseState;
use tore_sim::combat::delivery::{Ccip, DeliveryMode, LevelRelease, Reach, Warning};
use tore_sim::combat::guided::{Advisory, GuidedLevel, GuidedSight};
use tore_sim::combat::gunship::{Notice, SightNotice};
use tore_sim::combat::laser::{LaserSpot, LaserState, Mask};
use tore_sim::combat::live::Side;
use tore_sim::combat::missiles::Guidance;
use tore_sim::combat::sight::{Gimbal, SightMode, SightSensor, ZOOM_STEPS};
use tore_world::readout::{
    BombSight, HarmCue, HarmEmitter, LaserReadout, MAX_HARM_CUE, MAX_WEAPON_TTI, SightReadout,
    SpotReadout, WeaponTti,
};

fn bad(what: &'static str) -> WireError {
    WireError::Invalid(what)
}

fn point(p: Vector, step: f64) -> [i64; 3] {
    p.map(|x| q(x, step))
}

fn point_of(values: &[i64], at: usize, step: f64) -> Vector {
    std::array::from_fn(|i| v(values[at + i], step))
}

fn u32_of(x: i64, what: &'static str) -> WireResult<u32> {
    u32::try_from(x).map_err(|_| bad(what))
}

/// Reads a group's values after its presence flag: `None` when the group
/// has not arrived (empty) or says none, else the values after the flag,
/// which must be `count` of them (or a multiple past `count` when `each` is
/// not zero).
fn present<'a>(
    values: &'a [i64],
    count: usize,
    each: usize,
    what: &'static str,
) -> WireResult<Option<&'a [i64]>> {
    match values.first() {
        None | Some(0) if values.len() <= 1 => Ok(None),
        Some(1) => {
            let rest = &values[1..];
            let fits = if each == 0 {
                rest.len() == count
            } else {
                rest.len() >= count && (rest.len() - count).is_multiple_of(each)
            };
            if fits { Ok(Some(rest)) } else { Err(bad(what)) }
        }
        _ => Err(bad(what)),
    }
}

fn notice_code(notice: Option<SightNotice>, tick: u64) -> [i64; 2] {
    match notice {
        None => [0, 0],
        Some(SightNotice { notice, tick: at }) => [
            match notice {
                Notice::NoGroundPoint => 1,
                Notice::DropToSlew => 2,
                Notice::GimbalLimit => 3,
            },
            tick as i64 - at as i64,
        ],
    }
}

fn notice_of(code: i64, back: i64, tick: u64) -> WireResult<Option<SightNotice>> {
    let notice = match code {
        0 => return Ok(None),
        1 => Notice::NoGroundPoint,
        2 => Notice::DropToSlew,
        3 => Notice::GimbalLimit,
        _ => return Err(bad("sight notice")),
    };
    let tick = u64::try_from(tick as i64 - back).map_err(|_| bad("sight notice tick"))?;
    Ok(Some(SightNotice { notice, tick }))
}

/// Values of the Sight group after its flag.
const SIGHT_VALUES: usize = 2 + 3 + 2 + 1 + 4 + 2 + 1;

/// The Sight group (scalar 15): the flag, then the mode (0 free, 1 pinned,
/// 2 tracked) and its tracked id, the pin as an offset from the aim point,
/// the look angles, the returning flag, the aim point (a flag and the
/// point), the notice and its tick before the readout's, and the zoom step.
pub(super) fn sight_values(sight: Option<&SightReadout>, tick: u64) -> Vec<i64> {
    let Some(s) = sight else {
        return vec![0];
    };
    let origin = s.aim.unwrap_or([0.; 3]);
    let (mode, id, pin) = match s.mode {
        SightMode::Free => (0, 0, [0; 3]),
        SightMode::Pinned(p) => (1, 0, std::array::from_fn(|i| q(p[i] - origin[i], SIGHT_FT))),
        SightMode::Tracked(id) => (2, i64::from(id), [0; 3]),
    };
    let mut out = vec![1, mode, id];
    out.extend(pin);
    out.extend(s.look.map(|a| q(a, LOOK)));
    out.push(flag(s.returning));
    out.push(flag(s.aim.is_some()));
    out.extend(point(origin, SIGHT_FT));
    out.extend(notice_code(s.notice, tick));
    out.push(i64::from(s.zoom));
    out
}

/// Values of the Sight sensor group after its flag.
const SENSOR_VALUES: usize = 2 + 3 + 3 + 2;

/// The Sight sensor group (scalar 16), which changes only with the plane or
/// its pod: the flag, then the laser flag, the reach, the gimbal (0 the
/// hemisphere, 1 a cone, with its search and track half-angles), the eye
/// and the default look.
pub(super) fn sensor_values(sight: Option<&SightReadout>) -> Vec<i64> {
    let Some(s) = sight else {
        return vec![0];
    };
    let sensor = &s.sensor;
    let (kind, search, track) = match sensor.gimbal {
        Gimbal::Hemisphere => (0, 0, 0),
        Gimbal::Cone { search, track } => (1, q(search, LOOK), q(track, LOOK)),
    };
    let mut out = vec![1, flag(sensor.laser), q(sensor.range_ft, COARSE_FT)];
    out.extend([kind, search, track]);
    out.extend(point(sensor.eye, SIGHT_FT));
    out.extend(sensor.default_look.map(|a| q(a, LOOK)));
    out
}

/// The targeting sight from the Sight and Sight sensor groups; none until
/// both have arrived and say there is one.
pub(super) fn sight_of(
    sight: &[i64],
    sensor: &[i64],
    tick: u64,
) -> WireResult<Option<SightReadout>> {
    let (Some(s), Some(e)) = (
        present(sight, SIGHT_VALUES, 0, "sight values")?,
        present(sensor, SENSOR_VALUES, 0, "sight sensor values")?,
    ) else {
        return Ok(None);
    };
    let origin = point_of(s, 9, SIGHT_FT);
    let mode = match s[0] {
        0 => SightMode::Free,
        1 => SightMode::Pinned(std::array::from_fn(|i| origin[i] + v(s[2 + i], SIGHT_FT))),
        2 => SightMode::Tracked(u32_of(s[1], "sight target")?),
        _ => return Err(bad("sight mode")),
    };
    let zoom = u8::try_from(s[14])
        .ok()
        .filter(|z| (1..=ZOOM_STEPS).contains(z))
        .ok_or(bad("sight zoom"))?;
    let gimbal = match e[2] {
        0 => Gimbal::Hemisphere,
        1 => Gimbal::Cone {
            search: v(e[3], LOOK),
            track: v(e[4], LOOK),
        },
        _ => return Err(bad("sight gimbal")),
    };
    Ok(Some(SightReadout {
        mode,
        look: [v(s[5], LOOK), v(s[6], LOOK)],
        returning: s[7] != 0,
        aim: (s[8] != 0).then_some(origin),
        notice: notice_of(s[12], s[13], tick)?,
        zoom,
        sensor: SightSensor {
            laser: e[0] != 0,
            range_ft: v(e[1], COARSE_FT),
            gimbal,
            eye: point_of(e, 5, SIGHT_FT),
            default_look: [v(e[8], LOOK), v(e[9], LOOK)],
        },
    }))
}

/// Values of one spot: present, owner, side, position, target (flag and
/// id), own, range.
const SPOT_VALUES: usize = 1 + 1 + 1 + 3 + 2 + 1 + 1;

fn spot_values(spot: Option<&SpotReadout>) -> [i64; SPOT_VALUES] {
    let Some(s) = spot else {
        return [0; SPOT_VALUES];
    };
    let [x, y, z] = point(s.spot.position, COARSE_FT);
    [
        1,
        i64::from(s.spot.owner),
        i64::from(s.spot.side.0),
        x,
        y,
        z,
        flag(s.spot.target.is_some()),
        s.spot.target.map_or(0, i64::from),
        flag(s.own),
        q(s.range_ft, COARSE_FT),
    ]
}

fn spot_of(x: &[i64]) -> WireResult<Option<SpotReadout>> {
    if x[0] == 0 {
        return Ok(None);
    }
    Ok(Some(SpotReadout {
        spot: LaserSpot {
            owner: u32_of(x[1], "spot owner")?,
            side: Side(u32_of(x[2], "spot side")?),
            position: point_of(x, 3, COARSE_FT),
            target: (x[6] != 0)
                .then(|| u32_of(x[7], "spot target"))
                .transpose()?,
        },
        own: x[8] != 0,
        range_ft: v(x[9], COARSE_FT),
    }))
}

/// Values of the Laser group after its flag.
const LASER_VALUES: usize = 6 + 2 * SPOT_VALUES;

/// The Laser group (scalar 17): the flag, then the designator flag, the
/// state (0 off, 1 ready, 2 lasing, 3 masked), the mask (0 none, 1 range,
/// 2 gimbal, 3 terrain), the hand setting (0 automatic, 1 on, 2 off), the
/// range and the reach, then the spot and the buddy's spot (owner, side,
/// position, target, own and range each, all zero for none).
pub(super) fn laser_values(laser: Option<&LaserReadout>) -> Vec<i64> {
    let Some(l) = laser else {
        return vec![0];
    };
    let mut out = vec![
        1,
        flag(l.designator),
        match l.state {
            LaserState::Off => 0,
            LaserState::Ready => 1,
            LaserState::Lasing => 2,
            LaserState::Masked => 3,
        },
        match l.mask {
            None => 0,
            Some(Mask::Range) => 1,
            Some(Mask::Gimbal) => 2,
            Some(Mask::Terrain) => 3,
        },
        match l.manual {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        },
        q(l.range_ft, COARSE_FT),
        q(l.limit_ft, COARSE_FT),
    ];
    out.extend(spot_values(l.spot.as_ref()));
    out.extend(spot_values(l.buddy.as_ref()));
    out
}

pub(super) fn laser_of(values: &[i64]) -> WireResult<Option<LaserReadout>> {
    let Some(x) = present(values, LASER_VALUES, 0, "laser values")? else {
        return Ok(None);
    };
    Ok(Some(LaserReadout {
        designator: x[0] != 0,
        state: match x[1] {
            0 => LaserState::Off,
            1 => LaserState::Ready,
            2 => LaserState::Lasing,
            3 => LaserState::Masked,
            _ => return Err(bad("laser state")),
        },
        mask: match x[2] {
            0 => None,
            1 => Some(Mask::Range),
            2 => Some(Mask::Gimbal),
            3 => Some(Mask::Terrain),
            _ => return Err(bad("laser mask")),
        },
        manual: match x[3] {
            0 => None,
            1 => Some(true),
            2 => Some(false),
            _ => return Err(bad("laser setting")),
        },
        range_ft: v(x[4], COARSE_FT),
        limit_ft: v(x[5], COARSE_FT),
        spot: spot_of(&x[6..6 + SPOT_VALUES])?,
        buddy: spot_of(&x[6 + SPOT_VALUES..])?,
    }))
}

const WARNINGS: [Warning; 3] = [Warning::Low, Warning::Frag, Warning::Overfly];

/// Values of the CCIP part, after its flag.
const CCIP_VALUES: usize = 3 + 3 + 3 + 5 + 5;
/// Values of the level release part, after its flag.
const LEVEL_VALUES: usize = 3 + 7;

/// Values of the Bomb sight group after its flag.
const BOMB_VALUES: usize = 2 + 1 + CCIP_VALUES + 1 + LEVEL_VALUES + 1 + 2;

/// The Bomb sight group (scalar 18): the flag, then the mode (0 CCIP, 1
/// LVL), the rocket flag; the CCIP (a flag, the mean impact, the release
/// point as an offset from it, the release velocity, the fall's seconds,
/// the arc's range, the slant range, the record's zone-1 maximum and the
/// arc's steps); the level release (a flag, the target, D, C, the seconds
/// to release, the steering, the slant range, the release radius and the
/// ground speed); the warnings as a mask (1 LOW, 2 FRAG, 4 OVERFLY); and
/// the seconds the last bomb has to fall (a flag and the seconds). A part
/// that is absent sends zeros.
pub(super) fn bomb_values(sight: Option<&BombSight>) -> Vec<i64> {
    let Some(b) = sight else {
        return vec![0];
    };
    let mut out = vec![
        1,
        match b.mode {
            DeliveryMode::Ccip => 0,
            DeliveryMode::Level => 1,
        },
        flag(b.rocket),
        flag(b.ccip.is_some()),
    ];
    match &b.ccip {
        None => out.extend([0; CCIP_VALUES]),
        Some(c) => {
            out.extend(point(c.impact, SIGHT_FT));
            out.extend(std::array::from_fn::<i64, 3, _>(|i| {
                q(c.release.position[i] - c.impact[i], SIGHT_FT)
            }));
            out.extend(c.release.velocity.map(|x| q(x, FINE_FPS)));
            out.extend([
                q(c.seconds, SECONDS),
                q(c.range_ft, COARSE_FT),
                q(c.slant_ft, COARSE_FT),
                q(c.maximum_ft, COARSE_FT),
                i64::from(c.arc),
                q(c.release.body.mass, 1. / 16_777_216.),
                q(c.release.body.area, 1. / 16_777_216.),
                q(c.release.body.retarder, 1. / 16_777_216.),
                i64::from(c.release.body.steering),
                q(c.release.body.guidance_seconds, SECONDS),
            ]);
        }
    }
    out.push(flag(b.level.is_some()));
    match &b.level {
        None => out.extend([0; LEVEL_VALUES]),
        Some(l) => {
            out.extend(point(l.target, SIGHT_FT));
            out.extend([
                q(l.along_ft, COARSE_FT),
                q(l.cross_ft, COARSE_FT),
                q(l.release_seconds, SECONDS),
                q(l.steering_rad, SMALL_ANGLE),
                q(l.slant_ft, COARSE_FT),
                q(l.radius_ft, COARSE_FT),
                q(l.ground_speed_fps, FINE_FPS),
            ]);
        }
    }
    out.push(
        WARNINGS
            .iter()
            .enumerate()
            .filter(|(_, w)| b.warnings.contains(w))
            .map(|(i, _)| 1 << i)
            .sum(),
    );
    out.push(flag(b.falling.is_some()));
    out.push(b.falling.map_or(0, |t| q(t, SECONDS)));
    out
}

pub(super) fn bomb_of(values: &[i64]) -> WireResult<Option<BombSight>> {
    let Some(x) = present(values, BOMB_VALUES, 0, "bomb sight values")? else {
        return Ok(None);
    };
    let mode = match x[0] {
        0 => DeliveryMode::Ccip,
        1 => DeliveryMode::Level,
        _ => return Err(bad("delivery mode")),
    };
    let c = &x[3..3 + CCIP_VALUES];
    let ccip = (x[2] != 0)
        .then(|| -> WireResult<Ccip> {
            let impact = point_of(c, 0, SIGHT_FT);
            if !(1..=16_777_216_000_000).contains(&c[14])
                || !(0..=16_777_216_000).contains(&c[15])
                || !(0..=16_777_216_000).contains(&c[16])
                || !(0..=2).contains(&c[17])
                || !(0..=19_200).contains(&c[18])
            {
                return Err(bad("bomb body"));
            }

            Ok(Ccip {
                release: ReleaseState {
                    body: tore_sim::combat::bomb_flight::Body {
                        mass: v(c[14], 1. / 16_777_216.),
                        area: v(c[15], 1. / 16_777_216.),
                        retarder: v(c[16], 1. / 16_777_216.),
                        steering: u8::try_from(c[17]).map_err(|_| bad("bomb steering"))?,
                        guidance_seconds: v(c[18], SECONDS),
                    },
                    position: std::array::from_fn(|i| impact[i] + v(c[3 + i], SIGHT_FT)),
                    velocity: point_of(c, 6, FINE_FPS),
                },
                impact,
                seconds: v(c[9], SECONDS),
                range_ft: v(c[10], COARSE_FT),
                slant_ft: v(c[11], COARSE_FT),
                maximum_ft: v(c[12], COARSE_FT),
                arc: u8::try_from(c[13])
                    .ok()
                    .filter(|a| *a <= tore_sim::combat::delivery::ARC_STEPS)
                    .ok_or(bad("pipper arc"))?,
            })
        })
        .transpose()?;
    let at = 3 + CCIP_VALUES;
    let l = &x[at + 1..at + 1 + LEVEL_VALUES];
    let level = (x[at] != 0).then(|| LevelRelease {
        target: point_of(l, 0, SIGHT_FT),
        along_ft: v(l[3], COARSE_FT),
        cross_ft: v(l[4], COARSE_FT),
        release_seconds: v(l[5], SECONDS),
        steering_rad: v(l[6], SMALL_ANGLE),
        slant_ft: v(l[7], COARSE_FT),
        radius_ft: v(l[8], COARSE_FT),
        ground_speed_fps: v(l[9], FINE_FPS),
    });
    let at = at + 1 + LEVEL_VALUES;
    let mask = x[at];
    if !(0..8).contains(&mask) {
        return Err(bad("bomb sight warnings"));
    }
    Ok(Some(BombSight {
        mode,
        rocket: x[1] != 0,
        ccip,
        level,
        warnings: WARNINGS
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, w)| *w)
            .collect(),
        falling: (x[at + 1] != 0).then(|| v(x[at + 2], SECONDS)),
    }))
}

fn guidance_code(g: Guidance) -> i64 {
    match g {
        Guidance::Supported => 0,
        Guidance::Active => 1,
        Guidance::Infrared => 2,
        Guidance::Emitter => 3,
        Guidance::Laser => 4,
        Guidance::Coordinate => 5,
        Guidance::Command => 6,
    }
}
const GUIDANCES: [Guidance; 7] = [
    Guidance::Supported,
    Guidance::Active,
    Guidance::Infrared,
    Guidance::Emitter,
    Guidance::Laser,
    Guidance::Coordinate,
    Guidance::Command,
];

fn guidance_of(code: i64) -> WireResult<Guidance> {
    usize::try_from(code)
        .ok()
        .and_then(|i| GUIDANCES.get(i).copied())
        .ok_or(bad("guidance"))
}

/// Values of the Guided group after its flag.
const GUIDED_VALUES: usize = 1 + 4 + 1 + 2 + 2 + 3 + 1 + 1 + 3;

/// The Guided group (scalar 19): the flag, then the guidance, the aim (a
/// flag and the point), the buddy flag, the ground and slant ranges (a flag
/// and the range each), the reach (a flag, the minimum and the maximum), the
/// IN RNG flag and the advisory (0 none, 1 NO LASER, 2 MASK, 3 OUT OF REACH,
/// 4 NO TARGET), then the level T (a flag, the steering and the ground speed;
/// zeros when absent).
pub(super) fn guided_values(guided: Option<&GuidedSight>) -> Vec<i64> {
    let Some(g) = guided else {
        return vec![0];
    };
    let mut out = vec![1, guidance_code(g.kind), flag(g.aim.is_some())];
    out.extend(point(g.aim.unwrap_or([0.; 3]), COARSE_FT));
    out.push(flag(g.buddy));
    for range in [g.ground_range_ft, g.slant_range_ft] {
        out.extend([flag(range.is_some()), range.map_or(0, |r| q(r, COARSE_FT))]);
    }
    out.extend([
        flag(g.reach.is_some()),
        g.reach.map_or(0, |r| q(r.minimum_ft, COARSE_FT)),
        g.reach.map_or(0, |r| q(r.maximum_ft, COARSE_FT)),
        flag(g.in_reach),
        match g.advisory {
            None => 0,
            Some(Advisory::NoLaser) => 1,
            Some(Advisory::Masked) => 2,
            Some(Advisory::OutOfReach) => 3,
            Some(Advisory::NoTarget) => 4,
        },
        flag(g.level.is_some()),
        g.level.map_or(0, |l| q(l.steering_rad, SMALL_ANGLE)),
        g.level.map_or(0, |l| q(l.ground_speed_fps, FINE_FPS)),
    ]);
    out
}

pub(super) fn guided_of(values: &[i64]) -> WireResult<Option<GuidedSight>> {
    let Some(x) = present(values, GUIDED_VALUES, 0, "guided values")? else {
        return Ok(None);
    };
    let range = |at: usize| (x[at] != 0).then(|| v(x[at + 1], COARSE_FT));
    Ok(Some(GuidedSight {
        energy_intercept: None,
        kind: guidance_of(x[0])?,
        aim: (x[1] != 0).then(|| point_of(x, 2, COARSE_FT)),
        buddy: x[5] != 0,
        ground_range_ft: range(6),
        slant_range_ft: range(8),
        reach: (x[10] != 0).then(|| Reach {
            minimum_ft: v(x[11], COARSE_FT),
            maximum_ft: v(x[12], COARSE_FT),
        }),
        in_reach: x[13] != 0,
        advisory: match x[14] {
            0 => None,
            1 => Some(Advisory::NoLaser),
            2 => Some(Advisory::Masked),
            3 => Some(Advisory::OutOfReach),
            4 => Some(Advisory::NoTarget),
            _ => return Err(bad("guided advisory")),
        },
        level: (x[15] != 0).then(|| GuidedLevel {
            steering_rad: v(x[16], SMALL_ANGLE),
            ground_speed_fps: v(x[17], FINE_FPS),
        }),
    }))
}

/// Values of one HARM cue emitter: id, position, painting, emitting.
const EMITTER_VALUES: usize = 6;

/// The HARM group (scalar 20): the flag, then the candidate (a flag and its
/// id), the CUED flag, and each emitter in the cue's order (nearest the bore
/// first): its id, position, painting and emitting flags.
pub(super) fn harm_values(cue: Option<&HarmCue>) -> Vec<i64> {
    let Some(c) = cue else {
        return vec![0];
    };
    let mut out = vec![
        1,
        flag(c.candidate.is_some()),
        c.candidate.map_or(0, i64::from),
        flag(c.cued),
    ];
    for e in c.emitters.iter().take(MAX_HARM_CUE) {
        out.push(i64::from(e.id));
        out.extend(point(e.position, COARSE_FT));
        out.extend([flag(e.painting), flag(e.emitting)]);
    }
    out
}

pub(super) fn harm_of(values: &[i64]) -> WireResult<Option<HarmCue>> {
    let Some(x) = present(values, 3, EMITTER_VALUES, "HARM cue values")? else {
        return Ok(None);
    };
    let emitters = x[3..]
        .chunks_exact(EMITTER_VALUES)
        .map(|e| {
            Ok(HarmEmitter {
                id: u32_of(e[0], "HARM emitter")?,
                position: point_of(e, 1, COARSE_FT),
                painting: e[4] != 0,
                emitting: e[5] != 0,
            })
        })
        .collect::<WireResult<Vec<_>>>()?;
    if emitters.len() > MAX_HARM_CUE {
        return Err(WireError::TooMany {
            what: "HARM cue emitters",
            limit: MAX_HARM_CUE,
        });
    }
    Ok(Some(HarmCue {
        emitters,
        candidate: (x[0] != 0)
            .then(|| u32_of(x[1], "HARM candidate"))
            .transpose()?,
        cued: x[2] != 0,
    }))
}

/// Values of one weapon in flight: projectile, station, guidance, guiding,
/// seconds (a flag and the seconds), position.
const TTI_VALUES: usize = 4 + 2 + 3;

/// The Weapons in flight group (scalar 21): the tick the last guided weapon
/// ended (a flag and the ticks before the readout's), then each of the
/// plane's guided weapons in flight, oldest first: its projectile, station,
/// guidance, guiding flag, seconds to impact (a flag and the seconds) and
/// position. Always two values or more.
pub(super) fn tti_values(weapons: &[WeaponTti], impact: Option<u64>, tick: u64) -> Vec<i64> {
    let mut out = vec![
        flag(impact.is_some()),
        impact.map_or(0, |at| tick as i64 - at as i64),
    ];
    for w in weapons.iter().take(MAX_WEAPON_TTI) {
        out.extend([
            i64::from(w.projectile),
            i64::from(w.station),
            guidance_code(w.kind),
            flag(w.guiding),
            flag(w.seconds.is_some()),
            w.seconds.map_or(0, |t| q(t, SECONDS)),
        ]);
        out.extend(point(w.position, COARSE_FT));
    }
    out
}

/// The weapons in flight and the last one's end tick; none of either until
/// the group arrives.
pub(super) fn tti_of(values: &[i64], tick: u64) -> WireResult<(Vec<WeaponTti>, Option<u64>)> {
    if values.is_empty() {
        return Ok((Vec::new(), None));
    }
    if values.len() < 2 || !(values.len() - 2).is_multiple_of(TTI_VALUES) {
        return Err(bad("weapons in flight values"));
    }
    let weapons = values[2..]
        .chunks_exact(TTI_VALUES)
        .map(|w| {
            Ok(WeaponTti {
                projectile: u32_of(w[0], "weapon in flight")?,
                station: u8::try_from(w[1]).map_err(|_| bad("weapon station"))?,
                kind: guidance_of(w[2])?,
                guiding: w[3] != 0,
                seconds: (w[4] != 0).then(|| v(w[5], SECONDS)),
                position: point_of(w, 6, COARSE_FT),
            })
        })
        .collect::<WireResult<Vec<_>>>()?;
    if weapons.len() > MAX_WEAPON_TTI {
        return Err(WireError::TooMany {
            what: "weapons in flight",
            limit: MAX_WEAPON_TTI,
        });
    }
    let impact = (values[0] != 0)
        .then(|| u64::try_from(tick as i64 - values[1]).map_err(|_| bad("impact tick")))
        .transpose()?;
    Ok((weapons, impact))
}
