//! The strike cockpit: one record per aircraft of its ground-attack state,
//! whoever flies it (docs/spec/ai-strike.md, "One cockpit, two pilots";
//! docs/ARCHITECTURE.md, "The strike cockpit").
//!
//! The record holds the strike target (a ground object, a ground point, or
//! none), the selected station and whether it is armed, the delivery mode
//! (CCIP or LVL), the seeker mode (CUED or BORE), the targeting sight and the
//! laser with its hand setting. It lives in three places, always the same
//! type:
//!
//! - **A human flying** ([`Ownship::cockpit`]): the keys edit it. The
//!   designation's home stays the aircraft's sensors (the radar pages and
//!   picks); combat copies the designated object into the record's target at
//!   each step and after each command, so everything that reads the record
//!   sees what the human designated.
//! - **A plane a human gave back** ([`Kept::cockpit`], slice FX-J): the AI
//!   flies with what the human left; the sight and the laser step on.
//! - **An AI striker** ([`State::ai_cockpit_mut`]): the AI's strike
//!   controller edits it (target, station, laser on or off) and leaves the
//!   rest alone. An AI aircraft that never had a human gets one the first
//!   time the AI asks for it.
//!
//! One release function serves every launcher ([`State::release_round`]):
//! a human's trigger and an AI's release both turn a station's store into
//! rounds the same way (bomb packs with their offsets and scatter, pod
//! rockets, guided flights with their spot or stored point, the laser's
//! release note). The delivery solution, the guided sight and the guided
//! readiness are shared the same way. The AI releases on the shared solution
//! ([`State::strike_release_due`]); skill enters through [`ReleaseAim`] (the
//! pass's aim offset and release gate), which the strike tactics slice
//! (ST-T) draws. Opinionated (John, 2026-10-11) for the principle; the
//! record layout and the target copy are agent decisions.
use super::{
    Configuration, Kept, Launcher, MAX_PROJECTILES, Ownship, Projectile, Readiness, Side, State,
    Station, Target,
};
use crate::attitude::{Vector, dot};
use crate::combat::delivery::{self, Delivery, DeliveryMode};
use crate::combat::guided::GuidedSight;
use crate::combat::laser::{self, Laser, LaserSpot};
use crate::combat::missiles::{self, Flight, Guidance, LaunchMode, Motion, TargetRole};
use crate::combat::sight::Sight;
use crate::combat::{FallState, ballistics, launch_speed};
use std::collections::BTreeMap;
use tore_formats::weapons::Weapon;

/// What a strike cockpit attacks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrikeTarget {
    /// A ground object (or, for a human, whatever the sensors designate).
    Object(u32),
    /// A point on the ground: where a JDAM is sent with no object.
    Point(Vector),
}

/// One aircraft's ground-attack state (see the module notes).
#[derive(Clone, Debug, PartialEq)]
pub struct StrikeCockpit {
    /// What it attacks. For a human, the designated object, copied from the
    /// sensors by combat.
    pub target: Option<StrikeTarget>,
    /// The selected station, and whether a weapon is armed (NAV otherwise).
    pub selected: usize,
    pub armed: bool,
    /// The seeker mode (CUED or BORESIGHT).
    pub launch_mode: LaunchMode,
    /// The bomb sight's mode (Ctrl+M): CCIP or the level release.
    pub delivery_mode: DeliveryMode,
    /// The targeting sight of an aircraft with a laser designator or a FLIR
    /// (the AC-130's is its gunsight, not this).
    pub sight: Option<Sight>,
    /// The laser designator, the hand setting (Ctrl+L) included: off on an
    /// aircraft without one.
    pub laser: Laser,
}

impl StrikeCockpit {
    /// The cockpit of a fresh aircraft built from `config`: the first
    /// station armed (NAV with none), CUED, CCIP, the sight at its default
    /// look and the laser off.
    pub fn new(config: &Configuration) -> Self {
        let sight = crate::combat::gunship::State::new(config)
            .is_none()
            .then(|| config.sight_sensor().map(Sight::new))
            .flatten();
        Self {
            target: None,
            selected: 0,
            armed: !config.stations.is_empty(),
            launch_mode: LaunchMode::Cued,
            // A Heavy starts in LVL (no CCIP in retail); everything else in CCIP.
            delivery_mode: crate::models::strike_class::default_delivery(
                config.strike_class,
                false,
                0.,
            ),
            sight,
            laser: Laser::default(),
        }
    }

    /// The cockpit an AI striker starts with: as [`Self::new`], but with
    /// nothing armed until its controller picks a store.
    pub fn for_ai(config: &Configuration) -> Self {
        Self {
            armed: false,
            ..Self::new(config)
        }
    }

    /// The target object, when the target is one.
    pub fn target_object(&self) -> Option<u32> {
        match self.target {
            Some(StrikeTarget::Object(id)) => Some(id),
            _ => None,
        }
    }

    /// The selected station's weapon, when one is armed.
    pub fn armed_weapon<'a>(&self, config: &'a Configuration) -> Option<&'a Weapon> {
        self.armed
            .then(|| config.stations.get(self.selected).map(|s| &s.weapon))
            .flatten()
    }

    /// A laser weapon is selected and armed.
    pub fn laser_weapon_armed(&self, config: &Configuration) -> bool {
        self.armed_weapon(config)
            .is_some_and(laser::is_laser_weapon)
    }

    /// The point a JDAM would store or an AT-2 would steer at, and the
    /// object it belongs to (`u32::MAX` for a bare point): the target object,
    /// on the ground and standing; else, for a JDAM, the target point or the
    /// sight's held point.
    pub fn aim_point(&self, kind: Guidance, targets: &[Target]) -> Option<(Vector, u32)> {
        aim_point(self.target, self.sight.as_ref(), kind, targets)
    }

    /// Whether a guided air-to-ground weapon of `kind` may leave now, given
    /// the designated object as the cockpit sees it (`None` when nothing on
    /// the ground is designated): only a JDAM with nothing to fly to and an
    /// AT-2 with no living target are held back (docs/spec/air-to-ground.md,
    /// "Warnings"). A laser weapon goes with or without a spot. A human's
    /// readiness and the AI's both answer it here.
    pub fn guided_readiness(&self, kind: Guidance, designated: Option<&Target>) -> Readiness {
        let point = matches!(self.target, Some(StrikeTarget::Point(_)))
            || self.sight.is_some_and(|sight| sight.held_point().is_some());
        match kind {
            Guidance::Coordinate if designated.is_none_or(|t| t.hp <= 0) && !point => {
                Readiness::NoTarget
            }
            Guidance::Command => match designated {
                None => Readiness::NoTarget,
                Some(t) if t.hp <= 0 => Readiness::TargetDestroyed,
                Some(_) => Readiness::Ready,
            },
            _ => Readiness::Ready,
        }
    }
}

/// [`StrikeCockpit::aim_point`] for a target and a sight.
pub(super) fn aim_point(
    target: Option<StrikeTarget>,
    sight: Option<&Sight>,
    kind: Guidance,
    targets: &[Target],
) -> Option<(Vector, u32)> {
    let object = match target {
        Some(StrikeTarget::Object(id)) => targets.iter().find(|t| t.id == id),
        _ => None,
    }
    .filter(|t| t.hp > 0 && !t.airborne)
    .map(|t| (t.position, t.id));
    match kind {
        Guidance::Coordinate => object.or_else(|| {
            match target {
                Some(StrikeTarget::Point(point)) => Some(point),
                _ => None,
            }
            .or_else(|| sight.and_then(Sight::held_point))
            .map(|point| (point, u32::MAX))
        }),
        _ => object,
    }
}

/// The strike cockpit of an AI-flown aircraft that no human left: its side
/// (for its laser's spot) and the record.
#[derive(Clone, Debug, PartialEq)]
pub struct AiCockpit {
    pub side: Side,
    pub cockpit: StrikeCockpit,
}

impl std::ops::Deref for Ownship {
    type Target = StrikeCockpit;
    fn deref(&self) -> &StrikeCockpit {
        &self.cockpit
    }
}
impl std::ops::DerefMut for Ownship {
    fn deref_mut(&mut self) -> &mut StrikeCockpit {
        &mut self.cockpit
    }
}
impl std::ops::Deref for Kept {
    type Target = StrikeCockpit;
    fn deref(&self) -> &StrikeCockpit {
        &self.cockpit
    }
}
impl std::ops::DerefMut for Kept {
    fn deref_mut(&mut self) -> &mut StrikeCockpit {
        &mut self.cockpit
    }
}

impl Ownship {
    /// Copies the designated object into the cockpit's target: the sensors
    /// stay the designation's home, the record follows them.
    pub(super) fn sync_target(&mut self) {
        self.cockpit.target = self.designated().map(StrikeTarget::Object);
    }
}

/// Every strike cockpit combat holds, for the stages that read them all:
/// the ownships' (aircraft order), then the kept ones (aircraft order), then
/// the AI strikers' (aircraft order, skipping an aircraft a human or a kept
/// cockpit already stands for).
#[derive(Clone, Copy)]
pub(super) struct Cockpits<'a> {
    pub ships: &'a [Ownship],
    pub kept: &'a BTreeMap<u32, Kept>,
    pub ai: &'a BTreeMap<u32, AiCockpit>,
}

impl<'a> Cockpits<'a> {
    pub fn iter(self) -> impl Iterator<Item = (u32, Side, &'a StrikeCockpit)> + 'a {
        let ships = self.ships;
        let kept = self.kept;
        ships
            .iter()
            .map(|own| (own.aircraft, own.side, &own.cockpit))
            .chain(
                kept.iter()
                    .map(|(aircraft, kept)| (*aircraft, kept.side, &kept.cockpit)),
            )
            .chain(
                self.ai
                    .iter()
                    .filter(move |(aircraft, _)| {
                        !kept.contains_key(aircraft)
                            && !ships.iter().any(|own| own.aircraft == **aircraft)
                    })
                    .map(|(aircraft, ai)| (*aircraft, ai.side, &ai.cockpit)),
            )
    }
    /// `aircraft`'s cockpit and side: the ownship's, else the kept one, else
    /// the AI's.
    pub fn get(self, aircraft: u32) -> Option<(Side, &'a StrikeCockpit)> {
        if let Some(own) = self.ships.iter().find(|own| own.aircraft == aircraft) {
            return Some((own.side, &own.cockpit));
        }
        if let Some(kept) = self.kept.get(&aircraft) {
            return Some((kept.side, &kept.cockpit));
        }
        self.ai.get(&aircraft).map(|ai| (ai.side, &ai.cockpit))
    }
    /// The AI flies `aircraft` with a cockpit combat holds (kept or its own).
    pub fn flown_by_ai(self, aircraft: u32) -> bool {
        self.kept.contains_key(&aircraft) || self.ai.contains_key(&aircraft)
    }
}

/// One round leaving a station, a human's or the AI's: everything
/// [`State::release_round`] needs besides the cockpit.
pub(super) struct Round<'a> {
    /// The projectile number.
    pub id: u32,
    pub owner: u32,
    pub side: Side,
    /// The station and its index in the aircraft's configuration.
    pub index: usize,
    pub station: &'a Station,
    /// The round's place in this release (a pack's bomb `round` takes its
    /// offset).
    pub round: usize,
    pub launcher: Launcher,
    /// The quarter second it leaves in.
    pub launched_t: u16,
    /// The weapon record the round carries: the AI's own copy. `None` for a
    /// human's round, which reads its owner's configuration.
    pub record: Option<Weapon>,
    /// The aircraft the round warns (an AI round aimed at a human).
    pub incoming: Option<u32>,
    /// The AC-130 gun's slot, barrel heading and elevation.
    pub gun_pose: Option<(usize, f64, f64)>,
    /// A missile's guidance and the legacy seeker's target, as a human's
    /// fire loop works them out (none for a strike store).
    pub guidance: Option<Flight>,
    pub seeker_target: Option<u32>,
    /// A gun round's place in its burst and whether it is a tracer.
    pub gun_round: Option<u8>,
    pub tracer: bool,
    /// The whole releases the station holds once this one has gone, which
    /// picks the side of a paired station it leaves from
    /// ([`crate::combat::hardpoints::Rack::release_mount`]).
    pub units_after: u32,
}

/// The stores the AI releases through the shared release ([`State::release_strike`]):
/// bombs (packs included), rocket pods and the guided air-to-ground weapons
/// (laser, JDAM, AT-2). Missiles with a seeker profile (Mavericks, HARMs,
/// radar AGMs) and guns keep the AI's own release.
pub fn is_strike_store(w: &Weapon) -> bool {
    ballistics::is_bomb(w)
        || (ballistics::is_pod(w) && w.seeker.signature == 0 && !super::is_gun(w))
        || crate::combat::guided::kind(w).is_some()
}

/// What the AI releases: one trigger call of a station, as a human's press.
#[derive(Clone, Copy)]
pub struct StrikeRelease<'a> {
    /// The releasing aircraft, which must have a cockpit the AI flies with
    /// ([`State::ai_cockpit_mut`]).
    pub aircraft: u32,
    /// Its configuration: the station's mount and record.
    pub config: &'a Configuration,
    pub station: usize,
    /// Where it is and how it points.
    pub launcher: Launcher,
    /// The aircraft the rounds warn (a round aimed at a human).
    pub incoming: Option<u32>,
    /// The rounds the station holds after the debit, which picks the side
    /// of a paired station the release leaves from.
    pub rounds_after: u32,
}

/// Where and how tightly the AI aims one pass: the skill hook of
/// docs/spec/ai-strike.md, "Skill and release precision" (John's precision
/// rule, 2026-10-11: a release circle around the target by skill, Ace 100 m,
/// Experienced 175 m, Average 250 m, Novice 325 m, the aim offset drawn once
/// a pass, 90% inside the circle and 10% outside). **ST-T hook:** the strike
/// tactics slice makes the per-pass draw (seeded by mission, aircraft and
/// pass, so every host and a replay agree) and hands the result to
/// [`State::strike_release_due`]. It is for **unguided** weapons only (bombs,
/// packs, rockets): they aim the pipper or the T at [`Self::point`], and the
/// weapon's own scatter comes on top, as for a human. Smart weapons use the
/// targeting systems at every skill (John, 2026-10-11): the laser spot sits
/// on the target, a JDAM stores the target's point, seekers lock; a guided
/// release ignores the aim. Until ST-T draws, [`Self::on_target`] aims at
/// the target itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReleaseAim {
    /// The aim point's ground offset from the target, feet along x and z.
    pub offset_ft: [f64; 2],
    /// The release gate: the predicted impact on the aim point's ground (as
    /// the LVL cue is timed; [`super::super::delivery::Delivery::impact_at`])
    /// must lie within this many feet, along the ground, of the aim point.
    pub gate_ft: f64,
    /// A laser glide bomb let go by hand as an unguided bomb (slice ST-G):
    /// judged on the bomb equation's sight, not the guided one.
    pub ballistic: bool,
}

impl ReleaseAim {
    /// At the target, released once the predicted impact is within
    /// `gate_ft` of it.
    pub fn on_target(gate_ft: f64) -> Self {
        Self {
            offset_ft: [0., 0.],
            gate_ft,
            ballistic: false,
        }
    }
    /// The aim point for a target at `target`.
    pub fn point(&self, target: Vector) -> Vector {
        [
            target[0] + self.offset_ft[0],
            target[1],
            target[2] + self.offset_ft[1],
        ]
    }
}

/// A guided weapon that arrives inside the level T's bar still releases
/// within this share of its reach past the bar (fitted, agent).
pub const GUIDED_LATE_SHARE: f64 = 0.05;

/// A guided weapon is due for release on its sight: IN RNG, no MASK, and its
/// ground range at most the level T's bar plus [`GUIDED_LATE_SHARE`] of the
/// reach. MASK (a laser spot or an AT-2's target hidden from the launcher by
/// terrain) leaves IN RNG alone on the human's sight (slice CL-T), so the AI
/// reads it here and holds its release until the line clears, as it did
/// when a blocked line cleared IN RNG.
pub fn guided_due(sight: &GuidedSight) -> bool {
    if let Some(hit) = sight.energy_intercept {
        return hit && sight.in_reach && sight.advisory.is_none();
    }
    let (Some(range), Some(bar), Some(reach)) =
        (sight.ground_range_ft, sight.release_range_ft(), sight.reach)
    else {
        return false;
    };
    let window = GUIDED_LATE_SHARE * (reach.maximum_ft - reach.minimum_ft);
    sight.in_reach
        && sight.advisory != Some(crate::combat::guided::Advisory::Masked)
        && range <= bar + window
}

impl State {
    /// The strike cockpits as every stage reads them, with the ownships
    /// given separately (a step works them out of the state).
    pub(super) fn cockpits_with<'a>(&'a self, ships: &'a [Ownship]) -> Cockpits<'a> {
        Cockpits {
            ships,
            kept: &self.kept,
            ai: &self.ai_cockpits,
        }
    }

    /// `aircraft`'s strike cockpit, whoever flies it: the human's, the one a
    /// human left while the AI flies it, or the AI striker's.
    pub fn strike_cockpit(&self, aircraft: u32) -> Option<&StrikeCockpit> {
        self.cockpits_with(&self.ownships)
            .get(aircraft)
            .map(|(_, cockpit)| cockpit)
    }

    /// The cockpit the AI flies `aircraft` (of `side`, built from `config`)
    /// with: the one its human left, else its own, made now if it has none.
    /// `None` while a human flies it.
    pub fn ai_cockpit_mut(
        &mut self,
        aircraft: u32,
        side: Side,
        config: &Configuration,
    ) -> Option<&mut StrikeCockpit> {
        if self.ownship(aircraft).is_some() {
            return None;
        }
        if let Some(kept) = self.kept.get_mut(&aircraft) {
            return Some(&mut kept.cockpit);
        }
        Some(
            &mut self
                .ai_cockpits
                .entry(aircraft)
                .or_insert_with(|| AiCockpit {
                    side,
                    cockpit: StrikeCockpit::for_ai(config),
                })
                .cockpit,
        )
    }

    /// The AI strikers' own cockpits, in aircraft order (a plane a human
    /// gave back has its cockpit in [`State::kept`] instead).
    pub fn ai_cockpits(&self) -> impl Iterator<Item = (u32, &AiCockpit)> {
        self.ai_cockpits
            .iter()
            .map(|(aircraft, ai)| (*aircraft, ai))
    }

    /// Takes an AI striker's own cockpit, for a human taking the aircraft.
    pub fn take_ai_cockpit(&mut self, aircraft: u32) -> Option<AiCockpit> {
        self.ai_cockpits.remove(&aircraft)
    }

    /// The target object of `cockpit`, as a target row on the ground.
    fn strike_object<'a>(&'a self, cockpit: &StrikeCockpit) -> Option<&'a Target> {
        cockpit
            .target_object()
            .and_then(|id| self.targets.iter().find(|t| t.id == id))
            .filter(|t| !t.airborne)
    }

    /// Whether the AI's selected store in `aircraft`'s cockpit may leave now,
    /// as a human's readiness would say for the same store: the launcher
    /// lives, a store is armed, there is room for its rounds, and a guided
    /// weapon has what it needs ([`StrikeCockpit::guided_readiness`]). The
    /// AI's own weapon service keeps its rounds and envelopes.
    pub fn strike_readiness(
        &self,
        aircraft: u32,
        config: &Configuration,
        launcher: Launcher,
    ) -> Readiness {
        if !launcher.alive {
            return Readiness::LauncherLost;
        }
        let Some(cockpit) = self.strike_cockpit(aircraft) else {
            return Readiness::Safe;
        };
        let Some(w) = cockpit.armed_weapon(config) else {
            return Readiness::Safe;
        };
        if self.projectiles.len() >= MAX_PROJECTILES {
            return Readiness::Capacity;
        }
        match (self.weapon_rules.uses_seeker())
            .then(|| crate::combat::guided::kind(w))
            .flatten()
        {
            Some(kind) => cockpit.guided_readiness(kind, self.strike_object(cockpit)),
            None => Readiness::Ready,
        }
    }

    /// The bomb or rocket sight's solution for a cockpit's selected station:
    /// against its target on the ground, with the time to impact of the
    /// aircraft's last bomb, predicted from `mount`, where the store that
    /// leaves next hangs. The same for a human and the AI.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn delivery_of(
        &self,
        aircraft: u32,
        cockpit: &StrikeCockpit,
        config: &Configuration,
        launcher: &Launcher,
        mount: crate::attitude::Vector,
        ground: &impl Fn(f64, f64) -> f64,
        clock: delivery::Clock,
        ballistic: bool,
        beyond_life: bool,
    ) -> Option<Delivery> {
        if !cockpit.armed || !launcher.alive {
            return None;
        }
        let station = config.stations.get(cockpit.selected)?;
        let (target, target_velocity) = cockpit
            .target_object()
            .and_then(|id| self.targets.iter().find(|t| t.id == id))
            .filter(|t| t.role == TargetRole::Surface && t.hp > 0)
            .map(|t| (t.position, t.velocity))
            .or(match cockpit.target {
                Some(StrikeTarget::Point(point)) => Some((point, [0.; 3])),
                _ => None,
            })
            .map_or((None, [0.; 3]), |(p, v)| (Some(p), v));
        let falling = self
            .projectiles
            .iter()
            .filter(|p| p.owner == aircraft)
            .filter_map(|p| self.bombs.get(&p.id).map(|motion| (p, motion)))
            .max_by_key(|(p, _)| p.id)
            .and_then(|(p, motion)| {
                motion
                    .impact(&ground)
                    .map(|impact| (impact.seconds - p.age as f64 / 120.).max(0.))
            });
        delivery::solve(
            delivery::Situation {
                index: cockpit.selected,
                station,
                mount,
                launcher,
                mode: cockpit.delivery_mode,
                target,
                target_velocity,
                clock,
                falling,
                ballistic,
                beyond_life,
            },
            ground,
        )
    }

    /// The clock of the next step, which the rounds the AI releases between
    /// steps first move on.
    fn next_clock(&self) -> delivery::Clock {
        let total = self.service_remainder + 256;
        delivery::Clock {
            tick: self.tick,
            service: (total / 120) as i16,
            remainder: total % 120,
        }
    }

    /// The bomb or rocket solution for the AI flying `aircraft`: what a
    /// human's HUD would show for the same cockpit and launcher. With
    /// `ballistic` a laser glide bomb is solved on the bomb equation, as an
    /// AI that drops it by hand sees it (slice ST-G). `mount` is where the
    /// AI's next store from the selected station leaves (its side of a
    /// paired station), as the release will put it.
    pub fn strike_delivery(
        &self,
        aircraft: u32,
        config: &Configuration,
        launcher: &Launcher,
        mount: crate::attitude::Vector,
        ballistic: bool,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Option<Delivery> {
        let cockpit = self.strike_cockpit(aircraft)?;
        self.delivery_of(
            aircraft,
            cockpit,
            config,
            launcher,
            mount,
            ground,
            self.next_clock(),
            ballistic,
            // The AI's gate reads the in-life impact only.
            false,
        )
    }

    /// The guided sight (aim, reach, IN RNG, level T) for the AI flying
    /// `aircraft`: what a human's targeting page would show.
    pub fn strike_guided_sight(
        &self,
        aircraft: u32,
        config: &Configuration,
        launcher: &Launcher,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Option<GuidedSight> {
        let (side, cockpit) = self.cockpits_with(&self.ownships).get(aircraft)?;
        let sight = super::guided::sight(
            aircraft,
            side,
            cockpit,
            config,
            launcher,
            &super::guided::SightContext {
                spots: &self.laser_spots(),
                targets: &self.targets,
                weather: &self.sensor_weather,
                ground,
            },
        )?;
        let w = &config.stations.get(cockpit.selected)?.weapon;
        Some(self.energy_guided_output(aircraft, cockpit.selected, w, *launcher, sight))
    }

    /// Whether the AI flying `aircraft` should release its selected store
    /// now, on the shared solution and `aim` (docs/spec/ai-strike.md, "Skill
    /// and release precision"):
    ///
    /// - a bomb or rocket when the sight's impact (a bomb's mean path to the
    ///   terrain in CCIP, the pipper's own point; in LVL brought down to the
    ///   aim point's ground, as the cue is timed; a rocket's flown impact)
    ///   lies within `aim.gate_ft` of the aim point ([`ReleaseAim::point`] of
    ///   the cockpit's target);
    /// - a guided weapon (the aim does not apply: no skill offset for smart
    ///   weapons) when its aim on the target (its spot or its stored point)
    ///   is IN RNG and its ground range is at most the level T's bar plus
    ///   [`GUIDED_LATE_SHARE`] of the reach.
    ///
    /// The release itself is [`State::release_strike`].
    pub fn strike_release_due(
        &self,
        aircraft: u32,
        config: &Configuration,
        launcher: &Launcher,
        mount: crate::attitude::Vector,
        aim: ReleaseAim,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> bool {
        let Some(cockpit) = self.strike_cockpit(aircraft) else {
            return false;
        };
        let Some(w) = cockpit.armed_weapon(config) else {
            return false;
        };
        if !aim.ballistic && crate::combat::guided::kind(w).is_some() {
            return self
                .strike_guided_sight(aircraft, config, launcher, ground)
                .is_some_and(|sight| guided_due(&sight));
        }
        let Some(solution) =
            self.strike_delivery(aircraft, config, launcher, mount, aim.ballistic, ground)
        else {
            return false;
        };
        let Some(target) = self.strike_point(cockpit) else {
            return false;
        };
        let point = aim.point(target);
        // The cue the sight shows: the pipper where the path meets the
        // terrain, or in LVL the path brought down to the aim point's ground
        // (slices ST-LV and ST-G).
        let Some(impact) = solution.cue(ground(point[0], point[2])) else {
            return false;
        };
        let miss = [
            impact.position[0] - point[0],
            0.,
            impact.position[2] - point[2],
        ];
        dot(miss, miss).sqrt() <= aim.gate_ft
    }

    /// The point a cockpit's target stands at: its object's position, or
    /// its point.
    fn strike_point(&self, cockpit: &StrikeCockpit) -> Option<Vector> {
        match cockpit.target? {
            StrikeTarget::Object(id) => self
                .targets
                .iter()
                .find(|t| t.id == id && t.hp > 0)
                .map(|t| t.position),
            StrikeTarget::Point(point) => Some(point),
        }
    }

    /// The AI's release: one trigger call of `release.station` from the
    /// cockpit it flies `release.aircraft` with, through the same release a
    /// human's trigger takes ([`State::release_round`]): a pack leaves whole,
    /// a pod fires one rocket, a guided weapon takes its spot or stored
    /// point, and the laser notes it. `next_id` numbers the rounds. Returns
    /// their numbers, or the readiness that held the release back. The AI's
    /// weapon service has already debited the store.
    pub fn release_strike(
        &mut self,
        release: StrikeRelease<'_>,
        mut next_id: impl FnMut() -> u32,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Result<Vec<u32>, Readiness> {
        let StrikeRelease {
            aircraft,
            config,
            station: index,
            launcher,
            incoming,
            rounds_after,
        } = release;
        let station = config.stations.get(index).ok_or(Readiness::Safe)?;
        let units_after = crate::combat::hardpoints::units(
            rounds_after,
            u32::from(station.weapon.burst.actual_rounds_per_game),
        );
        if self.ownship(aircraft).is_some() {
            return Err(Readiness::Safe);
        }
        // The AI's release is of the station it asks for.
        let side = {
            let cockpit = self.flown_cockpit_mut(aircraft).ok_or(Readiness::Safe)?;
            cockpit.selected = index;
            cockpit.armed = true;
            self.flown_side(aircraft)
        };
        let readiness = self.strike_readiness(aircraft, config, launcher);
        if readiness != Readiness::Ready {
            return Err(readiness);
        }
        if launch_speed(&station.weapon.movement, (launcher.speed_fps * 256.) as i32).is_err() {
            return Err(Readiness::Safe);
        }
        let spots = self.laser_spots();
        let launched_t = (self.tick / 30) as u16;
        let mut cockpit = self.take_flown_cockpit(aircraft).ok_or(Readiness::Safe)?;
        let mut ids = Vec::new();
        for round in 0..ballistics::release_count(&station.weapon) {
            if self.projectiles.len() >= MAX_PROJECTILES {
                break;
            }
            let id = next_id();
            self.release_round(
                &mut cockpit,
                Round {
                    id,
                    owner: aircraft,
                    side,
                    index,
                    station,
                    round,
                    launcher,
                    launched_t,
                    record: Some(station.weapon.clone()),
                    incoming,
                    gun_pose: None,
                    guidance: None,
                    seeker_target: None,
                    gun_round: None,
                    tracer: false,
                    units_after,
                },
                &spots,
                ground,
            );
            ids.push(id);
        }
        self.put_flown_cockpit(aircraft, cockpit);
        Ok(ids)
    }

    /// The kept or AI cockpit of `aircraft`, for a change.
    fn flown_cockpit_mut(&mut self, aircraft: u32) -> Option<&mut StrikeCockpit> {
        if let Some(kept) = self.kept.get_mut(&aircraft) {
            return Some(&mut kept.cockpit);
        }
        self.ai_cockpits
            .get_mut(&aircraft)
            .map(|ai| &mut ai.cockpit)
    }
    fn flown_side(&self, aircraft: u32) -> Side {
        self.kept
            .get(&aircraft)
            .map(|kept| kept.side)
            .or_else(|| self.ai_cockpits.get(&aircraft).map(|ai| ai.side))
            .unwrap_or(super::NO_SIDE)
    }
    /// Lends the kept or AI cockpit of `aircraft` out for a release; it
    /// goes back with [`Self::put_flown_cockpit`].
    fn take_flown_cockpit(&mut self, aircraft: u32) -> Option<StrikeCockpit> {
        let placeholder = || StrikeCockpit {
            target: None,
            selected: 0,
            armed: false,
            launch_mode: LaunchMode::Cued,
            delivery_mode: DeliveryMode::default(),
            sight: None,
            laser: Laser::default(),
        };
        self.flown_cockpit_mut(aircraft)
            .map(|cockpit| std::mem::replace(cockpit, placeholder()))
    }
    fn put_flown_cockpit(&mut self, aircraft: u32, cockpit: StrikeCockpit) {
        if let Some(slot) = self.flown_cockpit_mut(aircraft) {
            *slot = cockpit;
        }
    }

    /// One round leaving its station, a human's or the AI's: the one release
    /// function (docs/spec/ai-strike.md, "One cockpit, two pilots"). A bomb
    /// leaves its mount (a pack's bomb `round` at its offset) along the nose
    /// and takes its scatter from combat's random stream; a guided
    /// air-to-ground weapon leaves on its own motion with its guidance
    /// ([`super::guided::release`]: the cockpit's spot or a buddy's, the
    /// stored point, the AT-2's target); a laser weapon's release keeps the
    /// cockpit's laser on; anything else leaves the mount (an AC-130 gun its
    /// muzzle) along the nose, or its barrel. The caller has debited the
    /// store and checked for room. Returns false only for a record whose
    /// speed limits are inverted (no round).
    pub(super) fn release_round(
        &mut self,
        cockpit: &mut StrikeCockpit,
        r: Round<'_>,
        spots: &[LaserSpot],
        ground: &impl Fn(f64, f64) -> f64,
    ) -> bool {
        let station = r.station;
        let w = &station.weapon;
        let launcher = r.launcher;
        let Ok(speed) = launch_speed(&w.movement, (launcher.speed_fps * 256.) as i32) else {
            return false;
        };
        // A store leaves its own side of a paired station; a gun keeps its
        // muzzle (B3). A gun pod's muzzle is the nose of the pod as drawn,
        // under the aircraft (2026-10-16).
        let mount = if super::is_gun(w) {
            station.gun_mount()
        } else {
            station.release_point(r.units_after)
        };
        let position = if let Some((slot, heading, elevation)) = r.gun_pose {
            crate::combat::gunship::muzzle(slot, launcher, heading, elevation)
        } else {
            std::array::from_fn(|i| {
                launcher.position[i]
                    + launcher.basis.right[i] * mount[0]
                    + launcher.basis.up[i] * mount[1]
                    + launcher.basis.forward[i] * mount[2]
            })
        };
        let mut direction = r
            .gun_pose
            .map_or(launcher.basis.forward, |(_, heading, elevation)| {
                crate::combat::gunship::direction(launcher, heading, elevation)
            });
        // A bomb leaves its station (staggered in a pack) along the nose,
        // then takes its own scatter.
        let bomb = ballistics::is_bomb(w).then(|| {
            let (heading, pitch) =
                ballistics::scatter(ballistics::scatter_stream(self.rng, self.tick, r.id));
            ballistics::ReleaseState::from_mount(&launcher, station, mount, r.round)
                .turned(heading, pitch)
        });
        let position = bomb.map_or(position, |release| release.position);
        if let Some(release) = bomb {
            direction = ballistics::BombMotion::new(release).direction(0.);
        }
        let spec = self.weapon_rules.uses_seeker();
        // A guided air-to-ground weapon flies its own guidance on a motion,
        // never the legacy pursuit: its projectile target stays empty.
        let guided_kind = spec.then(|| crate::combat::guided::kind(w)).flatten();
        let guided_velocity =
            guided_kind.map(|_| crate::combat::guided::release_velocity(w, &launcher));
        // A glide bomb that leaves with no spot falls like a dumb bomb, with
        // the scatter a dumb bomb takes (slice ST-G).
        let fall_velocity = guided_velocity.map(|velocity| {
            if crate::combat::guided::is_glide_bomb(w) {
                let (heading, pitch) =
                    ballistics::scatter(ballistics::scatter_stream(self.rng, self.tick, r.id));
                ballistics::ReleaseState {
                    position,
                    velocity,
                    body: super::super::bomb_flight::Body::of(w),
                }
                .turned(heading, pitch)
                .velocity
            } else {
                velocity
            }
        });
        let profile = missiles::Profile::for_weapon(w);
        self.projectiles.push(Projectile {
            id: r.id,
            owner: r.owner,
            weapon: r.record.clone(),
            guidance: r.guidance,
            guidance_ticks: spec.then(|| profile.map(|p| p.guidance_ticks)).flatten(),
            motion: if let Some(velocity) = guided_velocity {
                Some(Motion::launch_rules(
                    w,
                    velocity,
                    position[1],
                    self.weapon_rules,
                ))
            } else {
                (spec && profile.is_some()).then(|| {
                    Motion::launch_rules(w, launcher.velocity, position[1], self.weapon_rules)
                })
            },
            age: 0,
            incoming: r.incoming,
            station: r.index,
            position,
            previous: position,
            direction,
            speed_f8: speed * 256,
            launched_t: r.launched_t,
            target: if w.seeker.signature != 0 && guided_kind.is_none() {
                r.seeker_target
            } else {
                None
            },
            fall: FallState::default(),
            gun_round: r.gun_round,
            tracer: r.tracer,
        });
        if let Some(release) = bomb {
            self.bombs
                .insert(r.id, ballistics::BombMotion::new(release));
        }
        if let (Some(kind), Some(fall_velocity)) = (guided_kind, fall_velocity) {
            let flight = super::guided::release(
                w,
                kind,
                super::guided::Releaser {
                    aircraft: r.owner,
                    side: r.side,
                    cockpit,
                },
                r.index,
                &launcher,
                position,
                fall_velocity,
                spots,
                &self.targets,
                &self.sensor_weather,
                ground,
            );
            self.guided.insert(r.id, flight);
        }
        if laser::is_laser_weapon(w) {
            cockpit.laser.note_release();
        }
        true
    }
}
