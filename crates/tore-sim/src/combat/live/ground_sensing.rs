//! What the player's ground pages read of the targets (docs/spec/air-to-ground.md,
//! "Ground contacts and channels"): each row's class word, its type's listing
//! flag and visibility distance, its signatures, whether it emits, whether an
//! aircraft is in an airfield sequence, and the weather's sensor effects. The
//! host registers the type facts and the weather; the sensors decide what a
//! page lists ([`sensors::Sensors::step_ground`]).
use super::{Ownship, ParkedSite, State, Target, TargetRole};
use crate::sensors::{self, GroundObject, Volume, track::LISTED_FLAG, track::class};
use std::collections::{BTreeMap, BTreeSet};

/// The type facts a ground page reads of one ground object: its OBJECT
/// `flags` and `maxVisDist` (docs/formats/radar.md, "A-G page").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroundListing {
    pub flags: u32,
    /// Raw, in units of 256 ft.
    pub max_visible: u16,
    /// A hidden shoulder-fired SAM team (MANPADS; the host sets it for a team
    /// that is no mission target): off every pilot's sensor pages and map
    /// until that pilot spots it (T.O.R.E opinionated, John 2026-10-29;
    /// docs/spec/air-to-ground.md, "Hidden shoulder-fired teams").
    pub team: bool,
}

impl State {
    /// Registers the type facts of ground object `id`. Fails for an id that is
    /// not a ground object.
    pub fn set_ground_listing(&mut self, id: u32, listing: GroundListing) -> bool {
        if !self.ground_bounds.contains_key(&id) {
            return false;
        }
        self.ground_listing.insert(id, listing);
        true
    }
    /// The type facts of ground object `id`, if registered.
    pub fn ground_listing(&self, id: u32) -> Option<GroundListing> {
        self.ground_listing.get(&id).copied()
    }
    /// Whether aircraft `id` is in an airfield's takeoff or landing sequence:
    /// the A-G page lists aircraft only then. The host says so each tick.
    pub fn set_airfield_attached(&mut self, id: u32, attached: bool) {
        if attached {
            self.airfield.insert(id);
        } else {
            self.airfield.remove(&id);
        }
    }
    pub fn airfield_attached(&self, id: u32) -> bool {
        self.airfield.contains(&id)
    }
    /// The active weather layers' sensor effects, from the host's weather.
    pub fn set_sensor_weather(&mut self, weather: sensors::Weather) {
        if self.sensor_weather != weather {
            self.sensor_weather = weather;
        }
    }
    /// [`Self::set_sensor_weather`] for bands the caller has not collected:
    /// they are collected only when they differ from the ones held, which is
    /// almost never (the active layers change a few times a mission).
    pub fn set_sensor_weather_bands(
        &mut self,
        bands: impl ExactSizeIterator<Item = sensors::WeatherBand> + Clone,
    ) {
        let held = &self.sensor_weather.bands;
        if bands.len() != held.len() || bands.clone().zip(held).any(|(new, old)| new != *old) {
            self.sensor_weather = sensors::Weather {
                bands: bands.collect(),
            };
        }
    }
    pub fn sensor_weather(&self) -> &sensors::Weather {
        &self.sensor_weather
    }
    /// The highest the host's step `ground` reaches over each square, so a
    /// round's or a missile's sight line wholly above it is not sampled.
    /// Host setup, not state: never checkpointed. It must describe the
    /// `ground` every step is given, or be `None`.
    pub fn set_ground_ceiling(
        &mut self,
        ceiling: Option<std::sync::Arc<crate::ground_ceiling::GroundCeiling>>,
    ) {
        self.ground_ceiling = ceiling;
    }
    /// Ctrl+X: the nearest standing ground object (a surface row: a unit, a
    /// structure, a ship or a parked aircraft) of another side than `own`'s,
    /// neutral scenery excluded.
    pub(super) fn nearest_ground(
        &self,
        own: &Ownship,
        from: crate::attitude::Vector,
    ) -> Option<u32> {
        self.targets
            .iter()
            .filter(|t| {
                t.role == TargetRole::Surface
                    && t.hp > 0
                    && t.side != super::NO_SIDE
                    && t.side != own.side
                    && !own.friendlies.contains(&t.id)
                    && !self.hidden_from(own, t.id)
            })
            .map(|t| {
                let d: f64 = (0..3).map(|i| (t.position[i] - from[i]).powi(2)).sum();
                (d, t.id)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }
    /// Whether ground object `id` is a hidden shoulder-fired team that `own`'s
    /// pilot has not spotted: Easy targeting's Ctrl+X passes it by.
    pub(super) fn hidden_from(&self, own: &Ownship, id: u32) -> bool {
        self.ground_listing.get(&id).is_some_and(|l| l.team) && !own.spotted_teams.contains(&id)
    }
    /// What a ground page reads of target row `t`, for a reader outside the
    /// player's pages (the AI's ground picture, docs/spec/ai-strike.md).
    pub fn ground_object(&self, t: &Target) -> GroundObject {
        self.ground_facts().ground_object(t)
    }
    /// The facts the ground pages read, borrowed for one observation pass.
    pub(super) fn ground_facts(&self) -> GroundFacts<'_> {
        GroundFacts {
            listing: &self.ground_listing,
            parked: &self.parked,
            airfield: &self.airfield,
            weather: &self.sensor_weather,
        }
    }
}

/// The state's read-only facts for the ground pages, without its
/// projectiles, ownships or random stream, so observations on workers can
/// read them.
pub(super) struct GroundFacts<'a> {
    listing: &'a BTreeMap<u32, GroundListing>,
    parked: &'a BTreeMap<u32, ParkedSite>,
    airfield: &'a BTreeSet<u32>,
    pub weather: &'a sensors::Weather,
}
impl GroundFacts<'_> {
    /// Whether ground object `id` is a hidden shoulder-fired team
    /// ([`GroundListing::team`]).
    pub(super) fn team(&self, id: u32) -> bool {
        self.listing.get(&id).is_some_and(|l| l.team)
    }
    /// The listing distance of ground object `id`, feet (0: no limit).
    pub(super) fn listing_ft(&self, id: u32) -> f64 {
        self.listing.get(&id).map_or(0., |l| {
            f64::from(l.max_visible) * sensors::track::VISIBILITY_UNIT_FT
        })
    }
    /// What a ground page reads of target row `t`.
    pub(super) fn ground_object(&self, t: &Target) -> GroundObject {
        let listing = self.listing.get(&t.id);
        let parked = self.parked.contains_key(&t.id);
        GroundObject {
            id: t.id,
            position: t.position,
            velocity: t.velocity,
            class: t.category,
            // A parked aircraft has no OT record: listed (fitted, retail
            // aircraft types carry the flag).
            listed: parked || listing.is_some_and(|l| l.flags & LISTED_FLAG != 0),
            max_visible: listing.map_or(0, |l| l.max_visible),
            radar: t.signature.loaded_radar(t.configuration),
            infrared: t.signature.infrared,
            emitting: t.radar_emitting || t.jammer_active,
            aircraft: t.role == TargetRole::Aircraft
                || t.aircraft.is_some()
                || t.category & class::AIRCRAFT != 0,
            airborne: t.airborne,
            airfield: self.airfield.contains(&t.id),
            destroyed: t.hp <= 0,
        }
    }
}

/// The seeker the HARM page uses: of the anti-radiation (signature 4)
/// weapons aboard with rounds left on a working station, the one with the
/// shortest search range (retail `HARDBestSeekers`: with an AGM-45 and an
/// AGM-88 aboard, the AGM-45's).
pub(super) fn harm_seeker(own: &Ownship) -> Option<Volume> {
    own.config
        .stations
        .iter()
        .enumerate()
        .filter(|(index, station)| {
            station.weapon.seeker.signature == 4
                && own.rounds(*index) > 0
                && own.ammo.get(*index).is_some_and(|ammo| ammo & 0x8000 == 0)
        })
        .filter_map(|(_, station)| Volume::from_zone(&station.weapon.seeker.zones[0]).ok())
        .min_by(|a, b| a.maximum_ft.total_cmp(&b.maximum_ft))
}

// -- Hidden shoulder-fired teams (T.O.R.E opinionated, John 2026-10-29) ------
//
// A pilot's sensors list a hidden team (`GroundListing::team`) only once that
// pilot has spotted it, as the AI's do (docs/spec/air-to-ground.md, "Hidden
// shoulder-fired teams"): by eye within the best AI pilot's spotting distance,
// or by seeing it launch at the aircraft or at a friendly the pilot sees. The
// world still draws the team: the player's own eyes are the eyes.

/// How near the player's eye must be to spot a hidden team in clear air,
/// feet: the Ace's (fitted, agent, 2026-10-29: a human is at least as sharp
/// as the best AI pilot, and the pages need the spotting to be a rule).
pub fn player_team_spot_ft(visual_percent: f64) -> f64 {
    crate::ai::awareness::ground::team_spot_ft(crate::ai::Experience::Ace, visual_percent)
}

/// Spots, by eye, the hidden teams `own`'s pilot sees within
/// [`player_team_spot_ft`] (scaled by the weather's visual effect between
/// them): inside the visual sensor's volume with a clear line; and a team
/// the targeting sight's track holds.
pub(super) fn spot_teams(
    own: &mut Ownship,
    observer: &sensors::Observer,
    targets: &[Target],
    facts: &GroundFacts<'_>,
    environment: &sensors::Environment<'_>,
) {
    let found: Vec<u32> = targets
        .iter()
        .filter(|t| t.hp > 0 && facts.team(t.id) && !own.spotted_teams.contains(&t.id))
        .filter(|t| {
            let visual = facts.weather.percent(
                sensors::track::selector::VISUAL,
                observer.position[1],
                t.position[1],
            );
            let distance = (0..3)
                .map(|i| (t.position[i] - observer.position[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            distance <= player_team_spot_ft(visual)
                && own
                    .sensors
                    .sees_ground(observer, &facts.ground_object(t), environment)
        })
        .map(|t| t.id)
        .collect();
    own.spotted_teams.extend(found);
    // A team the targeting sight's track holds is in the pilot's view
    // through the sight's optics.
    if let Some(id) = own.sensors.held().filter(|id| facts.team(*id)) {
        own.spotted_teams.insert(id);
    }
}

/// The hidden teams `own`'s pilot has not spotted, ascending id: its sensors
/// leave them out.
pub(super) fn unspotted_teams(
    own: &Ownship,
    targets: &[Target],
    facts: &GroundFacts<'_>,
) -> Vec<u32> {
    let mut ids: Vec<u32> = targets
        .iter()
        .filter(|t| facts.team(t.id) && !own.spotted_teams.contains(&t.id))
        .map(|t| t.id)
        .collect();
    ids.sort_unstable();
    ids
}

impl State {
    /// A hidden team with a round in the air at `own`'s aircraft, or at a
    /// friendly aircraft its sensors observe now while the team stands
    /// within its own listing distance of `at`, is spotted (the AI's rule,
    /// docs/spec/ai-strike.md, "What the AI knows").
    pub(super) fn spot_team_launches(&self, own: &mut Ownship, at: crate::attitude::Vector) {
        let facts = self.ground_facts();
        for p in self.projectiles.iter().filter(|p| p.gun_round.is_none()) {
            let Some(aimed) = p.target else {
                continue;
            };
            if !facts.team(p.owner) || own.spotted_teams.contains(&p.owner) {
                continue;
            }
            let seen = aimed == own.aircraft
                || (self.friendly_to(own, aimed)
                    && own.sensors.observation(aimed).is_some()
                    && self
                        .targets
                        .iter()
                        .find(|t| t.id == p.owner)
                        .is_some_and(|team| {
                            let reach = facts.listing_ft(p.owner);
                            reach == 0.
                                || (0..3)
                                    .map(|i| (team.position[i] - at[i]).powi(2))
                                    .sum::<f64>()
                                    .sqrt()
                                    <= reach
                        }));
            if seen {
                own.spotted_teams.insert(p.owner);
            }
        }
    }

    /// Whether aircraft `id` flies for `own`'s side.
    fn friendly_to(&self, own: &Ownship, id: u32) -> bool {
        own.friendlies.contains(&id)
            || self
                .targets
                .iter()
                .chain(&self.ownship_rows)
                .any(|t| t.id == id && t.side == own.side)
    }

    /// The hidden teams `own`'s pilot has spotted, ascending id.
    pub fn spotted_teams(&self, aircraft: u32) -> Vec<u32> {
        self.ownships
            .iter()
            .find(|own| own.aircraft == aircraft)
            .map_or_else(Vec::new, |own| own.spotted_teams.iter().copied().collect())
    }
}
