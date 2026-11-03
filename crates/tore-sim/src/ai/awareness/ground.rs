//! A strike actor's ground picture (docs/spec/ai-strike.md, "What the AI
//! knows" and "Perceived threat and available speed").
//!
//! Only an aircraft with the Strike role, or with an ordered ground target,
//! has one; every other actor's is `None` and nothing here runs for it, so
//! air-to-air behaviour is unchanged. The picture holds:
//!
//! - the **briefing**: the objective's targets (and, for a SEAD striker, its
//!   air defences) and any ordered target. Known from the start, without a
//!   sensor, as a mission briefing names its target object;
//! - the **ground contacts** the aircraft perceives with its own A-G radar,
//!   FLIR and eye, by the player's list rules, rebuilt every 6 seconds and
//!   followed every quarter second between rebuilds, as the player's pages;
//! - its **memory** of every object it has perceived, for targets of
//!   opportunity and for the defences it knows;
//! - the **threat inputs**: the known SAM and AAA sites with their reach, the
//!   hostile fighters within 30 nm, and retail's threat geometry.
//!
//! A shoulder-fired SAM team (a MANPADS site) is hidden: no briefing names
//! it, the A-G radar and FLIR lists leave it out, and the eye lists it only
//! within a short spotting distance ([`team_spot_ft`]). It is also known from
//! the moment it launches at the aircraft or at a friendly the aircraft sees
//! ([`GroundPicture::unmask`]). T.O.R.E opinionated change requested by John
//! on 2026-10-29: such a team can really ambush an AI aircraft.
//!
//! Every value is a deterministic function of the inputs: no draws.
use crate::ai::Experience;
use crate::ai::engagement::{OPPORTUNITY_RADIUS_FT, StrikeObjective, StrikeRole};
use crate::ai::targeting::{ObjectId, Side, StrikeCandidate, StrikeSource};
use crate::attitude::Vector;
use crate::sensors::{FEET_PER_NAUTICAL_MILE, GroundObject, track};

/// The ground lists are rebuilt every 6 seconds (retail page cadence).
pub const GROUND_REBUILD_TICKS: u64 = track::GROUND_REBUILD_STEPS;
/// Listed contacts follow their objects every quarter second.
pub const GROUND_FOLLOW_TICKS: u64 = track::GROUND_REFRESH_STEPS;
/// The Air threat level's reach: a hostile fighter in the picture within 30 nm
/// (docs/spec/ai-strike.md, "Perceived threat and available speed").
pub const FIGHTER_THREAT_RANGE_FT: f64 = 30. * FEET_PER_NAUTICAL_MILE;
/// Retail's threat geometry (docs/formats/ai.md, "Threat perception"): a
/// ship threat is ignored beyond 40,000 ft,
pub const RETAIL_SHIP_IGNORE_FT: f64 = 40_000.;
/// any other beyond 30 statute miles,
pub const RETAIL_IGNORE_FT: f64 = 158_400.;
/// one inside 20,000 ft is always evaded,
pub const RETAIL_ALWAYS_FT: f64 = 20_000.;
/// and one between is evaded only while the aircraft is within 100 degrees of
/// the threat's nose.
pub const RETAIL_NOSE_DEG: f64 = 100.;
/// How near the eye must be to list a hidden shoulder-fired SAM team, feet,
/// by the pilot's skill (Novice, Average, Experienced, Ace), in clear air
/// (fitted, agent, 2026-10-29: a few men in cover, against the 19,968 ft at
/// which the eye lists every other missile or gun site; the driver's
/// stand-in used 3,000 ft).
pub const TEAM_SPOT_FT: [f64; 4] = [2_000., 3_000., 3_500., 4_000.];

/// How near the eye of a pilot of `experience` must be to list a hidden
/// shoulder-fired SAM team, feet: [`TEAM_SPOT_FT`] scaled by the weather's
/// visual effect along the line (`visual_percent`, 100 in clear air; the
/// sensors' weather selector 0).
pub fn team_spot_ft(experience: Experience, visual_percent: f64) -> f64 {
    TEAM_SPOT_FT[experience as usize] * (visual_percent / 100.).clamp(0., 1.)
}

/// What the host knows of an armed surface unit or a radar, for the threat
/// picture. Captured when the striker perceives it (or is briefed on it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DefenseFacts {
    /// The farthest its missiles may be launched (launch zone maximum),
    /// feet; 0 with none.
    pub missile_reach_ft: f64,
    /// The farthest its missiles may be launched at a helicopter, feet: the
    /// helicopter's threat ring's reach (docs/spec/ai-strike.md, "Helicopter
    /// threat awareness"). Under the energy rules a bound on the launch gate
    /// against a helicopter, which may lie past the launch zone; under the
    /// other rules the launch zone. 0 with no missiles.
    pub rotor_reach_ft: f64,
    /// The farthest its guns fire, feet; 0 with none.
    pub gun_reach_ft: f64,
    /// A radar: its own search radar or a battery's.
    pub radar: bool,
    /// It can guide what it fires: its own seeker, or its battery's radar
    /// stands.
    pub working: bool,
    /// A ship (retail ignores ship threats beyond 40,000 ft).
    pub ship: bool,
    /// Hull heading, degrees (0 north, clockwise): the nose of retail's
    /// threat geometry.
    pub heading_deg: f64,
}

/// One object as the host describes it to the ground picture on a refresh:
/// what the ground lists read of it, its side and, for an armed unit or a
/// radar, its defence facts.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundSubject {
    pub object: GroundObject,
    pub side: Side,
    pub defense: Option<DefenseFacts>,
    /// A shoulder-fired SAM team: hidden until spotted (module comment).
    pub hidden: bool,
}

/// Every ground object of one refresh, with the weather's sensor effects.
#[derive(Clone, Copy, Debug)]
pub struct GroundScene<'a> {
    /// In any order; ids unique.
    pub subjects: &'a [GroundSubject],
    pub weather: &'a crate::sensors::Weather,
    /// The surface radars painting aircraft this tick, as (aircraft, unit)
    /// pairs: the radar and the launchers it guides for (slice H4). A
    /// helicopter learns where they stand.
    pub painted: &'a [(u32, u32)],
    /// The hidden sites' rounds in the air, as (aircraft aimed at, unit)
    /// pairs, ascending: a launch gives a hidden site away
    /// ([`GroundPicture::unmask`]).
    pub launched: &'a [(u32, u32)],
}

/// Which of the AI's lists hold a contact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channels {
    /// The A-G radar list.
    pub radar: bool,
    /// The FLIR (IR page) list.
    pub infrared: bool,
    /// The pilot's eye.
    pub visual: bool,
}

/// One ground contact the striker perceives now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundTrack {
    pub id: u32,
    pub side: Side,
    /// The object's class word (`sensors::track::class`).
    pub class: u16,
    pub position: Vector,
    pub velocity: Vector,
    /// Slant range from the striker, feet.
    pub distance_ft: f64,
    pub channels: Channels,
    pub aircraft: bool,
    /// Its radar or jammer is on.
    pub emitting: bool,
    pub destroyed: bool,
}

/// An object the striker has perceived since it was tasked. It stays known
/// at its last perceived place until it is destroyed or leaves the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeenGround {
    pub id: u32,
    pub side: Side,
    pub class: u16,
    pub position: Vector,
    pub last_seen_tick: u64,
    pub emitting: bool,
    pub defense: Option<DefenseFacts>,
}

/// Why an object is in the briefing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefedKind {
    /// An objective target.
    Target,
    /// One of the objective's air defences (a SEAD striker's).
    Defense,
    /// An ordered target.
    Ordered,
}

/// A briefed or ordered object's status. Its identity, place and whether it
/// stands are known from the briefing; `perceived` says whether the striker's
/// own lists hold it now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BriefedTarget {
    pub id: u32,
    pub kind: BriefedKind,
    pub side: Side,
    pub class: u16,
    pub position: Vector,
    pub standing: bool,
    pub perceived: bool,
    pub defense: Option<DefenseFacts>,
}

/// A SAM or AAA site (or a radar) the striker knows: briefed, or perceived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnownDefense {
    pub id: u32,
    pub side: Side,
    pub position: Vector,
    pub facts: DefenseFacts,
    pub briefed: bool,
}

impl KnownDefense {
    /// A missile defence covers `point`: it can guide what it fires and the
    /// point lies within its missiles' reach (docs/spec/ai-strike.md: a
    /// defence covers a point within its own maximum engagement range).
    pub fn covers_with_missiles(&self, point: Vector) -> bool {
        self.facts.working
            && self.facts.missile_reach_ft > 0.
            && distance(self.position, point) <= self.facts.missile_reach_ft
    }
    /// A gun defence covers `point`: within its guns' reach.
    pub fn covers_with_guns(&self, point: Vector) -> bool {
        self.facts.gun_reach_ft > 0. && distance(self.position, point) <= self.facts.gun_reach_ft
    }
    /// Retail's reaction to it for an aircraft at `own`.
    pub fn retail_attention(&self, own: Vector) -> RetailAttention {
        retail_attention(
            self.facts.ship,
            distance(self.position, own),
            off_nose_deg(self.position, self.facts.heading_deg, own),
        )
    }
}

/// A hostile fighter within 30 nm, from the striker's air picture at the
/// last rebuild.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FighterThreat {
    pub id: u32,
    pub side: Side,
    pub position: Vector,
    pub distance_ft: f64,
    /// The angle between the fighter's nose and the line from it to the
    /// striker, degrees (0 pointing at it, 180 tail on), horizontal.
    pub off_nose_deg: f64,
}

impl FighterThreat {
    pub fn retail_attention(&self) -> RetailAttention {
        retail_attention(false, self.distance_ft, self.off_nose_deg)
    }
}

/// Retail's only threat geometry (docs/formats/ai.md, "Threat perception").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetailAttention {
    Ignored,
    Evaded,
}

/// Retail's threat geometry: a ship threat is ignored beyond 40,000 ft and
/// any other beyond 30 statute miles; one inside 20,000 ft is always evaded;
/// between, it is evaded only while the aircraft is within 100 degrees of
/// the threat's nose. A distance exactly on a band edge takes the nearer
/// band.
pub fn retail_attention(ship: bool, distance_ft: f64, off_nose_deg: f64) -> RetailAttention {
    let limit = if ship {
        RETAIL_SHIP_IGNORE_FT
    } else {
        RETAIL_IGNORE_FT
    };
    if !distance_ft.is_finite() || distance_ft > limit {
        RetailAttention::Ignored
    } else if distance_ft <= RETAIL_ALWAYS_FT || off_nose_deg <= RETAIL_NOSE_DEG {
        RetailAttention::Evaded
    } else {
        RetailAttention::Ignored
    }
}

/// The horizontal angle at `threat` between its nose (heading, degrees,
/// 0 along +z, clockwise towards +x) and the line to `own`, degrees in
/// 0..=180. Directly above or on it counts as on the nose.
pub fn off_nose_deg(threat: Vector, heading_deg: f64, own: Vector) -> f64 {
    let dx = own[0] - threat[0];
    let dz = own[2] - threat[2];
    if dx == 0. && dz == 0. {
        return 0.;
    }
    let bearing = dx.atan2(dz).to_degrees();
    let off = (bearing - heading_deg).rem_euclid(360.);
    off.min(360. - off)
}

/// The striker's perceived threat level, highest first applies
/// (docs/spec/ai-strike.md, "Perceived threat and available speed").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreatLevel {
    Clear,
    /// A hostile fighter within 30 nm, or an air-to-air warning in the last
    /// 10 s.
    Air,
    /// A known AAA site covers a given point.
    Guns,
    /// A known working SAM site covers a given point, or a surface-to-air
    /// missile warning in the last 10 s.
    Missiles,
}

/// The warnings of the last 10 s, from the striker's own RWR and missile
/// warnings (the caller reads them from its threat service).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecentWarnings {
    pub air_to_air: bool,
    pub surface_to_air: bool,
}

/// The threat inputs, rebuilt with the picture every 6 s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreatPicture {
    /// Every known defence of every side, ascending id.
    pub defenses: Vec<KnownDefense>,
    /// Hostile fighters within 30 nm, nearest first.
    pub fighters: Vec<FighterThreat>,
}

impl ThreatPicture {
    /// The known defences of a side other than `own`.
    pub fn hostile_defenses(&self, own: Side) -> impl Iterator<Item = &KnownDefense> {
        self.defenses.iter().filter(move |d| d.side != own)
    }
    /// The perceived threat level against `points` (the target point and the
    /// planned release point; any number).
    pub fn level(&self, own: Side, points: &[Vector], warnings: RecentWarnings) -> ThreatLevel {
        let covers = |test: fn(&KnownDefense, Vector) -> bool| {
            self.hostile_defenses(own)
                .any(|d| points.iter().any(|p| test(d, *p)))
        };
        if warnings.surface_to_air || covers(KnownDefense::covers_with_missiles) {
            ThreatLevel::Missiles
        } else if covers(KnownDefense::covers_with_guns) {
            ThreatLevel::Guns
        } else if warnings.air_to_air || !self.fighters.is_empty() {
            ThreatLevel::Air
        } else {
            ThreatLevel::Clear
        }
    }
}

/// A hidden site the picture learned of from its launch, with nothing else
/// having shown it before: the tick of that refresh. The helicopter's ambush
/// rule reads it ([`crate::ai::helicopter_threat::rings_before_shot`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unmasked {
    pub id: u32,
    pub tick: u64,
}

/// What a refresh must do for one picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Rebuild,
    Follow,
}

/// A strike actor's ground picture. See the module comment.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundPicture {
    role: StrikeRole,
    /// `None` until the host briefs it (or with an order alone).
    objective: Option<StrikeObjective>,
    ordered: Option<u32>,
    /// The targets of a class order (a flight lead's Alt+W on a ground
    /// class, slice ST-O): briefed like an objective's targets, so the
    /// striker takes the nearest first and two to an object, but with no
    /// objective point, so nothing outside the class is taken as an
    /// opportunity. Ascending id.
    class_order: Vec<u32>,
    /// The tick of the last rebuild; `None` before the first.
    built: Option<u64>,
    /// The lists as last rebuilt and followed, nearest first.
    contacts: Vec<GroundTrack>,
    /// Ascending id.
    seen: Vec<SeenGround>,
    /// Objective targets, then defences, then the order, each ascending id.
    briefed: Vec<BriefedTarget>,
    threats: ThreatPicture,
    /// The hidden sites first known by their launch, ascending id, while
    /// they stand.
    unmasked: Vec<Unmasked>,
}

impl GroundPicture {
    pub fn new(role: StrikeRole) -> Self {
        Self {
            role,
            objective: None,
            ordered: None,
            class_order: Vec::new(),
            built: None,
            contacts: Vec::new(),
            seen: Vec::new(),
            briefed: Vec::new(),
            threats: ThreatPicture::default(),
            unmasked: Vec::new(),
        }
    }
    pub fn role(&self) -> StrikeRole {
        self.role
    }
    pub fn set_role(&mut self, role: StrikeRole) {
        self.role = role;
    }
    /// The briefing; `None` until briefed.
    pub fn objective(&self) -> Option<&StrikeObjective> {
        self.objective.as_ref()
    }
    /// Briefs the striker. Its status list is rebuilt at the next refresh.
    pub fn set_objective(&mut self, objective: StrikeObjective) {
        self.objective = Some(objective);
        self.built = None;
    }
    /// The ordered ground target, if any.
    pub fn ordered(&self) -> Option<u32> {
        self.ordered
    }
    /// Orders a ground target, or clears the order. The picture rebuilds at
    /// the next refresh.
    pub fn set_ordered(&mut self, target: Option<u32>) {
        if self.ordered != target {
            self.ordered = target;
            self.built = None;
        }
    }
    /// The class order's targets, if one stands.
    pub fn class_order(&self) -> &[u32] {
        &self.class_order
    }
    /// Gives a class order its targets (ascending id, any repeat dropped), or
    /// clears it with none. The picture rebuilds at the next refresh.
    pub fn set_class_order(&mut self, mut targets: Vec<u32>) {
        targets.sort_unstable();
        targets.dedup();
        if self.class_order != targets {
            self.class_order = targets;
            self.built = None;
        }
    }
    /// Whether the picture has anything to do: the Strike role, or an order
    /// (one target or a class).
    pub fn active(&self) -> bool {
        self.role == StrikeRole::Strike || self.ordered.is_some() || !self.class_order.is_empty()
    }
    /// The tick of the last rebuild.
    pub fn built_tick(&self) -> Option<u64> {
        self.built
    }
    /// The perceived ground contacts, nearest first.
    pub fn contacts(&self) -> &[GroundTrack] {
        &self.contacts
    }
    pub fn contact(&self, id: u32) -> Option<&GroundTrack> {
        self.contacts.iter().find(|c| c.id == id)
    }
    /// Everything perceived since tasking and still standing, ascending id.
    pub fn seen(&self) -> &[SeenGround] {
        &self.seen
    }
    /// The briefed and ordered objects' status.
    pub fn briefed(&self) -> &[BriefedTarget] {
        &self.briefed
    }
    pub fn threats(&self) -> &ThreatPicture {
        &self.threats
    }
    /// When a hidden site that nothing else had shown became known by its
    /// launch (the refresh's tick), while it stands.
    pub fn unmasked_at(&self, id: u32) -> Option<u64> {
        self.unmasked
            .binary_search_by_key(&id, |u| u.id)
            .ok()
            .map(|at| self.unmasked[at].tick)
    }

    /// What a refresh at `tick` must do: rebuild every 6 s (and when never
    /// built), follow every quarter second between.
    pub fn due(&self, tick: u64) -> Option<Refresh> {
        match self.built {
            None => Some(Refresh::Rebuild),
            Some(built) if tick.saturating_sub(built) >= GROUND_REBUILD_TICKS => {
                Some(Refresh::Rebuild)
            }
            Some(built)
                if tick > built
                    && tick
                        .saturating_sub(built)
                        .is_multiple_of(GROUND_FOLLOW_TICKS) =>
            {
                Some(Refresh::Follow)
            }
            Some(_) => None,
        }
    }

    /// Replaces the lists with a fresh rebuild at `tick` and rebuilds the
    /// memory, the briefing status and the threat inputs.
    pub(crate) fn rebuild(
        &mut self,
        tick: u64,
        tracks: Vec<GroundTrack>,
        subjects: &Subjects<'_>,
        fighters: Vec<FighterThreat>,
    ) {
        self.built = Some(tick);
        self.contacts = tracks;
        self.remember(tick, subjects);
        self.brief(subjects);
        let mut defenses: Vec<KnownDefense> = self
            .briefed
            .iter()
            .filter_map(|b| {
                Some(KnownDefense {
                    id: b.id,
                    side: b.side,
                    position: b.position,
                    facts: b.defense?,
                    briefed: true,
                })
                .filter(|_| b.standing)
            })
            .collect();
        for seen in &self.seen {
            if let Some(facts) = seen.defense
                && !defenses.iter().any(|d| d.id == seen.id)
            {
                defenses.push(KnownDefense {
                    id: seen.id,
                    side: seen.side,
                    position: seen.position,
                    facts,
                    briefed: false,
                });
            }
        }
        defenses.sort_by_key(|d| d.id);
        self.threats = ThreatPicture { defenses, fighters };
    }

    /// A radar painting the aircraft, and the launchers it guides for, are
    /// known from now on as if seen (slice H4, John's rule: a SAM or AAA
    /// radar painting a helicopter makes it aware of the threat). Only
    /// standing defences are taken.
    pub(crate) fn reveal(&mut self, tick: u64, ids: &[u32], subjects: &Subjects<'_>) {
        for id in ids {
            self.know(tick, *id, subjects);
        }
    }

    /// A hidden site that launched at the aircraft, or at a friendly it
    /// sees, is known from now on as if seen (T.O.R.E opinionated change
    /// requested by John, 2026-10-29), so any later shot comes from a known
    /// site. When nothing had shown it before, the tick is kept
    /// ([`Self::unmasked_at`]): the shot that gave it away still came from a
    /// site the aircraft did not know.
    pub(crate) fn unmask(&mut self, tick: u64, ids: &[u32], subjects: &Subjects<'_>) {
        for id in ids {
            let known = self.seen.binary_search_by_key(id, |s| s.id).is_ok()
                || self
                    .threats
                    .defenses
                    .binary_search_by_key(id, |d| d.id)
                    .is_ok();
            if self.know(tick, *id, subjects)
                && !known
                && let Err(at) = self.unmasked.binary_search_by_key(id, |u| u.id)
            {
                self.unmasked.insert(at, Unmasked { id: *id, tick });
            }
        }
    }

    /// Puts a standing defence into the memory and the threat inputs at
    /// once. False when it is no standing defence.
    fn know(&mut self, tick: u64, id: u32, subjects: &Subjects<'_>) -> bool {
        let Some(subject) = subjects.get(id) else {
            return false;
        };
        let Some(facts) = subject.defense else {
            return false;
        };
        if subject.object.destroyed {
            return false;
        }
        let entry = SeenGround {
            id,
            side: subject.side,
            class: subject.object.class,
            position: subject.object.position,
            last_seen_tick: tick,
            emitting: subject.object.emitting,
            defense: Some(facts),
        };
        match self.seen.binary_search_by_key(&id, |s| s.id) {
            Ok(at) => self.seen[at] = entry,
            Err(at) => self.seen.insert(at, entry),
        }
        if let Err(at) = self.threats.defenses.binary_search_by_key(&id, |d| d.id) {
            self.threats.defenses.insert(
                at,
                KnownDefense {
                    id,
                    side: subject.side,
                    position: subject.object.position,
                    facts,
                    briefed: false,
                },
            );
        }
        true
    }

    /// Between rebuilds: each listed contact follows its object (a gone one
    /// leaves), `visible` says whether the eye still holds it, and the
    /// briefing status and memory follow too.
    pub(crate) fn follow(
        &mut self,
        tick: u64,
        own: Vector,
        subjects: &Subjects<'_>,
        visible: &dyn Fn(&GroundObject) -> bool,
    ) {
        self.contacts.retain_mut(|track| {
            let Some(subject) = subjects.get(track.id) else {
                return false;
            };
            let object = &subject.object;
            track.position = object.position;
            track.velocity = object.velocity;
            track.distance_ft = distance(own, object.position);
            track.destroyed = object.destroyed;
            track.emitting = object.emitting;
            track.channels.visual = visible(object);
            true
        });
        self.remember(tick, subjects);
        self.brief(subjects);
    }

    /// Adds every standing contact to the memory and forgets what is
    /// destroyed or gone.
    fn remember(&mut self, tick: u64, subjects: &Subjects<'_>) {
        self.seen.retain(|seen| {
            subjects
                .get(seen.id)
                .is_some_and(|subject| !subject.object.destroyed)
        });
        let seen = &self.seen;
        self.unmasked
            .retain(|u| seen.binary_search_by_key(&u.id, |s| s.id).is_ok());
        for track in self.contacts.iter().filter(|t| !t.destroyed) {
            let defense = subjects.get(track.id).and_then(|s| s.defense);
            let entry = SeenGround {
                id: track.id,
                side: track.side,
                class: track.class,
                position: track.position,
                last_seen_tick: tick,
                emitting: track.emitting,
                defense,
            };
            match self.seen.binary_search_by_key(&track.id, |s| s.id) {
                Ok(at) => self.seen[at] = entry,
                Err(at) => self.seen.insert(at, entry),
            }
        }
    }

    /// The briefing's status from the host's description of the objects.
    fn brief(&mut self, subjects: &Subjects<'_>) {
        let mut named: Vec<(u32, BriefedKind)> = Vec::new();
        if let Some(objective) = &self.objective {
            named.extend(
                objective
                    .targets
                    .iter()
                    .map(|id| (*id, BriefedKind::Target)),
            );
            named.extend(
                objective
                    .defenses
                    .iter()
                    .map(|id| (*id, BriefedKind::Defense)),
            );
        }
        for id in &self.class_order {
            if !named.iter().any(|(named, _)| named == id) {
                named.push((*id, BriefedKind::Target));
            }
        }
        named.extend(self.ordered.map(|id| (id, BriefedKind::Ordered)));
        self.briefed = named
            .into_iter()
            .map(|(id, kind)| {
                let subject = subjects.get(id);
                let previous = self.briefed.iter().find(|b| b.id == id);
                let perceived = self.contacts.iter().any(|t| t.id == id && !t.destroyed);
                match subject {
                    Some(subject) => BriefedTarget {
                        id,
                        kind,
                        side: subject.side,
                        class: subject.object.class,
                        position: subject.object.position,
                        standing: !subject.object.destroyed,
                        perceived,
                        defense: subject.defense,
                    },
                    // Gone from the world: no longer standing, last place kept.
                    None => BriefedTarget {
                        id,
                        kind,
                        side: previous.map_or(Side(0), |b| b.side),
                        class: previous.map_or(0, |b| b.class),
                        position: previous.map_or([0.; 3], |b| b.position),
                        standing: false,
                        perceived: false,
                        defense: previous.and_then(|b| b.defense),
                    },
                }
            })
            .collect();
    }

    /// The strike selector's candidates
    /// ([`crate::ai::targeting::select_strike_target`]): the ordered target,
    /// the briefed targets (and a SEAD striker's briefed defences), then the
    /// hostile objects seen near the objective. `attackers` counts the other
    /// wing members attacking an object; `usable` says whether a carried
    /// store can attack an object of a class word.
    pub fn strike_candidates(
        &self,
        own: Vector,
        own_side: Side,
        attackers: &dyn Fn(u32) -> u32,
        usable: &dyn Fn(u16) -> bool,
    ) -> Vec<StrikeCandidate> {
        let mut candidates: Vec<StrikeCandidate> = self
            .briefed
            .iter()
            .map(|b| StrikeCandidate {
                id: ObjectId(b.id),
                side: b.side,
                source: if b.kind == BriefedKind::Ordered {
                    StrikeSource::Ordered
                } else {
                    StrikeSource::Briefed
                },
                valid: b.standing,
                usable: usable(b.class),
                spatial_distance_feet: distance(own, b.position),
                wing_attackers: attackers(b.id),
                near_objective: false,
            })
            .collect();
        let point = self.objective.as_ref().and_then(|o| o.point);
        for seen in self
            .seen
            .iter()
            .filter(|s| s.side != own_side && !self.briefed.iter().any(|b| b.id == s.id))
        {
            candidates.push(StrikeCandidate {
                id: ObjectId(seen.id),
                side: seen.side,
                source: StrikeSource::Opportunity,
                valid: true,
                usable: usable(seen.class),
                spatial_distance_feet: distance(own, seen.position),
                wing_attackers: attackers(seen.id),
                near_objective: point
                    .is_some_and(|p| horizontal(p, seen.position) <= OPPORTUNITY_RADIUS_FT),
            });
        }
        candidates
    }
}

/// The host's subjects of one refresh, indexed by id.
pub struct Subjects<'a> {
    subjects: &'a [GroundSubject],
    by_id: std::collections::BTreeMap<u32, usize>,
}

impl<'a> Subjects<'a> {
    pub fn new(subjects: &'a [GroundSubject]) -> Self {
        Self {
            subjects,
            by_id: subjects
                .iter()
                .enumerate()
                .map(|(at, s)| (s.object.id, at))
                .collect(),
        }
    }
    pub fn get(&self, id: u32) -> Option<&'a GroundSubject> {
        self.by_id.get(&id).map(|at| &self.subjects[*at])
    }
    pub fn all(&self) -> &'a [GroundSubject] {
        self.subjects
    }
}

pub(crate) fn distance(a: Vector, b: Vector) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn horizontal(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

#[path = "ground_checkpoint.rs"]
mod checkpoint;
