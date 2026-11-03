//! Quick Mission delivery for the simulation's mission engagement policy.

use super::*;
use std::{fmt, str::FromStr};
use tore_sim::ai::engagement::{
    Assignment, GroupObjective, HostileEscort, PatrolRegion, Role, Stance,
};

pub const CAP_RADIUS_FT: f64 = 10. * tore_sim::sensors::FEET_PER_NAUTICAL_MILE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    #[default]
    Free,
    Cap,
    Intercept,
    Escort,
    SelfDefense,
    Hold,
}

impl Preset {
    pub const ALL: [Self; 6] = [
        Self::Free,
        Self::Cap,
        Self::Intercept,
        Self::Escort,
        Self::SelfDefense,
        Self::Hold,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Cap => "cap",
            Self::Intercept => "intercept",
            Self::Escort => "escort",
            Self::SelfDefense => "self-defense",
            Self::Hold => "hold",
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Preset {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "free" => Ok(Self::Free),
            "cap" => Ok(Self::Cap),
            "intercept" => Ok(Self::Intercept),
            "escort" => Ok(Self::Escort),
            "self-defense" | "self_defense" | "selfdefense" => Ok(Self::SelfDefense),
            "hold" => Ok(Self::Hold),
            _ => Err(format!(
                "unknown AI mission preset {value:?}; expected {}",
                Self::ALL.map(Self::name).join(", ")
            )),
        }
    }
}

impl AiWings {
    pub fn apply_mission_preset(&mut self, preset: Preset, player_position: [f64; 3]) {
        let primary_enemy = self
            .slots
            .iter()
            .find(|slot| slot.side == launch::Side::Enemy)
            .map(|slot| slot.id);
        // Every human-flown aircraft gets the preset's player assignment, and
        // the presets that name "the player" name every friendly human.
        for human in &self.humans {
            self.mission.set_human_assignment(
                human.id,
                clipped(
                    preset_for_human(preset, player_position, primary_enemy),
                    self.airfields.patrol_bounds,
                ),
            );
        }
        let friendly_humans: Vec<u32> = self
            .humans
            .iter()
            .filter(|h| h.side == launch::Side::Friendly)
            .map(|h| h.id)
            .collect();

        let mut assignments: Vec<_> = self
            .slots
            .iter()
            .map(|slot| {
                (
                    slot.id,
                    actor_assignment(
                        preset,
                        self.patrol_station(slot.side, slot.wing_number - 1, player_position),
                        primary_enemy,
                        slot,
                        &friendly_humans,
                    ),
                )
            })
            .collect();
        let relationships: Vec<_> = assignments
            .iter()
            .flat_map(|(id, assignment)| {
                assignment
                    .protected_ids
                    .iter()
                    .map(move |principal| HostileEscort {
                        principal_id: *principal,
                        escort_id: *id,
                    })
            })
            .collect();
        for (id, assignment) in &mut assignments {
            let side = self.slot(*id).unwrap().side;
            assignment.hostile_escorts = relationships
                .iter()
                .filter(|relation| {
                    self.slot(relation.escort_id)
                        .is_some_and(|escort| escort.side != side)
                })
                .copied()
                .collect();
        }
        for (id, assignment) in assignments {
            if let Some(actor) = self.mission.actor_mut(id) {
                actor.set_assignment(clipped(assignment, self.airfields.patrol_bounds));
            }
        }
        self.mission_preset = preset;
    }

    /// Resolve six group stamps after the mission preset. Inactive groups
    /// produce empty assignments, never fallback targets on the other side.
    pub fn apply_group_objectives(
        &mut self,
        objectives: &[GroupObjective; 6],
        player_position: [f64; 3],
    ) {
        for (index, objective) in objectives.iter().copied().enumerate() {
            // Strike leaves the air orders to the mission, as Inherit does
            // (its role is set by `apply_strike_roles`).
            if matches!(objective, GroupObjective::Inherit | GroupObjective::Strike) {
                continue;
            }
            let side = if index < 3 {
                launch::Side::Friendly
            } else {
                launch::Side::Enemy
            };
            let wing = index as u8 % 3;
            let mut assignment = match objective {
                GroupObjective::Inherit | GroupObjective::Strike => unreachable!(),
                GroupObjective::Free => Assignment::default(),
                GroupObjective::Cap => cap(self.patrol_station(side, wing, player_position)),
                GroupObjective::SelfDefense => self_defense(),
                GroupObjective::Hold => hold(),
                GroupObjective::Intercept(group) => {
                    let ids = if group.side != side {
                        self.group_members(group)
                    } else {
                        Vec::new()
                    };
                    Assignment {
                        role: Role::Intercept,
                        stance: Stance::EngageAssigned,
                        destroy_ids: ids,
                        ..Assignment::default()
                    }
                }
                GroupObjective::Escort(group) => {
                    let ids = if group.side == side && group.index != wing {
                        self.group_members(group)
                    } else {
                        Vec::new()
                    };
                    Assignment {
                        role: Role::Escort,
                        stance: Stance::ProtectAssigned,
                        protected_ids: ids,
                        ..Assignment::default()
                    }
                }
            };
            // Inherit existing relationship metadata until the complete set of
            // group assignments below rebuilds it symmetrically.
            assignment = clipped(assignment, self.airfields.patrol_bounds);
            assignment.hostile_escorts.clear();
            let members: Vec<_> = self
                .slots
                .iter()
                .filter(|slot| slot.side == side && slot.wing_number == wing + 1)
                .map(|slot| slot.id)
                .collect();
            for id in members {
                if let Some(actor) = self.mission.actor_mut(id) {
                    actor.set_assignment(assignment.clone());
                }
            }
            let humans: Vec<u32> = self
                .humans
                .iter()
                .filter(|h| h.side == side && h.wing == wing)
                .map(|h| h.id)
                .collect();
            for human in humans {
                self.mission.set_human_assignment(human, assignment.clone());
            }
        }
        let relationships: Vec<_> = self
            .mission
            .actors()
            .iter()
            .flat_map(|actor| {
                actor
                    .assignment()
                    .protected_ids
                    .iter()
                    .map(move |principal| {
                        (
                            actor.identity().side,
                            HostileEscort {
                                principal_id: *principal,
                                escort_id: actor.id(),
                            },
                        )
                    })
            })
            .collect();
        for actor in self.mission.actors_mut() {
            let mut assignment = actor.assignment().clone();
            assignment.hostile_escorts = relationships
                .iter()
                .filter(|(side, _)| *side != actor.identity().side)
                .map(|(_, relationship)| *relationship)
                .collect();
            actor.set_assignment(clipped(assignment, self.airfields.patrol_bounds));
        }
    }

    /// Fitted station for creator CAP: the wing leader's launch position.
    /// A human-led wing uses the supplied human launch position.
    fn patrol_station(&self, side: launch::Side, wing: u8, human_position: [f64; 3]) -> [f64; 3] {
        if self
            .humans
            .iter()
            .any(|h| h.side == side && h.wing == wing && h.member == 0)
        {
            return human_position;
        }
        self.slots
            .iter()
            .filter(|s| s.side == side && s.wing_number == wing + 1)
            .min_by_key(|s| s.member_number)
            .and_then(|s| self.mission.actor(s.id))
            .map_or(human_position, |a| a.flight().position)
    }

    pub fn apply_group_survival(&mut self, groups: &[bool; 6]) {
        let ids = self.survival_ids(groups);
        for human in self.humans.iter().map(|h| h.id).collect::<Vec<_>>() {
            self.mission.set_must_survive(human, ids.clone());
        }
    }

    /// An open mission's objectives (the lobby pass's follow-up F1): every
    /// plane starts on the AI, so no human had its objectives set when the
    /// mission was built, and a human who took a plane by handoff was asked
    /// nothing (its target window never read "Obj:", its debrief listed no
    /// friendly objective). Here every plane is given, as a human-flown
    /// plane's, the assignment the mission gave its AI pilot (its wing's
    /// group objective over the preset) and the survival list of `groups`,
    /// so whoever flies it is asked what the mission asks of that plane.
    /// Call after [`Self::apply_mission_preset`], the group objectives and
    /// the group survival. Single player never builds an open mission.
    pub fn open_objectives(&mut self, groups: &[bool; 6]) {
        let ids = self.survival_ids(groups);
        let planes: Vec<u32> = self.slots.iter().map(|slot| slot.id).collect();
        for plane in planes {
            if let Some(actor) = self.mission.actor(plane) {
                let assignment = actor.assignment().clone();
                self.mission.set_human_assignment(plane, assignment);
            }
            if !ids.is_empty() {
                self.mission.set_must_survive(plane, ids.clone());
            }
        }
    }

    /// `new`, a plane a respawn or a revival added, continues the lineage
    /// `lineage` (oldest first): every objective that names the lineage names
    /// `new` too ([`tore_sim::ai::mission::AiMission::join_lineage`]; the
    /// lobby pass's follow-up F1).
    pub(crate) fn join_lineage(&mut self, new: u32, lineage: &[u32]) {
        self.mission.join_lineage(new, lineage);
    }

    /// The aircraft of the groups that must survive, in group order.
    fn survival_ids(&self, groups: &[bool; 6]) -> Vec<u32> {
        groups
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, required)| *required)
            .flat_map(|(index, _)| {
                let side = if index < 3 {
                    launch::Side::Friendly
                } else {
                    launch::Side::Enemy
                };
                self.group_members(
                    launch::WingId::new(side, index as u8 % 3).expect("fixed Quick Mission group"),
                )
            })
            .collect()
    }

    fn group_members(&self, group: launch::WingId) -> Vec<u32> {
        // The humans in the group come first, in id order.
        self.humans
            .iter()
            .filter(|h| h.side == group.side && h.wing == group.index)
            .map(|h| h.id)
            .chain(
                self.slots
                    .iter()
                    .filter(|slot| slot.side == group.side && slot.wing_number == group.index + 1)
                    .map(|slot| slot.id),
            )
            .collect()
    }

    /// Mission requirement for an aircraft target as human-flown aircraft
    /// `viewer` sees it, from that aircraft's own assignment and survival
    /// list. Allegiance is checked before assignment lists so invalid
    /// cross-side metadata cannot manufacture a destroy or survival
    /// requirement.
    pub fn target_objective(
        &self,
        viewer: u32,
        id: u32,
    ) -> Option<crate::target_window::TargetObjective> {
        use crate::target_window::TargetObjective;
        let side_of = |plane: u32| match self.humans.iter().find(|h| h.id == plane) {
            Some(human) => Some(human.side),
            None => self.slot(plane).map(|slot| slot.side),
        };
        let side = side_of(id)?;
        // The viewer's own side: a Redfor player's friends are the enemy
        // side's planes (the lobby pass's follow-up F1; single player's
        // viewer is always friendly).
        let own = side_of(viewer).unwrap_or(launch::Side::Friendly);
        let assignment = self.mission.human_assignment(viewer);
        if side == own {
            (assignment.protected_ids.contains(&id)
                || self.mission.must_survive(viewer).contains(&id))
            .then_some(TargetObjective::Survive)
        } else {
            assignment
                .destroy_ids
                .contains(&id)
                .then_some(TargetObjective::Destroy)
        }
    }

    #[cfg(test)]
    pub fn objective_for_player(&self, id: u32) -> bool {
        self.target_objective(PLAYER_ID, id).is_some()
    }
}

fn preset_for_human(
    preset: Preset,
    player_position: [f64; 3],
    primary_enemy: Option<u32>,
) -> Assignment {
    match preset {
        Preset::Free => Assignment::default(),
        Preset::Cap => cap(player_position),
        Preset::Intercept => assigned(Role::Intercept, primary_enemy),
        // In the escort preset the player is the protected charge. The player
        // itself receives self-defense policy rather than a fabricated target.
        Preset::Escort | Preset::SelfDefense => self_defense(),
        Preset::Hold => hold(),
    }
}

fn actor_assignment(
    preset: Preset,
    player_position: [f64; 3],
    primary_enemy: Option<u32>,
    slot: &Slot,
    friendly_humans: &[u32],
) -> Assignment {
    match preset {
        Preset::Free => Assignment::default(),
        Preset::Cap => cap(player_position),
        Preset::SelfDefense => self_defense(),
        Preset::Hold => hold(),
        Preset::Intercept => match slot.side {
            launch::Side::Friendly => assigned(Role::Intercept, primary_enemy),
            launch::Side::Enemy if Some(slot.id) == primary_enemy => {
                assigned_to(Role::Intercept, friendly_humans)
            }
            launch::Side::Enemy => protect(primary_enemy),
        },
        Preset::Escort => match slot.side {
            launch::Side::Friendly => protect_all(friendly_humans),
            launch::Side::Enemy if Some(slot.id) == primary_enemy => {
                assigned_to(Role::Intercept, friendly_humans)
            }
            launch::Side::Enemy => protect(primary_enemy),
        },
    }
}

fn cap(center_ft: [f64; 3]) -> Assignment {
    Assignment {
        role: Role::CombatAirPatrol,
        stance: Stance::EngageAssigned,
        patrol: Some(PatrolRegion {
            clip: None,
            center_ft,
            radius_ft: CAP_RADIUS_FT,
        }),
        ..Assignment::default()
    }
}

fn assigned(role: Role, target: Option<u32>) -> Assignment {
    Assignment {
        role,
        stance: Stance::EngageAssigned,
        destroy_ids: target.into_iter().collect(),
        ..Assignment::default()
    }
}

fn assigned_to(role: Role, targets: &[u32]) -> Assignment {
    Assignment {
        role,
        stance: Stance::EngageAssigned,
        destroy_ids: targets.to_vec(),
        ..Assignment::default()
    }
}

fn protect_all(targets: &[u32]) -> Assignment {
    Assignment {
        role: Role::Escort,
        stance: Stance::ProtectAssigned,
        protected_ids: targets.to_vec(),
        ..Assignment::default()
    }
}

fn protect(target: Option<u32>) -> Assignment {
    Assignment {
        role: Role::Escort,
        stance: Stance::ProtectAssigned,
        protected_ids: target.into_iter().collect(),
        ..Assignment::default()
    }
}

fn self_defense() -> Assignment {
    Assignment {
        role: Role::Disengage,
        stance: Stance::SelfDefense,
        ..Assignment::default()
    }
}

fn hold() -> Assignment {
    Assignment {
        stance: Stance::WeaponsHold,
        ..Assignment::default()
    }
}

/// Preserve the station and ten-mile circle, intersected with the theater.
fn clipped(mut assignment: Assignment, bounds: Option<[[f64; 2]; 2]>) -> Assignment {
    if let Some(region) = &mut assignment.patrol {
        region.clip = bounds;
        if let Some([min, max]) = bounds {
            region.center_ft[0] = region.center_ft[0].clamp(min[0], max[0]);
            region.center_ft[2] = region.center_ft[2].clamp(min[1], max[1]);
        }
    }
    assignment
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_formats::aircraft::AircraftId;

    fn slot(id: u32, side: launch::Side) -> Slot {
        Slot {
            id,
            side,
            wing_number: 1,
            member_number: id as u8,
            aircraft: AircraftId::F18,
        }
    }

    #[test]
    fn group_stamps_reach_both_sides_and_the_target_view() {
        let (mut wings, targets) = super::super::tests::build(None);
        wings.apply_mission_preset(Preset::Free, [0., 20000., 0.]);
        let enemy = launch::WingId::new(launch::Side::Enemy, 0).unwrap();
        let mut objectives = [GroupObjective::Inherit; 6];
        // Fixture friendly actors are group 2; player is friendly group 1.
        objectives[0] = GroupObjective::Intercept(enemy);
        objectives[1] = GroupObjective::Intercept(enemy);
        objectives[3] = GroupObjective::SelfDefense;
        wings.apply_group_objectives(&objectives, [0., 20000., 0.]);
        assert_eq!(
            wings.mission.actor(1).unwrap().assignment().destroy_ids,
            [3, 4]
        );
        assert_eq!(
            wings.mission.actor(3).unwrap().assignment().stance,
            Stance::SelfDefense
        );
        assert!(wings.objective_for_player(3));
        assert!(!wings.objective_for_player(1));
        let actor = wings.mission.actor(3).unwrap();
        let mut readout = crate::target_window::Readout::new(
            &crate::readout::TargetRow::of(&targets[2]),
            actor.flight(),
            "TEST".into(),
        );
        readout.with_brief(&crate::target_window::TargetBrief::of(
            &wings, PLAYER_ID, readout.id,
        ));
        assert_eq!(
            readout.objective,
            Some(crate::target_window::TargetObjective::Destroy)
        );
    }

    #[test]
    fn target_objectives_are_player_relative_and_side_checked() {
        use crate::target_window::TargetObjective;
        let (mut wings, _) = super::super::tests::build(None);

        let mut survival = [false; 6];
        survival[1] = true;
        survival[3] = true;
        wings.apply_group_survival(&survival);
        assert_eq!(wings.mission.must_survive(PLAYER_ID), [1, 2, 3, 4]);
        assert_eq!(
            wings.target_objective(PLAYER_ID, 1),
            Some(TargetObjective::Survive)
        );
        assert_eq!(wings.target_objective(PLAYER_ID, 3), None);

        survival[1] = false;
        wings.apply_group_survival(&survival);
        let mut objectives = [GroupObjective::Inherit; 6];
        objectives[0] =
            GroupObjective::Escort(launch::WingId::new(launch::Side::Friendly, 1).unwrap());
        objectives[1] = GroupObjective::Free;
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert_eq!(
            wings.target_objective(PLAYER_ID, 1),
            Some(TargetObjective::Survive)
        );
        assert_eq!(
            wings.target_objective(PLAYER_ID, 2),
            Some(TargetObjective::Survive)
        );

        objectives[0] =
            GroupObjective::Intercept(launch::WingId::new(launch::Side::Enemy, 0).unwrap());
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert_eq!(
            wings.target_objective(PLAYER_ID, 3),
            Some(TargetObjective::Destroy)
        );
        assert_eq!(
            wings.target_objective(PLAYER_ID, 4),
            Some(TargetObjective::Destroy)
        );
        assert_eq!(wings.target_objective(PLAYER_ID, 1), None);
        assert_eq!(wings.target_objective(PLAYER_ID, 99), None);
    }

    #[test]
    fn group_references_never_substitute_an_empty_or_wrong_side_group() {
        let (mut wings, _) = super::super::tests::build(None);
        let mut objectives = [GroupObjective::Inherit; 6];
        objectives[1] =
            GroupObjective::Intercept(launch::WingId::new(launch::Side::Enemy, 2).unwrap());
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert!(
            wings
                .mission
                .actor(1)
                .unwrap()
                .assignment()
                .destroy_ids
                .is_empty()
        );
        objectives[1] =
            GroupObjective::Intercept(launch::WingId::new(launch::Side::Friendly, 0).unwrap());
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert!(
            wings
                .mission
                .actor(1)
                .unwrap()
                .assignment()
                .destroy_ids
                .is_empty()
        );
        objectives[1] =
            GroupObjective::Escort(launch::WingId::new(launch::Side::Friendly, 0).unwrap());
        wings.apply_group_objectives(&objectives, [0.; 3]);
        assert_eq!(
            wings.mission.actor(1).unwrap().assignment().protected_ids,
            [PLAYER_ID]
        );
    }

    #[test]
    fn creator_cap_uses_each_wings_station_on_both_sides() {
        let (mut wings, _) = super::super::tests::build(None);
        let positions: Vec<_> = wings
            .slots
            .iter()
            .map(|s| {
                (
                    s.id,
                    s.side,
                    s.wing_number,
                    s.member_number,
                    wings.mission.actor(s.id).unwrap().flight().position,
                )
            })
            .collect();
        wings.apply_group_objectives(&[GroupObjective::Cap; 6], [12., 20000., 34.]);
        for (id, side, wing, _, _) in &positions {
            let center = if *side == launch::Side::Friendly && *wing == 1 {
                [12., 20000., 34.]
            } else {
                positions
                    .iter()
                    .filter(|p| p.1 == *side && p.2 == *wing)
                    .min_by_key(|p| p.3)
                    .unwrap()
                    .4
            };
            assert_eq!(
                wings
                    .mission
                    .actor(*id)
                    .unwrap()
                    .assignment()
                    .patrol
                    .unwrap()
                    .center_ft,
                center
            );
        }
    }

    #[test]
    fn names_round_trip_and_reject_unknown_values() {
        for preset in Preset::ALL {
            assert_eq!(preset.name().parse(), Ok(preset));
            assert_eq!(preset.to_string(), preset.name());
        }
        assert_eq!("SELF_DEFENSE".parse(), Ok(Preset::SelfDefense));
        assert!("attack-everything".parse::<Preset>().is_err());
    }

    #[test]
    fn cap_uses_the_player_launch_position_and_authored_ten_nm_radius() {
        let position = [12., 34., 56.];
        let assignment = actor_assignment(
            Preset::Cap,
            position,
            Some(3),
            &slot(1, launch::Side::Friendly),
            &[PLAYER_ID],
        );
        assert_eq!(assignment.role, Role::CombatAirPatrol);
        assert_eq!(assignment.stance, Stance::EngageAssigned);
        assert_eq!(assignment.patrol.unwrap().center_ft, position);
        assert_eq!(assignment.patrol.unwrap().radius_ft, CAP_RADIUS_FT);
    }

    #[test]
    fn intercept_assigns_friendlies_and_makes_enemy_wingmen_escorts() {
        let friendly = actor_assignment(
            Preset::Intercept,
            [0.; 3],
            Some(3),
            &slot(1, launch::Side::Friendly),
            &[PLAYER_ID],
        );
        assert_eq!(friendly.destroy_ids, [3]);
        let primary = actor_assignment(
            Preset::Intercept,
            [0.; 3],
            Some(3),
            &slot(3, launch::Side::Enemy),
            &[PLAYER_ID],
        );
        assert_eq!(primary.destroy_ids, [PLAYER_ID]);
        let escort = actor_assignment(
            Preset::Intercept,
            [0.; 3],
            Some(3),
            &slot(4, launch::Side::Enemy),
            &[PLAYER_ID],
        );
        assert_eq!(escort.role, Role::Escort);
        assert_eq!(escort.protected_ids, [3]);
    }

    #[test]
    fn escort_protects_player_and_gives_enemy_primary_the_intercept() {
        let friendly = actor_assignment(
            Preset::Escort,
            [0.; 3],
            Some(3),
            &slot(1, launch::Side::Friendly),
            &[PLAYER_ID],
        );
        assert_eq!(friendly.protected_ids, [PLAYER_ID]);
        let primary = actor_assignment(
            Preset::Escort,
            [0.; 3],
            Some(3),
            &slot(3, launch::Side::Enemy),
            &[PLAYER_ID],
        );
        assert_eq!(primary.destroy_ids, [PLAYER_ID]);
        let antagonist = actor_assignment(
            Preset::Escort,
            [0.; 3],
            Some(3),
            &slot(4, launch::Side::Enemy),
            &[PLAYER_ID],
        );
        assert_eq!(antagonist.protected_ids, [3]);
    }

    #[test]
    fn missing_primary_never_invents_an_objective() {
        let intercept = actor_assignment(
            Preset::Intercept,
            [0.; 3],
            None,
            &slot(1, launch::Side::Friendly),
            &[PLAYER_ID],
        );
        assert!(intercept.destroy_ids.is_empty());
        assert!(
            preset_for_human(Preset::Intercept, [0.; 3], None)
                .destroy_ids
                .is_empty()
        );
    }

    #[test]
    fn self_defense_and_hold_have_distinct_stances() {
        assert_eq!(self_defense().stance, Stance::SelfDefense);
        assert_eq!(self_defense().role, Role::Disengage);
        assert_eq!(hold().stance, Stance::WeaponsHold);
    }
}

#[cfg(test)]
mod clipping_tests {
    use super::*;
    #[test]
    fn cap_is_the_intersection_with_the_map_not_a_smaller_circle() {
        let a = clipped(
            cap([100., 20_000., 100.]),
            Some([[0., 0.], [200_000., 200_000.]]),
        );
        let p = a.patrol.unwrap();
        assert_eq!(p.radius_ft, CAP_RADIUS_FT);
        assert!(p.contains([40_000., 20_000., 100.]));
        assert!(!p.contains([-1., 20_000., 100.]));
        assert!(!p.contains([100., 20_000., -1.]));
        assert!(!p.contains([100_000., 20_000., 100.]));
    }
}
