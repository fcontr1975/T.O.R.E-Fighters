//! A helicopter's threat awareness on a strike (docs/spec/ai-strike.md,
//! "Helicopter threat awareness"; John's rule, 2026-10-12; slice H4).
//!
//! - **Aware** of a defence (one in its ground picture, briefed, seen, or a
//!   radar that painted it): the helicopter does not press an attack through
//!   that defence's **ring** (its reach plus a margin). It looks for an
//!   attack that stays outside every ring it knows: the pop-up when the
//!   target and the run in are clear, else a stand-off shot from a firing
//!   point outside the rings ([`plan_attack`]). With none, it **holds off**:
//!   masked, low, outside the rings ([`hold_centre`]), for up to three
//!   minutes while SEAD may kill the site; then it takes another target that
//!   can be attacked from outside every ring, and with none it goes home.
//! - **Not aware** (a MANPAD, a radar that was off, a radar busy with someone
//!   else): when it is engaged by a site it did not know ([`known_shooter`]
//!   finds none; John, 2026-10-29) it breaks (the helicopter defence), then
//!   leaves the place it was engaged from (an **ambush ring**,
//!   [`ambush_ring`]) and stays out until the threat has been quiet for a
//!   minute; then it may resume. A shot from a site it knew is no ambush:
//!   it leaves that site's ring.
//!
//! A ring's reach is the site's real launch reach at a helicopter (the
//! host's bound on its launch gate), the stand-off's advance the store's
//! time of flight at the helicopter's speed, and a stand-off blocked at its
//! usual ranges looks farther out, to the store's reliable reach on the
//! host's exact-point answers (John, 2026-10-29).
//!
//! Everything is a fixed function of the threats, the positions, the store
//! and the terrain: no dice (John, decision 13). The numbers are fitted
//! (agent) unless marked.
use crate::ai::awareness::ground::{GroundPicture, KnownDefense};
use crate::ai::strike::FlightTimes;
use crate::ai::strike::reach::{self, ShotLaunch, Shots};
use crate::ai::targeting::Side;
use crate::ai::weapon_service::StationId;
use crate::attitude::Vector;
use crate::sensors::FEET_PER_NAUTICAL_MILE;

/// The margin a ring adds to a defence's reach (fitted, agent).
pub const RING_MARGIN_FT: f64 = 2_000.;
/// The longest the helicopter holds off for SEAD before it takes another
/// target (John, 2026-10-12, "up to 3 minutes", the lead's number).
pub const HOLD_WAIT_TICKS: u64 = 180 * 120;
/// A hold-off is called on the radio once it has lasted this long (fitted,
/// agent): a plan that comes clear a moment later is not worth two calls.
pub const HOLD_CALL_TICKS: u64 = 5 * 120;
/// The radius of the circle it flies while holding off (fitted, agent: a
/// helicopter cannot hover on the AI's adapter, so it circles slowly).
pub const HOLD_RADIUS_FT: f64 = 3_000.;
/// Within this of its hold point it counts as there (fitted, agent).
pub const HOLD_REACHED_FT: f64 = 2.0 * HOLD_RADIUS_FT;
/// The stand-off's usual range: this share of the guided store's zone-1
/// range (the ST-F stand-off, which flies straight at the target).
pub const STANDOFF_SHARE: f64 = 0.6;
/// The other firing range tried: this share of the AI's guided fire range
/// (retail's 10,000 ft or half the zone, where the round may leave), so the
/// shot leaves as soon as it unmasks (fitted, agent).
pub const FIRE_RANGE_SHARE: f64 = 0.95;
/// The least the helicopter is taken to carry on toward the target after
/// it unmasks (the advance, [`StandOff::advance_ft`]): the fixed advance
/// before 0.2.2 (fitted, agent). This stretch must stay outside every ring
/// too.
pub const STANDOFF_ADVANCE_FT: f64 = 2_500.;
/// Added to the store's flight in the advance: the unmask's climb before the
/// shot leaves, about 6 s at its strike speed less the flight it saves
/// (fitted, agent). Its longest climb, 10 s, carries it up to about 300 ft
/// farther: inside a ring's margin, never a threat's reach.
pub const ADVANCE_MARGIN_FT: f64 = 1_000.;
/// Off the straight line, the helicopter first flies to a point this much
/// farther out on the firing point's radial, then in along it, so it
/// arrives nose on (fitted, agent).
pub const LEAD_IN_FT: f64 = 3_000.;
/// The firing point's bearing from the target is tried in steps of this,
/// both ways from the helicopter's own bearing, all the way round
/// (fitted, agent).
pub const APPROACH_STEP_DEG: f64 = 15.;
/// The ambush ring: centred this far from where the helicopter was engaged
/// toward the threat's bearing (when it is known), with this radius
/// (fitted, agent: about a MANPAD's or a light gun's reach plus the margin).
pub const AMBUSH_OFFSET_FT: f64 = FEET_PER_NAUTICAL_MILE;
pub const AMBUSH_RADIUS_FT: f64 = 3. * FEET_PER_NAUTICAL_MILE;
/// The ambush ring is forgotten after this long without a warning or
/// gunfire, once the helicopter is outside it (fitted, agent).
pub const AMBUSH_QUIET_TICKS: u64 = 60 * 120;
/// The ring id of an ambush (no site known).
pub const AMBUSH_ID: u32 = 0;
/// The hold point may slide round the site by these angles, degrees, to
/// find terrain between them (fitted, agent).
pub const MASK_TURNS_DEG: [f64; 5] = [0., 30., -30., 60., -60.];
/// The height it holds at over the ground, feet (the masked height).
pub const MASK_AGL_FT: f64 = crate::ai::helicopter_defense::MASK_AGL_FT;

/// A threat's ring on the ground: inside it the threat reaches the
/// helicopter (horizontal distance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    /// The defence's id, or [`AMBUSH_ID`].
    pub id: u32,
    pub centre: Vector,
    pub radius_ft: f64,
}

/// The rings of the known hostile defences: the reach of a missile site
/// that can guide, or of a gun site, plus [`RING_MARGIN_FT`]. A missile
/// site's reach is the farthest it may launch at a helicopter
/// ([`DefenseFacts::rotor_reach_ft`](crate::ai::awareness::ground::DefenseFacts),
/// from its real launch gate; John, 2026-10-29), never less than its launch
/// zone. A defence that reaches nothing (a lone search radar, a launcher
/// whose battery radar is dead) has none.
pub fn rings<'a>(defenses: impl Iterator<Item = &'a KnownDefense>) -> Vec<Ring> {
    defenses
        .filter_map(|d| {
            let missiles = if d.facts.working {
                d.facts.missile_reach_ft.max(d.facts.rotor_reach_ft)
            } else {
                0.
            };
            let reach = missiles.max(d.facts.gun_reach_ft);
            (reach > 0.).then_some(Ring {
                id: d.id,
                centre: d.position,
                radius_ft: reach + RING_MARGIN_FT,
            })
        })
        .collect()
}

/// The class words of a SAM and an AAA site.
const CLASS_AIR_DEFENSE: u16 = 0x1000 | 0x800;

/// The rings a helicopter of `side` knows from its ground picture: every
/// known hostile defence whose missiles can guide, and the guns of the SAM
/// and AAA sites and ships (a tank's gun does not ring the sky; fitted,
/// agent).
pub fn known_rings(picture: &GroundPicture, side: Side) -> Vec<Ring> {
    rings_with(picture, side, true)
}

/// A hidden site that gave itself away by a launch this recently is still the
/// unknown shooter of the threat the helicopter defends against: the round
/// it saw leave is the one coming (fitted, agent, 2026-10-29: a
/// shoulder-fired round flies for 20 s at most; John's rule that a launch
/// unmasks a team for every later shot).
pub const UNMASK_SHOT_TICKS: u64 = 30 * 120;

/// The rings a helicopter of `side` knew before the shot it defends against
/// at `tick` left: [`known_rings`] without the hidden sites whose own launch
/// in the last [`UNMASK_SHOT_TICKS`] is what made them known
/// ([`GroundPicture::unmasked_at`]; T.O.R.E opinionated, John 2026-10-29).
pub fn rings_before_shot(picture: &GroundPicture, side: Side, tick: u64) -> Vec<Ring> {
    let mut rings = known_rings(picture, side);
    rings.retain(|r| {
        picture
            .unmasked_at(r.id)
            .is_none_or(|at| tick.saturating_sub(at) > UNMASK_SHOT_TICKS)
    });
    rings
}

/// The rings a fixed-wing striker of `side` plans a stand-off release round
/// (docs/spec/ai-strike.md, "Stand-off reach"; John, 2026-10-29): as
/// [`known_rings`], with a missile site's reach its launch zone
/// ([`DefenseFacts::missile_reach_ft`](crate::ai::awareness::ground::DefenseFacts)),
/// not its reach at a helicopter.
pub fn jet_rings(picture: &GroundPicture, side: Side) -> Vec<Ring> {
    rings_with(picture, side, false)
}

fn rings_with(picture: &GroundPicture, side: Side, rotor: bool) -> Vec<Ring> {
    let class = |id: u32| {
        picture
            .seen()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.class)
            .or_else(|| {
                picture
                    .briefed()
                    .iter()
                    .find(|b| b.id == id)
                    .map(|b| b.class)
            })
            .unwrap_or(0)
    };
    let defenses: Vec<KnownDefense> = picture
        .threats()
        .hostile_defenses(side)
        .map(|d| {
            let mut d = *d;
            if class(d.id) & CLASS_AIR_DEFENSE == 0 && !d.facts.ship {
                d.facts.gun_reach_ft = 0.;
            }
            if !rotor {
                d.facts.rotor_reach_ft = 0.;
            }
            d
        })
        .collect();
    rings(defenses.iter())
}

/// The ambush ring for a helicopter engaged at `own` by a threat on
/// compass `bearing_deg` (when known).
pub fn ambush_ring(own: Vector, bearing_deg: Option<f64>) -> Ring {
    let centre = match bearing_deg {
        Some(b) => {
            let [x, z] = along(own, b, AMBUSH_OFFSET_FT);
            [x, own[1], z]
        }
        None => own,
    };
    Ring {
        id: AMBUSH_ID,
        centre,
        radius_ft: AMBUSH_RADIUS_FT,
    }
}

/// A threat is a known site's when the site lies within this of the
/// threat's bearing, degrees (fitted, agent: a round flies a near-constant
/// bearing from its launcher to the helicopter).
pub const SHOOTER_BEARING_DEG: f64 = 20.;
/// Nearer a site than this its bearing says nothing: inside its ring it is
/// the shooter whatever the bearing (fitted, agent).
pub const SHOOTER_CLOSE_FT: f64 = 1_000.;

/// The known site a threat engaging the helicopter at `own` came from, by
/// what the helicopter can tell (T.O.R.E opinionated change, John
/// 2026-10-29: only a shot from a site it did not know is an ambush): of the
/// known defences' rings (`rings`, the ambush ring aside), one whose site
/// lies within [`SHOOTER_BEARING_DEG`] of the threat's compass bearing and
/// near enough to have fired (the helicopter inside the ring widened by
/// another [`RING_MARGIN_FT`]), the nearest in bearing; with no bearing, the
/// ring the helicopter is inside. `None`: an unknown shooter.
pub fn known_shooter(rings: &[Ring], own: Vector, bearing_deg: Option<f64>) -> Option<&Ring> {
    let known = rings.iter().filter(|r| r.id != AMBUSH_ID);
    let Some(threat) = bearing_deg else {
        return known
            .into_iter()
            .find(|r| horizontal(r.centre, own) < r.radius_ft);
    };
    known
        .filter_map(|r| {
            let distance = horizontal(r.centre, own);
            if distance >= r.radius_ft + RING_MARGIN_FT {
                return None;
            }
            let off = if distance < SHOOTER_CLOSE_FT {
                0.
            } else {
                let d = (bearing(own, r.centre) - threat).rem_euclid(360.);
                d.min(360. - d)
            };
            (off <= SHOOTER_BEARING_DEG).then_some((off, r))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, r)| r)
}

pub(crate) fn horizontal(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

pub(crate) fn along(from: Vector, bearing_deg: f64, feet: f64) -> [f64; 2] {
    let b = bearing_deg.to_radians();
    [from[0] + b.sin() * feet, from[2] + b.cos() * feet]
}

pub(crate) fn bearing(from: Vector, to: Vector) -> f64 {
    (to[0] - from[0])
        .atan2(to[2] - from[2])
        .to_degrees()
        .rem_euclid(360.)
}

/// The ring `point` lies inside, if any (the first in the list).
pub fn inside(rings: &[Ring], point: Vector) -> Option<&Ring> {
    rings
        .iter()
        .find(|r| horizontal(r.centre, point) < r.radius_ft)
}

/// The horizontal distance from `c` to the segment `a`-`b`.
fn segment_distance(c: Vector, a: Vector, b: Vector) -> f64 {
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let length2 = dx * dx + dz * dz;
    let t = if length2 > 0. {
        (((c[0] - a[0]) * dx + (c[2] - a[2]) * dz) / length2).clamp(0., 1.)
    } else {
        0.
    };
    (a[0] + t * dx - c[0]).hypot(a[2] + t * dz - c[2])
}

/// Whether the straight line from `a` to `b` stays outside every ring. A
/// ring `a` already lies in does not count when the line leads out of it
/// (the helicopter may always leave): it ends outside and never comes
/// nearer the centre than `a`. A line that runs on deeper in before it
/// leaves is not clear, as the helicopter leaves a ring by the shortest
/// way out after a break.
pub fn clear(rings: &[Ring], a: Vector, b: Vector) -> bool {
    rings.iter().all(|r| {
        let from_inside = horizontal(r.centre, a) < r.radius_ft;
        if from_inside {
            // The distance from the centre along a line has one minimum: a
            // line that starts outward keeps going outward.
            let outward =
                (b[0] - a[0]) * (a[0] - r.centre[0]) + (b[2] - a[2]) * (a[2] - r.centre[2]) >= 0.;
            outward && horizontal(r.centre, b) >= r.radius_ft
        } else {
            segment_distance(r.centre, a, b) >= r.radius_ft
        }
    })
}

/// A guided store's stand-off as the plan needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandOff<'a> {
    /// The store's zone-1 range, feet.
    pub reach_ft: f64,
    /// The AI's fire range, feet.
    pub fire_ft: f64,
    /// The store's time of flight to the target (none: unknown).
    pub flight: Option<FlightTimes>,
    /// The helicopter's speed while it guides, ft/s.
    pub guide_fps: f64,
    /// The host's answers on its shots, from the launch the stand-off
    /// unmasks to (none: every firing range is taken to hit, and the plan
    /// looks no farther out than its usual ranges).
    pub shots: Option<StandOffShots<'a>>,
}

/// Where the plan asks whether a stand-off shot hits ([`reach::reliable`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandOffShots<'a> {
    pub shots: &'a Shots,
    pub station: StationId,
    pub launch: ShotLaunch,
}

impl StandOffShots<'_> {
    /// A shot from `range_ft` hits reliably.
    pub fn reliable(&self, range_ft: f64) -> bool {
        reach::reliable(self.shots, self.station, self.launch, range_ft).is_some()
    }
}

impl StandOff<'_> {
    /// How far the helicopter carries on toward the target from a firing
    /// point `range_ft` out (T.O.R.E opinionated change, John 2026-10-29):
    /// the store's time of flight from there at its guiding speed, plus
    /// [`ADVANCE_MARGIN_FT`], never under [`STANDOFF_ADVANCE_FT`]. With no
    /// flight time known, [`STANDOFF_ADVANCE_FT`].
    pub fn advance_ft(&self, range_ft: f64) -> f64 {
        self.flight.map_or(STANDOFF_ADVANCE_FT, |flight| {
            (flight.seconds_to(range_ft) * self.guide_fps + ADVANCE_MARGIN_FT)
                .max(STANDOFF_ADVANCE_FT)
        })
    }
}

/// How the attack is flown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// The pop-up straight at the target.
    PopUp,
    /// A stand-off shot from `approach` (`None`: straight at the target, as
    /// the ST-F stand-off), at `range_ft` from the target.
    StandOff {
        approach: Option<Vector>,
        range_ft: f64,
    },
    /// A stand-off shot at a moving target from a point ahead of its path
    /// ([`crate::ai::helicopter_ahead`]; John, 2026-10-29).
    Ahead(crate::ai::helicopter_ahead::Ahead),
}

/// An attack on `target` from `own` that stays outside every ring, if
/// there is one. `standoff` is the guided store's (none for rockets);
/// `prefer_standoff` is the pass the threat level asks for (the stand-off
/// under threat Missiles). The pop-up needs the target and the line to it
/// clear; a stand-off needs its firing point, the line to it and the
/// advance beyond it ([`StandOff::advance_ft`]) clear. The firing point's
/// bearing from the target is tried from the helicopter's own outward, in
/// [`APPROACH_STEP_DEG`] steps (the right first at each step), each at the
/// usual range ([`STANDOFF_SHARE`] of the zone) and then just inside the
/// fire range ([`FIRE_RANGE_SHARE`]).
///
/// With none of those clear, it looks farther out, to the store's reliable
/// reach (T.O.R.E opinionated change, John 2026-10-29: "If an attack
/// aircraft can hit outside of range safely of known threats AI should do
/// that"): the grid ranges past the usual ones, nearest first, each at every
/// bearing as above; the first clear firing point whose shot the host says
/// hits reliably ([`StandOffShots::reliable`]) is the plan. Under rings, a
/// usual range whose shot would not hit is skipped too. The store flies
/// faster than the helicopter, so the advance's end moves out as the range
/// grows. Only with no clear range within reach does it hold off.
pub fn plan_attack(
    own: Vector,
    target: Vector,
    standoff: Option<StandOff<'_>>,
    prefer_standoff: bool,
    rings: &[Ring],
) -> Option<Plan> {
    let popup = inside(rings, target).is_none() && clear(rings, own, target);
    if popup && (!prefer_standoff || standoff.is_none()) {
        return Some(Plan::PopUp);
    }
    if let Some(standoff) = standoff.filter(|st| st.reach_ft > 0.) {
        let usual = [
            STANDOFF_SHARE * standoff.reach_ft,
            FIRE_RANGE_SHARE * standoff.fire_ft,
        ];
        let base = bearing(target, own);
        // The firing point at `range` on the radial `theta` from the target
        // (`k` steps off the helicopter's own), when it, the way in and the
        // advance stand clear of every ring.
        let fits = |k: i32, theta: f64, range: f64| -> Option<Vector> {
            let [x, z] = along(target, theta, range);
            let point = [x, own[1], z];
            let [ex, ez] = along(target, theta, (range - standoff.advance_ft(range)).max(0.));
            let end = [ex, own[1], ez];
            let [lx, lz] = along(target, theta, range + LEAD_IN_FT);
            let lead = [lx, own[1], lz];
            // Straight in it flies the radial already; off it, it joins the
            // radial at the lead-in point first.
            let way_in = if k == 0 {
                clear(rings, own, point)
            } else {
                inside(rings, lead).is_none()
                    && clear(rings, own, lead)
                    && clear(rings, lead, point)
            };
            (inside(rings, point).is_none() && way_in && clear(rings, point, end)).then_some(point)
        };
        // Under rings a usual range whose shot the host says would not hit
        // is not planned; with no rings the run is as before (the release
        // gate waits for its shot).
        let shots = standoff.shots.filter(|_| !rings.is_empty());
        let mut usual_hits: [Option<bool>; 2] = [None; 2];
        for (k, theta) in approach_bearings(base) {
            for (n, range) in usual.into_iter().enumerate() {
                let Some(point) = fits(k, theta, range) else {
                    continue;
                };
                let hits =
                    *usual_hits[n].get_or_insert_with(|| shots.is_none_or(|sh| sh.reliable(range)));
                if hits {
                    // Straight in at the usual share: the ST-F stand-off.
                    let straight = k == 0 && n == 0;
                    return Some(Plan::StandOff {
                        approach: (!straight).then_some(point),
                        range_ft: range,
                    });
                }
            }
        }
        if let Some(shots) = shots {
            let cap = reach::ground_reach(standoff.reach_ft, shots.launch.height_ft);
            let beyond = usual[0].max(usual[1]);
            for range in reach::ranges_beyond(beyond, cap) {
                let first = approach_bearings(base).find_map(|(k, theta)| fits(k, theta, range));
                if let Some(point) = first
                    && shots.reliable(range)
                {
                    return Some(Plan::StandOff {
                        approach: Some(point),
                        range_ft: range,
                    });
                }
            }
        }
    }
    popup.then_some(Plan::PopUp)
}

/// The firing point's bearings from the target: `base` (the helicopter's
/// own) and then outward in [`APPROACH_STEP_DEG`] steps, the right first at
/// each step, all the way round; with the step count `k`.
pub(crate) fn approach_bearings(base: f64) -> impl Iterator<Item = (i32, f64)> {
    let steps = (180. / APPROACH_STEP_DEG).round() as i32;
    (0..=steps).flat_map(move |k| {
        let sides: &[f64] = if k == 0 || k == steps {
            &[1.]
        } else {
            &[1., -1.]
        };
        sides
            .iter()
            .map(move |side| (k, base + side * f64::from(k) * APPROACH_STEP_DEG))
    })
}

/// Whether the pass in hand still stays outside every ring: a ring learned
/// after the choice may block it ("not pressed through", slice H4). The
/// pop-up needs the target and the line to it clear; a stand-off its firing
/// point and the advance beyond it (`standoff`'s, else
/// [`STANDOFF_ADVANCE_FT`]), and flown straight at the target the line to
/// the firing point.
pub fn holds(
    plan: Plan,
    own: Vector,
    target: Vector,
    standoff: Option<StandOff<'_>>,
    rings: &[Ring],
) -> bool {
    match plan {
        Plan::PopUp => inside(rings, target).is_none() && clear(rings, own, target),
        // Judged on the target's motion and the rings' by
        // [`crate::ai::helicopter_ahead::holds`].
        Plan::Ahead(_) => true,
        Plan::StandOff { approach, range_ft } => {
            let point = approach.unwrap_or_else(|| {
                let [x, z] = along(target, bearing(target, own), range_ft);
                [x, own[1], z]
            });
            let advance = standoff.map_or(STANDOFF_ADVANCE_FT, |st| st.advance_ft(range_ft));
            let [ex, ez] = along(target, bearing(target, point), (range_ft - advance).max(0.));
            let end = [ex, own[1], ez];
            inside(rings, point).is_none()
                && clear(rings, point, end)
                && (approach.is_some() || clear(rings, own, point))
        }
    }
}

/// Where the helicopter holds off: the nearest point to it whose
/// [`HOLD_RADIUS_FT`] circle lies outside every ring; of that point and the
/// points slid round the nearest ring's centre by [`MASK_TURNS_DEG`], the
/// first the terrain hides from that centre at the masked height, else that
/// point. `ground` is the terrain height (none: no masking).
pub fn hold_centre(
    own: Vector,
    rings: &[Ring],
    ground: Option<&dyn Fn(f64, f64) -> f64>,
) -> Vector {
    let base = push_out(own, rings, own);
    let (Some(ground), Some(site)) = (
        ground,
        rings
            .iter()
            .min_by(|a, b| {
                (horizontal(a.centre, base) - a.radius_ft)
                    .total_cmp(&(horizontal(b.centre, base) - b.radius_ft))
            })
            .copied(),
    ) else {
        return base;
    };
    let distance = horizontal(site.centre, base);
    let from_site = bearing(site.centre, base);
    for turn in MASK_TURNS_DEG {
        let [x, z] = along(site.centre, from_site + turn, distance);
        let candidate = push_out([x, own[1], z], rings, own);
        let fits = rings
            .iter()
            .all(|r| horizontal(r.centre, candidate) >= r.radius_ft + HOLD_RADIUS_FT - 1.);
        if fits && masked(site.centre, candidate, ground) {
            return candidate;
        }
    }
    base
}

/// Whether the terrain hides a point at the masked height from a site
/// (seen from 40 ft over its ground, as the helicopter defence's).
pub fn masked(site: Vector, point: Vector, ground: &dyn Fn(f64, f64) -> f64) -> bool {
    let eye = [
        site[0],
        ground(site[0], site[2]) + crate::ai::helicopter_defense::SITE_HEIGHT_FT,
        site[2],
    ];
    let at = [point[0], ground(point[0], point[2]) + MASK_AGL_FT, point[2]];
    crate::combat::live::terrain_hit(eye, at, &|x, z| ground(x, z)).is_some()
}

/// Moves `point` out of every ring (plus the hold circle), radially from
/// each ring's centre; `fallback` gives the direction when it sits on one.
fn push_out(mut point: Vector, rings: &[Ring], fallback: Vector) -> Vector {
    for _ in 0..8 {
        let mut moved = false;
        for r in rings {
            let need = r.radius_ft + HOLD_RADIUS_FT;
            let d = horizontal(r.centre, point);
            if d < need {
                let b = if d > 1. {
                    bearing(r.centre, point)
                } else if horizontal(r.centre, fallback) > 1. {
                    bearing(r.centre, fallback)
                } else {
                    0.
                };
                let [x, z] = along(r.centre, b, need);
                point = [x, point[1], z];
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    point
}

/// What the helicopter decided about a threat, for the radio, the driver
/// and the replay's `ai.strike` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A ring blocks every attack on its target: it holds off outside it.
    HoldOff,
    /// The ring is gone (SEAD killed the site, or the threat went quiet):
    /// it resumes the attack.
    Resume,
    /// The wait ran out: it takes another target outside every ring.
    Retarget,
    /// The wait ran out and nothing is outside the rings: it goes home.
    Home,
    /// Engaged by a site it did not know (an ambush): after the break it
    /// leaves the ambush ring. A shot from a site it knew is not one.
    Leave,
}

impl Decision {
    /// The fixed `reason` key of the replay's `ai.strike` event.
    pub fn reason(self) -> &'static str {
        match self {
            Self::HoldOff => "threat_hold_off",
            Self::Resume => "threat_resume",
            Self::Retarget => "threat_retarget",
            Self::Home => "threat_home",
            Self::Leave => "threat_leave",
        }
    }
    /// The words in the replay and the driver.
    pub fn words(self) -> &'static str {
        match self {
            Self::HoldOff => "threat on target, holding off",
            Self::Resume => "threat clear, resuming",
            Self::Retarget => "target defended, switching targets",
            Self::Home => "target defended, RTB",
            Self::Leave => "engaged, leaving the threat ring",
        }
    }
}

/// A decision and when it was made, for the target it concerns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Call {
    pub decision: Decision,
    pub tick: u64,
    pub target: Option<u32>,
    /// The ring that blocked (a defence's id, or [`AMBUSH_ID`]).
    pub ring: Option<u32>,
    /// The radio has said it.
    pub told: bool,
}

/// The helicopter's threat state inside the strike.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeliThreat {
    /// Holding off: the point it circles and the tick it began.
    pub hold: Option<(Vector, u64)>,
    /// The hold-off has been called (after [`HOLD_CALL_TICKS`]): a resume is
    /// called too. A plan that comes clear at once is flown without a word.
    pub announced: bool,
    /// The three minutes ran out once: blocked targets are no longer waited
    /// for.
    pub waited: bool,
    /// The ring of a threat it did not know, and the last tick it was
    /// engaged.
    pub ambush: Option<(Ring, u64)>,
    /// The stand-off's firing point off the straight line, and its range.
    pub approach: Option<Vector>,
    pub standoff_ft: Option<f64>,
    /// A move-ahead ambush on a moving target, when the stand-off is one
    /// ([`crate::ai::helicopter_ahead`]; John, 2026-10-29).
    pub ahead: Option<crate::ai::helicopter_ahead::Ahead>,
    /// The last decision.
    pub call: Option<Call>,
}

impl HeliThreat {
    /// Records `decision` (the radio says it once).
    pub fn decide(
        &mut self,
        decision: Decision,
        tick: u64,
        target: Option<u32>,
        ring: Option<u32>,
    ) {
        self.call = Some(Call {
            decision,
            tick,
            target,
            ring,
            told: false,
        });
    }
}

#[cfg(test)]
#[path = "helicopter_threat_tests.rs"]
mod tests;
