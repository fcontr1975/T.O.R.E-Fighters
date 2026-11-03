//! Mission role and rules-of-engagement filtering for current observations.
//!
//! This module never discovers a target. Callers supply the actor's current
//! [`TargetView`] observations and reports containing only independently known
//! attacker identities. Assignment metadata classifies those observations but
//! cannot turn an unobserved object into a target.

use super::{controller::TargetView, targeting};

const FEET_PER_NAUTICAL_MILE: f64 = 6_076.;
pub const ESCORT_EXIT_LEASH_FT: f64 = 10. * FEET_PER_NAUTICAL_MILE;
pub const ESCORT_REENTRY_LEASH_FT: f64 = 8. * FEET_PER_NAUTICAL_MILE;
pub const ESCORT_ASSESSMENT_RADIUS_FT: f64 = 30. * FEET_PER_NAUTICAL_MILE;
pub const ESCORT_DEFENSE_RADIUS_FT: f64 = 10. * FEET_PER_NAUTICAL_MILE;
pub const ESCORT_ASSESSMENT_TIME_S: f64 = 60.;
pub const SUPPORTING_RADAR_IDENTIFICATION_TOLERANCE_DEG: f64 = 2.;

/// Group-level Quick Mission objective before aircraft IDs are assigned.
/// Resolving a group only assigns duties; it never grants sensor knowledge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupObjective {
    #[default]
    Inherit,
    Free,
    Cap,
    Intercept(super::launch::WingId),
    Escort(super::launch::WingId),
    SelfDefense,
    Hold,
    /// Strike the mission's ground target (slice ST-O): the wing's air
    /// orders are the mission's own (as for `Inherit`) and its aircraft take
    /// the [`StrikeRole::Strike`] role. Offered for a friendly wing with a
    /// ground target set.
    Strike,
}

/// A wing's air-to-ground role (docs/spec/ai-strike.md, "Who strikes").
/// **CAP** is every wing's behaviour before the strike round and stays the
/// default, so nothing changes unless a mission asks for **Strike**. Side
/// neutral: any side's wing may strike (opinionated, John, 2026-10-11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrikeRole {
    #[default]
    Cap,
    Strike,
}

/// A striker whose briefed targets are gone attacks hostile ground objects it
/// has seen within this distance of the objective (docs/spec/ai-strike.md,
/// "What the AI knows": 10 nm, fitted).
pub const OPPORTUNITY_RADIUS_FT: f64 = 10. * FEET_PER_NAUTICAL_MILE;
/// The objective's air defences a SEAD striker is briefed on: hostile armed
/// units and radars within this distance of the objective point (5 nm, the
/// distance Quick Mission's Red defends its target within; fitted).
pub const SEAD_BRIEFING_RADIUS_FT: f64 = 5. * FEET_PER_NAUTICAL_MILE;

/// What a strike actor's briefing names: its **Strike objective**
/// (docs/spec/ai-strike.md, "Who strikes" and "What the AI knows"). Briefed
/// objects are known from the start, without a sensor: this is the briefing.
/// Anything else must be seen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrikeObjective {
    /// The targets the mission names: Quick Mission's ground objective
    /// objects (flagged 0x80) on another side, or a mission's own orders.
    /// Ascending id.
    pub targets: Vec<u32>,
    /// The objective's air defences, for a SEAD striker (an anti-radiation
    /// store aboard); empty otherwise. Ascending id.
    pub defenses: Vec<u32>,
    /// Where the objective is: the centre of its targets as briefed, feet.
    /// `None` with no targets (an order alone).
    pub point: Option<[f64; 3]>,
}

impl StrikeObjective {
    /// Whether `id` is one of the briefed targets or defences.
    pub fn names(&self, id: u32) -> bool {
        self.targets.contains(&id) || self.defenses.contains(&id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    FreeEngagement,
    CombatAirPatrol,
    Intercept,
    Escort,
    Disengage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    WeaponsHold,
    SelfDefense,
    ProtectAssigned,
    EngageAssigned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostileEscort {
    /// The hostile principal this aircraft is known to escort.
    pub principal_id: u32,
    pub escort_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatrolRegion {
    /// Optional map rectangle: the patrol is its intersection with the circle.
    pub clip: Option<[[f64; 2]; 2]>,
    pub center_ft: [f64; 3],
    pub radius_ft: f64,
}

impl PatrolRegion {
    pub fn contains(self, position: [f64; 3]) -> bool {
        let dx = position[0] - self.center_ft[0];
        let dz = position[2] - self.center_ft[2];
        self.clip.is_none_or(|[min, max]| {
            (min[0]..=max[0]).contains(&position[0]) && (min[1]..=max[1]).contains(&position[2])
        }) && self.radius_ft.is_finite()
            && self.radius_ft >= 0.
            && dx.mul_add(dx, dz * dz) <= self.radius_ft * self.radius_ft
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub role: Role,
    pub stance: Stance,
    pub protected_ids: Vec<u32>,
    pub destroy_ids: Vec<u32>,
    /// Relationships are mission metadata. They affect priority only after the
    /// escort itself is present in the current observation list.
    pub hostile_escorts: Vec<HostileEscort>,
    pub patrol: Option<PatrolRegion>,
}

impl Assignment {
    /// `new` continues the lineage of the planes `lineage` (a respawn or a
    /// revival in a multiplayer game; the lobby pass's follow-up F1): every
    /// list that names a plane of the lineage names `new` too, and every
    /// escort relationship with a plane of it gains one with `new` in its
    /// place. The lists keep their order, `new` added at the end once.
    /// Returns whether anything changed.
    pub fn join_lineage(&mut self, new: u32, lineage: &[u32]) -> bool {
        let named = |ids: &[u32]| ids.iter().any(|id| lineage.contains(id)) && !ids.contains(&new);
        let mut changed = false;
        for ids in [&mut self.destroy_ids, &mut self.protected_ids] {
            if named(ids) {
                ids.push(new);
                changed = true;
            }
        }
        let mut gained = Vec::new();
        for relation in &self.hostile_escorts {
            if lineage.contains(&relation.principal_id) {
                gained.push(HostileEscort {
                    principal_id: new,
                    escort_id: relation.escort_id,
                });
            }
            if lineage.contains(&relation.escort_id) {
                gained.push(HostileEscort {
                    principal_id: relation.principal_id,
                    escort_id: new,
                });
            }
        }
        for relation in gained {
            if !self.hostile_escorts.contains(&relation) {
                self.hostile_escorts.push(relation);
                changed = true;
            }
        }
        changed
    }
}

impl Default for Assignment {
    fn default() -> Self {
        Self {
            role: Role::FreeEngagement,
            stance: Stance::EngageAssigned,
            protected_ids: Vec::new(),
            destroy_ids: Vec::new(),
            hostile_escorts: Vec::new(),
            patrol: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProtectedView {
    /// Mission-logistics position for an explicitly assigned friendly. Callers
    /// must not populate this from an enemy world object or private AI state.
    pub id: u32,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub alive: bool,
}

/// A perceived attack report. `None` means the attack is known but its attacker
/// is not independently identified, so it cannot authorize an aircraft target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreatReport {
    pub attacker_id: Option<u32>,
    pub defended_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    OwnDefense,
    ProtectedThreat,
    HostileEscort,
    ApproachingThreat,
    Assigned,
    Free,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub id: u32,
    pub priority: Priority,
}

/// The most observed aircraft one [`Explanation`] lists.
pub const EXPLANATION_LIMIT: usize = 16;

/// Why an observed aircraft could not be a mission target at all. Every
/// reason that applies is set; none set means it was eligible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ineligibility {
    pub own: bool,
    pub same_side: bool,
    /// Destroyed or otherwise no longer valid.
    pub invalid: bool,
    pub not_aircraft: bool,
    pub type_not_allowed: bool,
    /// No carried store can engage it.
    pub no_usable_weapon: bool,
}

impl Ineligibility {
    fn of(own_id: u32, own_side: targeting::Side, target: &TargetView) -> Self {
        Self {
            own: target.id == own_id,
            same_side: target.side == own_side,
            invalid: !target.valid,
            not_aircraft: !target.is_aircraft,
            type_not_allowed: !target.type_allowed,
            no_usable_weapon: !target.seeker_eligible,
        }
    }

    /// Whether any reason applies.
    pub fn any(&self) -> bool {
        self.own
            || self.same_side
            || self.invalid
            || self.not_aircraft
            || self.type_not_allowed
            || self.no_usable_weapon
    }
}

/// One observed aircraft in an [`Explanation`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateExplanation {
    pub id: u32,
    pub ineligible: Ineligibility,
    /// Its mission priority; `None` when it was ineligible, weapons were
    /// held, or the assignment gives it none.
    pub priority: Option<Priority>,
    /// B41 score parts: the distance, plus 10,000 ft for each penalty.
    pub distance_ft: f64,
    pub not_aircraft_penalty: bool,
    /// A wing member already attacks it.
    pub wing_attacking_penalty: bool,
    /// Its wing attackers already fill the allowance.
    pub wing_full_penalty: bool,
    /// The B41 score, feet; the lowest wins among the best priority.
    pub score_ft: f64,
    pub chosen: bool,
}

/// Why the engagement policy chose what it did on one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Explanation {
    /// Weapons hold: no target at all.
    pub weapons_hold: bool,
    /// Self-defense stance, disengage role or an escort beyond its leash:
    /// only the aircraft's own attackers can be targets.
    pub own_defense_only: bool,
    pub escort_outside_leash: bool,
    /// Identified attackers of this aircraft.
    pub own_attackers: Vec<u32>,
    /// Identified attackers of the aircraft it protects.
    pub protected_attackers: Vec<u32>,
    pub best_priority: Option<Priority>,
    /// The current target shares the best priority, so it is kept.
    pub kept_current: bool,
    pub chosen: Option<Selection>,
    /// Observed aircraft, best first.
    pub candidates: Vec<CandidateExplanation>,
    /// Observed aircraft left out because the list was full.
    pub omitted: usize,
}

/// Per-aircraft policy state. The only persistence is escort-leash hysteresis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    escort_outside_leash: bool,
}

impl Policy {
    pub fn reset(&mut self) {
        self.escort_outside_leash = false;
    }

    pub fn escort_outside_leash(&self) -> bool {
        self.escort_outside_leash
    }

    pub fn must_rejoin(&self) -> bool {
        self.escort_outside_leash
    }

    /// Whether a frozen observation may guide a search. This never admits the
    /// record to combat selection or weapon employment.
    pub fn allows_investigation(
        &self,
        assignment: &Assignment,
        remembered: &TargetView,
        protected: &[ProtectedView],
        reports: &[ThreatReport],
        own_id: u32,
    ) -> bool {
        if assignment.stance == Stance::WeaponsHold
            || remembered.id == own_id
            || !remembered.valid
            || !remembered.is_aircraft
        {
            return false;
        }

        let own_attackers = known_attackers(reports, own_id);
        if own_attackers.contains(&remembered.id) {
            return true;
        }
        if assignment.stance == Stance::SelfDefense
            || assignment.role == Role::Disengage
            || self.escort_outside_leash
        {
            return false;
        }

        let protected_attackers: Vec<_> = reports
            .iter()
            .filter(|report| assignment.protected_ids.contains(&report.defended_id))
            .filter_map(|report| report.attacker_id)
            .collect();
        self.mission_priority(assignment, remembered, protected, &protected_attackers)
            .is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn select(
        &mut self,
        own_id: u32,
        own_side: targeting::Side,
        own_position: [f64; 3],
        assignment: &Assignment,
        targets: &[TargetView],
        protected: &[ProtectedView],
        reports: &[ThreatReport],
        current_target: Option<u32>,
        assignment_allowance: u32,
    ) -> Option<Selection> {
        self.update_leash(own_position, assignment, protected);
        if assignment.stance == Stance::WeaponsHold {
            return None;
        }

        let own_attackers = known_attackers(reports, own_id);
        let protected_attackers: Vec<u32> = reports
            .iter()
            .filter(|report| assignment.protected_ids.contains(&report.defended_id))
            .filter_map(|report| report.attacker_id)
            .collect();

        let mut candidates = Vec::new();
        for target in targets {
            if !eligible(own_id, own_side, target) {
                continue;
            }
            let priority = self.target_priority(
                assignment,
                target,
                protected,
                &own_attackers,
                &protected_attackers,
            );
            if let Some(priority) = priority {
                candidates.push((target, priority));
            }
        }

        let best_priority = candidates.iter().map(|(_, priority)| *priority).min()?;
        if let Some(id) = current_target
            && candidates
                .iter()
                .any(|(target, priority)| target.id == id && *priority == best_priority)
        {
            return Some(Selection {
                id,
                priority: best_priority,
            });
        }

        candidates
            .into_iter()
            .filter(|(_, priority)| *priority == best_priority)
            .map(|(target, priority)| {
                let candidate = ranking_candidate(own_position, target);
                (
                    Selection {
                        id: target.id,
                        priority,
                    },
                    targeting::candidate_score(&candidate, assignment_allowance),
                )
            })
            .min_by(|(a, a_score), (b, b_score)| {
                a_score.total_cmp(b_score).then_with(|| a.id.cmp(&b.id))
            })
            .map(|(selection, _)| selection)
    }

    /// [`Self::select`] and [`Self::explain`] in one pass: the explanation
    /// classifies every target once, and its choice is exactly what
    /// `select` returns (the same eligibility, priorities, kept current
    /// target, B41 score and id order).
    #[allow(clippy::too_many_arguments)]
    pub fn select_explained(
        &mut self,
        own_id: u32,
        own_side: targeting::Side,
        own_position: [f64; 3],
        assignment: &Assignment,
        targets: &[TargetView],
        protected: &[ProtectedView],
        reports: &[ThreatReport],
        current_target: Option<u32>,
        assignment_allowance: u32,
    ) -> (Option<Selection>, Explanation) {
        // Every test that selects checks the shared pass against `select`.
        #[cfg(test)]
        let mut reference = self.clone();
        #[cfg(test)]
        let expected = reference.select(
            own_id,
            own_side,
            own_position,
            assignment,
            targets,
            protected,
            reports,
            current_target,
            assignment_allowance,
        );
        self.update_leash(own_position, assignment, protected);
        let explanation = self.explain(
            own_id,
            own_side,
            own_position,
            assignment,
            targets,
            protected,
            reports,
            current_target,
            assignment_allowance,
        );
        let selection = explanation.chosen;
        #[cfg(test)]
        {
            assert_eq!(selection, expected);
            assert_eq!(*self, reference);
        }
        (selection, explanation)
    }

    /// Why [`Self::select`] chose what it did, for the replay debug panels.
    /// Call it after `select` with the same inputs: `select` updates the
    /// escort leash first, and this reads that state without changing
    /// anything. It reuses the same eligibility, priority and B41 score, and
    /// lists every observed aircraft (up to [`EXPLANATION_LIMIT`]), best
    /// first: the best priority by score, then the other priorities, then
    /// aircraft that were not candidates.
    #[allow(clippy::too_many_arguments)]
    pub fn explain(
        &self,
        own_id: u32,
        own_side: targeting::Side,
        own_position: [f64; 3],
        assignment: &Assignment,
        targets: &[TargetView],
        protected: &[ProtectedView],
        reports: &[ThreatReport],
        current_target: Option<u32>,
        assignment_allowance: u32,
    ) -> Explanation {
        let weapons_hold = assignment.stance == Stance::WeaponsHold;
        let own_attackers = known_attackers(reports, own_id);
        let protected_attackers: Vec<u32> = reports
            .iter()
            .filter(|report| assignment.protected_ids.contains(&report.defended_id))
            .filter_map(|report| report.attacker_id)
            .collect();
        let mut candidates: Vec<CandidateExplanation> = targets
            .iter()
            .map(|target| {
                let priority = (!weapons_hold && eligible(own_id, own_side, target))
                    .then(|| {
                        self.target_priority(
                            assignment,
                            target,
                            protected,
                            &own_attackers,
                            &protected_attackers,
                        )
                    })
                    .flatten();
                let candidate = ranking_candidate(own_position, target);
                CandidateExplanation {
                    id: target.id,
                    ineligible: Ineligibility::of(own_id, own_side, target),
                    priority,
                    distance_ft: candidate.spatial_distance_feet,
                    not_aircraft_penalty: !candidate.is_aircraft,
                    wing_attacking_penalty: candidate.wing_attackers >= 1,
                    wing_full_penalty: candidate.wing_attackers >= assignment_allowance,
                    score_ft: targeting::candidate_score(&candidate, assignment_allowance),
                    chosen: false,
                }
            })
            .collect();

        let best_priority = candidates.iter().filter_map(|c| c.priority).min();
        let kept_current = best_priority.is_some_and(|best| {
            current_target.is_some_and(|id| {
                candidates
                    .iter()
                    .any(|c| c.id == id && c.priority == Some(best))
            })
        });
        let chosen = best_priority.and_then(|best| {
            if kept_current {
                current_target.map(|id| Selection { id, priority: best })
            } else {
                candidates
                    .iter()
                    .filter(|c| c.priority == Some(best))
                    .min_by(|a, b| {
                        a.score_ft
                            .total_cmp(&b.score_ft)
                            .then_with(|| a.id.cmp(&b.id))
                    })
                    .map(|c| Selection {
                        id: c.id,
                        priority: best,
                    })
            }
        });
        if let Some(selection) = chosen
            && let Some(entry) = candidates.iter_mut().find(|c| c.id == selection.id)
        {
            entry.chosen = true;
        }
        // Candidates first, best priority first, then by score; the rest
        // keep their observed order.
        candidates.sort_by(|a, b| match (a.priority, b.priority) {
            (Some(pa), Some(pb)) => pa
                .cmp(&pb)
                .then_with(|| a.score_ft.total_cmp(&b.score_ft))
                .then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let omitted = candidates.len().saturating_sub(EXPLANATION_LIMIT);
        candidates.truncate(EXPLANATION_LIMIT);
        Explanation {
            weapons_hold,
            own_defense_only: assignment.stance == Stance::SelfDefense
                || assignment.role == Role::Disengage
                || self.escort_outside_leash,
            escort_outside_leash: self.escort_outside_leash,
            own_attackers,
            protected_attackers,
            best_priority,
            kept_current,
            chosen,
            candidates,
            omitted,
        }
    }

    /// The mission priority of one eligible target: the aircraft's own
    /// attackers first, then the assignment's priorities unless only
    /// self-defense applies.
    fn target_priority(
        &self,
        assignment: &Assignment,
        target: &TargetView,
        protected: &[ProtectedView],
        own_attackers: &[u32],
        protected_attackers: &[u32],
    ) -> Option<Priority> {
        if own_attackers.contains(&target.id) {
            Some(Priority::OwnDefense)
        } else if assignment.stance == Stance::SelfDefense
            || assignment.role == Role::Disengage
            || self.escort_outside_leash
        {
            None
        } else {
            self.mission_priority(assignment, target, protected, protected_attackers)
        }
    }

    fn mission_priority(
        &self,
        assignment: &Assignment,
        target: &TargetView,
        protected: &[ProtectedView],
        protected_attackers: &[u32],
    ) -> Option<Priority> {
        match assignment.stance {
            Stance::WeaponsHold | Stance::SelfDefense => None,
            Stance::ProtectAssigned => {
                if protected_attackers.contains(&target.id) {
                    Some(Priority::ProtectedThreat)
                } else if assignment.hostile_escorts.iter().any(|relationship| {
                    relationship.escort_id == target.id
                        && protected_attackers.contains(&relationship.principal_id)
                }) {
                    Some(Priority::HostileEscort)
                } else if protected.iter().any(|charge| {
                    charge.alive
                        && assignment.protected_ids.contains(&charge.id)
                        && threatens_charge(target, charge)
                }) {
                    Some(Priority::ApproachingThreat)
                } else if assignment.destroy_ids.contains(&target.id) {
                    Some(Priority::Assigned)
                } else {
                    None
                }
            }
            Stance::EngageAssigned => match assignment.role {
                Role::FreeEngagement => Some(Priority::Free),
                Role::CombatAirPatrol => assignment
                    .patrol
                    .filter(|region| region.contains(target.position))
                    .map(|_| Priority::Free),
                Role::Intercept | Role::Escort => assignment
                    .destroy_ids
                    .contains(&target.id)
                    .then_some(Priority::Assigned),
                Role::Disengage => None,
            },
        }
    }

    fn update_leash(
        &mut self,
        own_position: [f64; 3],
        assignment: &Assignment,
        protected: &[ProtectedView],
    ) {
        if assignment.role != Role::Escort {
            self.escort_outside_leash = false;
            return;
        }
        let distance = protected
            .iter()
            .filter(|view| view.alive && assignment.protected_ids.contains(&view.id))
            .map(|view| horizontal_distance(own_position, view.position))
            .min_by(f64::total_cmp);
        let Some(distance) = distance else {
            self.escort_outside_leash = true;
            return;
        };
        if self.escort_outside_leash {
            if distance <= ESCORT_REENTRY_LEASH_FT {
                self.escort_outside_leash = false;
            }
        } else if distance > ESCORT_EXIT_LEASH_FT {
            self.escort_outside_leash = true;
        }
    }
}

/// Resolve a supporting-radar bearing only from current observations. The
/// missile owner and any unobserved world actors are intentionally absent.
pub fn identify_supporting_attacker(
    own_position: [f64; 3],
    own_heading_deg: f64,
    radar_bearing_deg: f64,
    observed: &[TargetView],
    emitter_ids: &[u32],
    own_side: targeting::Side,
) -> Option<u32> {
    if !own_position.iter().all(|value| value.is_finite())
        || !own_heading_deg.is_finite()
        || !radar_bearing_deg.is_finite()
    {
        return None;
    }
    let mut matches = observed
        .iter()
        .filter(|target| {
            target.valid
                && target.is_aircraft
                && target.side != own_side
                && emitter_ids.contains(&target.id)
        })
        .filter(|target| {
            let dx = target.position[0] - own_position[0];
            let dz = target.position[2] - own_position[2];
            let relative_bearing = (dx.atan2(dz).to_degrees() - own_heading_deg).rem_euclid(360.);
            angular_difference_deg(relative_bearing, radar_bearing_deg)
                <= SUPPORTING_RADAR_IDENTIFICATION_TOLERANCE_DEG
        })
        .map(|target| target.id);
    let identified = matches.next()?;
    matches.next().is_none().then_some(identified)
}

fn known_attackers(reports: &[ThreatReport], defended_id: u32) -> Vec<u32> {
    reports
        .iter()
        .filter(|report| report.defended_id == defended_id)
        .filter_map(|report| report.attacker_id)
        .collect()
}

/// The B41 ranking view of one observed aircraft.
fn ranking_candidate(own_position: [f64; 3], target: &TargetView) -> targeting::CandidateTarget {
    let distance = spatial_distance(own_position, target.position);
    targeting::CandidateTarget {
        id: targeting::ObjectId(target.id),
        side: target.side,
        valid: target.valid,
        type_allowed: target.type_allowed,
        is_aircraft: target.is_aircraft,
        seeker_eligible: target.seeker_eligible,
        spatial_distance_feet: distance,
        wing_attackers: target.wing_attackers,
    }
}

fn eligible(own_id: u32, own_side: targeting::Side, target: &TargetView) -> bool {
    target.id != own_id
        && target.side != own_side
        && target.valid
        && target.is_aircraft
        && target.type_allowed
        && target.seeker_eligible
}

/// An observed aircraft near a charge, or one whose relative course reaches
/// the defended radius during the assessment horizon. This is an authored M1
/// escort rule, not evidence of the hostile's intent or missile ownership.
fn threatens_charge(target: &TargetView, charge: &ProtectedView) -> bool {
    if !target.position.iter().all(|v| v.is_finite())
        || !charge.position.iter().all(|v| v.is_finite())
        || !charge.velocity.iter().all(|v| v.is_finite())
        || !target.heading_deg.is_finite()
        || !target.pitch_deg.is_finite()
        || !target.speed.0.is_finite()
        || target.speed.0 < 0.
    {
        return false;
    }
    let displacement =
        std::array::from_fn::<_, 3, _>(|axis| target.position[axis] - charge.position[axis]);
    let range_sq = displacement.iter().map(|d| d * d).sum::<f64>();
    if !range_sq.is_finite() {
        return false;
    }
    if range_sq <= ESCORT_DEFENSE_RADIUS_FT.powi(2) {
        return true;
    }
    if range_sq > ESCORT_ASSESSMENT_RADIUS_FT.powi(2) {
        return false;
    }

    let heading = target.heading_deg.to_radians();
    let pitch = target.pitch_deg.to_radians();
    let horizontal_speed = target.speed.0 * pitch.cos();
    let velocity = [
        horizontal_speed * heading.sin(),
        target.speed.0 * pitch.sin(),
        horizontal_speed * heading.cos(),
    ];
    let relative_velocity =
        std::array::from_fn::<_, 3, _>(|axis| velocity[axis] - charge.velocity[axis]);
    let dot = displacement
        .iter()
        .zip(relative_velocity)
        .map(|(position, velocity)| position * velocity)
        .sum::<f64>();
    let speed_sq = relative_velocity.iter().map(|v| v * v).sum::<f64>();
    if !dot.is_finite() || !speed_sq.is_finite() || dot >= 0. || speed_sq <= 0. {
        return false;
    }
    let time = (-dot / speed_sq).clamp(0., ESCORT_ASSESSMENT_TIME_S);
    let closest_sq = displacement
        .iter()
        .zip(relative_velocity)
        .map(|(position, velocity)| (position + velocity * time).powi(2))
        .sum::<f64>();
    closest_sq.is_finite() && closest_sq <= ESCORT_DEFENSE_RADIUS_FT.powi(2)
}

fn spatial_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f64>()
        .sqrt()
}

fn horizontal_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn angular_difference_deg(a: f64, b: f64) -> f64 {
    ((a - b + 180.).rem_euclid(360.) - 180.).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::ScalarSpeed;

    const OWN: u32 = 1;
    const FRIEND: targeting::Side = targeting::Side(1);

    fn target(id: u32, distance: f64) -> TargetView {
        TargetView {
            infrared_strength: 1.,
            id,
            side: targeting::Side(2),
            position: [distance, 0., 0.],
            heading_deg: 0.,
            pitch_deg: 0.,
            speed: ScalarSpeed(300.),
            maximum_speed: ScalarSpeed(600.),
            is_aircraft: true,
            is_fighter: true,
            human_controlled: false,
            valid: true,
            type_allowed: true,
            seeker_eligible: true,
            wing_attackers: 0,
            terrain_blocked: false,
            sensor_supported: true,
            link_track: false,
        }
    }

    fn assignment(role: Role, stance: Stance) -> Assignment {
        Assignment {
            role,
            stance,
            protected_ids: vec![],
            destroy_ids: vec![],
            hostile_escorts: vec![],
            patrol: None,
        }
    }

    fn charge(position: [f64; 3], velocity: [f64; 3]) -> ProtectedView {
        ProtectedView {
            id: 10,
            position,
            velocity,
            alive: true,
        }
    }

    fn select(
        policy: &mut Policy,
        assignment: &Assignment,
        targets: &[TargetView],
        protected: &[ProtectedView],
        reports: &[ThreatReport],
        current: Option<u32>,
    ) -> Option<Selection> {
        policy.select(
            OWN, FRIEND, [0.; 3], assignment, targets, protected, reports, current, 1,
        )
    }

    #[test]
    fn protected_threat_outranks_nearer_unrelated_contact_and_escort() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        a.hostile_escorts.push(HostileEscort {
            principal_id: 3,
            escort_id: 4,
        });
        let reports = [ThreatReport {
            attacker_id: Some(3),
            defended_id: 10,
        }];
        let protected = [ProtectedView {
            id: 10,
            position: [0.; 3],
            velocity: [0.; 3],
            alive: true,
        }];
        let selected = select(
            &mut Policy::default(),
            &a,
            &[target(2, 100.), target(4, 200.), target(3, 5_000.)],
            &protected,
            &reports,
            None,
        );
        assert_eq!(
            selected,
            Some(Selection {
                id: 3,
                priority: Priority::ProtectedThreat
            })
        );
    }

    #[test]
    fn escort_engages_observed_hostiles_near_charge_and_on_intercept_course() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let protected = [charge([0.; 3], [0.; 3])];
        let near = target(2, ESCORT_DEFENSE_RADIUS_FT);
        assert_eq!(
            select(&mut Policy::default(), &a, &[near], &protected, &[], None),
            Some(Selection {
                id: 2,
                priority: Priority::ApproachingThreat,
            })
        );

        let mut approaching = target(3, 20. * FEET_PER_NAUTICAL_MILE);
        approaching.heading_deg = 270.;
        approaching.speed = ScalarSpeed(1_200.);
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[approaching],
                &protected,
                &[],
                None,
            ),
            Some(Selection {
                id: 3,
                priority: Priority::ApproachingThreat,
            })
        );
        assert_eq!(
            select(&mut Policy::default(), &a, &[], &protected, &[], None),
            None
        );
    }

    #[test]
    fn escort_assessment_rejects_outside_range_receding_and_tangent_tracks() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let protected = [charge([0.; 3], [0.; 3])];
        let mut outside = target(2, ESCORT_ASSESSMENT_RADIUS_FT + 1.);
        outside.heading_deg = 270.;
        outside.speed = ScalarSpeed(2_000.);
        let mut receding = target(3, 20. * FEET_PER_NAUTICAL_MILE);
        receding.heading_deg = 90.;
        receding.speed = ScalarSpeed(1_200.);
        let tangent = target(4, 20. * FEET_PER_NAUTICAL_MILE);
        for contact in [outside, receding, tangent] {
            assert_eq!(
                select(
                    &mut Policy::default(),
                    &a,
                    &[contact],
                    &protected,
                    &[],
                    None
                ),
                None
            );
        }
        let mut nonfinite = target(5, 100.);
        nonfinite.speed = ScalarSpeed(f64::NAN);
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[nonfinite],
                &protected,
                &[],
                None
            ),
            None
        );
    }

    #[test]
    fn escort_assessment_includes_exact_range_and_time_boundaries() {
        let protected = charge([0.; 3], [0.; 3]);
        for (range, speed, expected) in [
            (ESCORT_ASSESSMENT_RADIUS_FT, 3_000., true),
            (ESCORT_ASSESSMENT_RADIUS_FT + 1., 3_000., false),
            (ESCORT_DEFENSE_RADIUS_FT + 600. * 60., 600., true),
            (ESCORT_DEFENSE_RADIUS_FT + 600. * 60. + 1., 600., false),
        ] {
            let mut contact = target(2, 0.);
            contact.position = [0., 0., -range];
            contact.speed = ScalarSpeed(speed);
            assert_eq!(threatens_charge(&contact, &protected), expected);
        }
    }

    #[test]
    fn escort_assessment_uses_charge_motion_and_preserves_confirmed_priority() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let mut approaching = target(2, 20. * FEET_PER_NAUTICAL_MILE);
        approaching.heading_deg = 270.;
        approaching.speed = ScalarSpeed(600.);
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[approaching],
                &[charge([0.; 3], [0.; 3])],
                &[],
                None,
            ),
            None
        );
        let protected = [charge([0.; 3], [600., 0., 0.])];
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[approaching],
                &protected,
                &[],
                None,
            )
            .map(|selection| selection.id),
            Some(2)
        );
        let confirmed = target(3, 15. * FEET_PER_NAUTICAL_MILE);
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[approaching, confirmed],
                &protected,
                &[ThreatReport {
                    attacker_id: Some(3),
                    defended_id: 10,
                }],
                Some(2),
            ),
            Some(Selection {
                id: 3,
                priority: Priority::ProtectedThreat,
            })
        );
    }

    #[test]
    fn escort_assessment_does_not_override_hold_leash_or_dead_charge() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let near = target(2, 100.);
        let mut dead = charge([0.; 3], [0.; 3]);
        dead.alive = false;
        assert_eq!(
            select(&mut Policy::default(), &a, &[near], &[dead], &[], None),
            None
        );
        a.stance = Stance::WeaponsHold;
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[near],
                &[charge([0.; 3], [0.; 3])],
                &[],
                None,
            ),
            None
        );
        a.stance = Stance::ProtectAssigned;
        let mut policy = Policy::default();
        assert_eq!(
            select(
                &mut policy,
                &a,
                &[near],
                &[charge([ESCORT_EXIT_LEASH_FT + 1., 0., 0.], [0.; 3])],
                &[],
                None,
            ),
            None
        );
        assert!(policy.escort_outside_leash());
    }

    #[test]
    fn own_defense_preempts_protection_and_current_retention() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let reports = [
            ThreatReport {
                attacker_id: Some(2),
                defended_id: 10,
            },
            ThreatReport {
                attacker_id: Some(3),
                defended_id: OWN,
            },
        ];
        let protected = [ProtectedView {
            id: 10,
            position: [0.; 3],
            velocity: [0.; 3],
            alive: true,
        }];
        assert_eq!(
            select(
                &mut Policy::default(),
                &a,
                &[target(2, 100.), target(3, 5_000.)],
                &protected,
                &reports,
                Some(2),
            ),
            Some(Selection {
                id: 3,
                priority: Priority::OwnDefense
            })
        );
    }

    #[test]
    fn hold_unknown_attackers_and_same_side_never_authorize_fire() {
        let hold = assignment(Role::FreeEngagement, Stance::WeaponsHold);
        assert_eq!(
            select(
                &mut Policy::default(),
                &hold,
                &[target(2, 100.)],
                &[],
                &[ThreatReport {
                    attacker_id: Some(2),
                    defended_id: OWN
                }],
                None,
            ),
            None
        );
        let self_defense = assignment(Role::Disengage, Stance::SelfDefense);
        let mut friendly = target(2, 100.);
        friendly.side = FRIEND;
        assert_eq!(
            select(
                &mut Policy::default(),
                &self_defense,
                &[friendly, target(3, 200.)],
                &[],
                &[ThreatReport {
                    attacker_id: None,
                    defended_id: OWN
                }],
                None,
            ),
            None
        );
    }

    #[test]
    fn patrol_requires_a_region_and_includes_its_exact_boundary() {
        let mut cap = assignment(Role::CombatAirPatrol, Stance::EngageAssigned);
        assert_eq!(
            select(
                &mut Policy::default(),
                &cap,
                &[target(2, 100.)],
                &[],
                &[],
                None
            ),
            None
        );
        cap.patrol = Some(PatrolRegion {
            clip: None,
            center_ft: [0.; 3],
            radius_ft: 100.,
        });
        assert_eq!(
            select(
                &mut Policy::default(),
                &cap,
                &[target(2, 100.), target(3, 101.)],
                &[],
                &[],
                None,
            )
            .map(|selection| selection.id),
            Some(2)
        );
    }

    #[test]
    fn escort_leash_has_exact_hysteresis_and_reset() {
        let mut a = assignment(Role::Escort, Stance::EngageAssigned);
        a.protected_ids.push(10);
        a.destroy_ids.push(2);
        let target = [target(2, 100.)];
        let protected_at = |distance| {
            [ProtectedView {
                id: 10,
                position: [distance, 0., 0.],
                velocity: [0.; 3],
                alive: true,
            }]
        };
        let mut policy = Policy::default();
        assert!(
            select(
                &mut policy,
                &a,
                &target,
                &protected_at(ESCORT_EXIT_LEASH_FT),
                &[],
                None
            )
            .is_some()
        );
        assert_eq!(
            select(
                &mut policy,
                &a,
                &target,
                &protected_at(ESCORT_EXIT_LEASH_FT + 1.),
                &[],
                None
            ),
            None
        );
        assert!(policy.escort_outside_leash());
        assert_eq!(
            select(
                &mut policy,
                &a,
                &target,
                &protected_at(ESCORT_REENTRY_LEASH_FT + 1.),
                &[],
                None
            ),
            None
        );
        assert!(
            select(
                &mut policy,
                &a,
                &target,
                &protected_at(ESCORT_REENTRY_LEASH_FT),
                &[],
                None
            )
            .is_some()
        );
        policy.reset();
        assert!(!policy.escort_outside_leash());
    }

    #[test]
    fn current_target_is_kept_only_inside_the_best_priority() {
        let mut free = assignment(Role::FreeEngagement, Stance::EngageAssigned);
        let targets = [target(8, 100.), target(2, 1_000.)];
        assert_eq!(
            select(&mut Policy::default(), &free, &targets, &[], &[], Some(2))
                .map(|selection| selection.id),
            Some(2)
        );
        free.destroy_ids.push(8);
        let reports = [ThreatReport {
            attacker_id: Some(8),
            defended_id: OWN,
        }];
        assert_eq!(
            select(
                &mut Policy::default(),
                &free,
                &targets,
                &[],
                &reports,
                Some(2),
            ),
            Some(Selection {
                id: 8,
                priority: Priority::OwnDefense
            })
        );
    }

    #[test]
    fn invalid_destroy_objective_and_unarmed_neutral_are_not_forced_targets() {
        let mut intercept = assignment(Role::Intercept, Stance::EngageAssigned);
        intercept.destroy_ids.extend([2, 3]);
        let mut destroyed = target(2, 100.);
        destroyed.valid = false;
        let mut neutral = target(3, 200.);
        neutral.seeker_eligible = false;
        assert_eq!(
            select(
                &mut Policy::default(),
                &intercept,
                &[destroyed, neutral],
                &[],
                &[],
                None,
            ),
            None
        );
        let mut ground_object = target(2, 100.);
        ground_object.is_aircraft = false;
        assert_eq!(
            select(
                &mut Policy::default(),
                &intercept,
                &[ground_object],
                &[],
                &[],
                None,
            ),
            None
        );
    }

    #[test]
    fn default_preserves_free_engagement_and_b41_ranking_with_stable_ties() {
        let default = Assignment::default();
        let mut occupied = target(9, 100.);
        occupied.wing_attackers = 1;
        assert_eq!(
            select(
                &mut Policy::default(),
                &default,
                &[occupied, target(4, 1_000.)],
                &[],
                &[],
                None,
            )
            .map(|selection| selection.id),
            Some(4)
        );
        assert_eq!(
            select(
                &mut Policy::default(),
                &default,
                &[target(9, 1_000.), target(4, 1_000.)],
                &[],
                &[],
                None,
            )
            .map(|selection| selection.id),
            Some(4)
        );
    }

    #[test]
    fn remembered_permission_is_search_only_and_obeys_stance_reports_and_patrol() {
        let remembered = target(2, 100.);
        let policy = Policy::default();

        let hold = assignment(Role::FreeEngagement, Stance::WeaponsHold);
        assert!(!policy.allows_investigation(&hold, &remembered, &[], &[], OWN));

        let self_defense = assignment(Role::Disengage, Stance::SelfDefense);
        assert!(!policy.allows_investigation(&self_defense, &remembered, &[], &[], OWN));
        assert!(policy.allows_investigation(
            &self_defense,
            &remembered,
            &[],
            &[ThreatReport {
                attacker_id: Some(2),
                defended_id: OWN,
            }],
            OWN,
        ));

        let mut protect = assignment(Role::Escort, Stance::ProtectAssigned);
        protect.protected_ids.push(10);
        assert!(!policy.allows_investigation(&protect, &remembered, &[], &[], OWN));
        assert!(policy.allows_investigation(
            &protect,
            &remembered,
            &[],
            &[ThreatReport {
                attacker_id: Some(2),
                defended_id: 10,
            }],
            OWN,
        ));

        let mut cap = assignment(Role::CombatAirPatrol, Stance::EngageAssigned);
        cap.patrol = Some(PatrolRegion {
            clip: None,
            center_ft: [0.; 3],
            radius_ft: 100.,
        });
        assert!(policy.allows_investigation(&cap, &remembered, &[], &[], OWN));
        let outside = target(3, 101.);
        assert!(!policy.allows_investigation(&cap, &outside, &[], &[], OWN));

        // Investigation permission cannot create a current combat target.
        assert!(policy.allows_investigation(&Assignment::default(), &remembered, &[], &[], OWN));
        assert_eq!(
            select(
                &mut Policy::default(),
                &Assignment::default(),
                &[],
                &[],
                &[],
                None,
            ),
            None
        );
    }

    #[test]
    fn remembered_escort_investigation_stops_outside_the_leash() {
        let mut a = assignment(Role::Escort, Stance::EngageAssigned);
        a.protected_ids.push(10);
        a.destroy_ids.push(2);
        let protected = [ProtectedView {
            id: 10,
            position: [ESCORT_EXIT_LEASH_FT + 1., 0., 0.],
            velocity: [0.; 3],
            alive: true,
        }];
        let mut policy = Policy::default();
        assert_eq!(select(&mut policy, &a, &[], &protected, &[], None), None);
        assert!(!policy.allows_investigation(&a, &target(2, 100.), &protected, &[], OWN));
    }

    #[test]
    fn remembered_approach_can_guide_search_but_cannot_create_weapon_target() {
        let mut a = assignment(Role::Escort, Stance::ProtectAssigned);
        a.protected_ids.push(10);
        let protected = [charge([0.; 3], [0.; 3])];
        let remembered = target(2, ESCORT_DEFENSE_RADIUS_FT);
        let policy = Policy::default();
        assert!(policy.allows_investigation(&a, &remembered, &protected, &[], OWN));
        assert_eq!(
            select(&mut Policy::default(), &a, &[], &protected, &[], None),
            None
        );

        let outside = target(3, ESCORT_ASSESSMENT_RADIUS_FT + 1.);
        assert!(!policy.allows_investigation(&a, &outside, &protected, &[], OWN));
        let dead = [ProtectedView {
            alive: false,
            ..protected[0]
        }];
        assert!(!policy.allows_investigation(&a, &remembered, &dead, &[], OWN));
    }

    #[test]
    fn supporting_attacker_requires_one_current_hostile_emitter_on_bearing() {
        let hostile = target(2, 1_000.);
        assert_eq!(
            identify_supporting_attacker([0.; 3], 90., 0., &[hostile], &[2], FRIEND),
            Some(2)
        );
        assert_eq!(
            identify_supporting_attacker([0.; 3], 90., 0., &[], &[2], FRIEND),
            None
        );
        assert_eq!(
            identify_supporting_attacker([0.; 3], 90., 0., &[hostile], &[], FRIEND),
            None
        );

        let mut friendly = hostile;
        friendly.side = FRIEND;
        assert_eq!(
            identify_supporting_attacker([0.; 3], 90., 0., &[friendly], &[2], FRIEND),
            None
        );

        let mut second = target(3, 1_000.);
        second.position[2] = 10.;
        assert_eq!(
            identify_supporting_attacker([0.; 3], 90., 0., &[hostile, second], &[2, 3], FRIEND,),
            None
        );
    }

    /// The lobby pass's follow-up F1: a plane added to a lineage joins every
    /// list and relationship that names a plane of it, once, at the end.
    #[test]
    fn a_new_plane_joins_the_lists_that_name_its_lineage() {
        let mut assignment = Assignment {
            destroy_ids: vec![7, 2, 9],
            protected_ids: vec![4],
            hostile_escorts: vec![
                HostileEscort {
                    principal_id: 2,
                    escort_id: 5,
                },
                HostileEscort {
                    principal_id: 8,
                    escort_id: 2,
                },
            ],
            ..Assignment::default()
        };
        assert!(assignment.join_lineage(12, &[2, 10]));
        assert_eq!(assignment.destroy_ids, [7, 2, 9, 12]);
        assert_eq!(assignment.protected_ids, [4]);
        assert_eq!(
            assignment.hostile_escorts[2..],
            [
                HostileEscort {
                    principal_id: 12,
                    escort_id: 5,
                },
                HostileEscort {
                    principal_id: 8,
                    escort_id: 12,
                },
            ]
        );
        // Once only, and a lineage nobody names changes nothing.
        let joined = assignment.clone();
        assert!(!assignment.join_lineage(12, &[2, 10, 12]));
        assert!(!assignment.join_lineage(13, &[3]));
        assert_eq!(assignment, joined);
        let mut protect = Assignment {
            protected_ids: vec![3],
            ..Assignment::default()
        };
        assert!(protect.join_lineage(20, &[3]));
        assert_eq!(protect.protected_ids, [3, 20]);
        assert!(protect.destroy_ids.is_empty());
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "engagement_checkpoint.rs"]
mod checkpoint;
