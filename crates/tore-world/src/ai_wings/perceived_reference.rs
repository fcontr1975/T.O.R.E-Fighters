//! The original `report_perceived_attacks` (before the S7 reorder), kept to
//! compare the reordered search against: range, then terrain, then the
//! shooters, with the same reports in the same order.
use super::*;

impl AiWings {
    pub(super) fn perceived_attacks_reference(
        &self,
        state: &live::State,
        humans: &[&HumanAircraft],
        ground: &dyn Fn(f64, f64) -> f64,
    ) -> Vec<super::PerceivedAttack> {
        use tore_sim::ai::{awareness, engagement::ThreatReport};
        use tore_sim::combat::threats::EvidenceSource;
        // The human-flown aircraft that combat keeps an ownship for, in id
        // order: each one's own records (RWR, emitters, sensor picture) reach
        // the AI's attack evidence for that aircraft alone.
        let combats: Vec<(&HumanAircraft, &live::Ownship)> = humans
            .iter()
            .copied()
            .filter_map(|h| state.ownship(h.slot.id).map(|own| (h, own)))
            .collect();
        let mut reports = Vec::new();
        // A human-flown aircraft's RWR may identify a supporting source only
        // by a unique independently observed hostile emitter at the received
        // bearing. The records belong to one aircraft, so each is delivered
        // to that aircraft and no other.
        for (human, own, record) in combats.iter().flat_map(|(human, own)| {
            own.missile_threats
                .records()
                .filter(|r| r.targeting_receiver && !r.stale)
                .map(move |record| (*human, *own, record))
        }) {
            let attacker_id = if record.source == EvidenceSource::ElectronicSupported {
                record.radar_bearing_deg.and_then(|bearing| {
                    let mut matches = own.emitters.iter().filter(|emitter| {
                        self.slot(emitter.id)
                            .is_some_and(|slot| slot.side != human.slot.side)
                            && own.sensors.observation(emitter.id).is_some()
                            && ((emitter.bearing_rad.to_degrees() - bearing + 180.)
                                .rem_euclid(360.)
                                - 180.)
                                .abs()
                                <= 2.
                    });
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first.id)
                })
            } else {
                None
            };
            reports.push((
                human.slot.id,
                ThreatReport {
                    attacker_id,
                    defended_id: human.slot.id,
                },
                Some(
                    (human.flight.yaw.to_degrees()
                        + record.radar_bearing_deg.unwrap_or(record.bearing_deg))
                    .rem_euclid(360.),
                ),
                Some(record.missile_id),
            ));
        }
        // A fresh, visibly departing missile or tracer can reveal its shooter
        // only when that aircraft is independently observed. Hidden projectile
        // target IDs do not participate in this association.
        for projectile in state
            .projectiles
            .iter()
            .filter(|p| p.age <= 30 && p.threatening())
        {
            let weapon = state.weapon(projectile);
            let gun = live::is_gun(weapon);
            if gun && !projectile.tracer {
                continue;
            }
            // A human without an ownship has no records to perceive a shot
            // with, so it is not a receiver here.
            for receiver in combats.iter().map(|(h, _)| h.slot.id).chain(
                self.mission
                    .actors()
                    .iter()
                    .filter(|a| a.alive() && !a.is_dummy())
                    .map(AiActor::id),
            ) {
                if receiver == projectile.owner {
                    continue;
                }
                let (position, velocity, heading, pitch, skill, possible_shooters, incoming) =
                    if let Some((human, own)) =
                        combats.iter().copied().find(|(h, _)| h.slot.id == receiver)
                    {
                        let player = human.flight;
                        (
                            player.position,
                            player.velocity,
                            player.yaw.to_degrees(),
                            player.pitch.to_degrees(),
                            tore_sim::ai::Experience::Ace,
                            state
                                .targets
                                .iter()
                                .filter(|target| {
                                    self.slot(target.id)
                                        .is_some_and(|slot| slot.side != human.slot.side)
                                })
                                .filter_map(|target| {
                                    own.sensors
                                        .observation(target.id)
                                        .map(|o| (target.id, o.position))
                                })
                                .collect::<Vec<_>>(),
                            own.missile_threats.records().any(|r| {
                                r.missile_id == projectile.id && r.targeting_receiver && !r.stale
                            }),
                        )
                    } else {
                        let actor = self.mission.actor(receiver).unwrap();
                        (
                            actor.flight().position,
                            actor.flight().velocity,
                            actor.flight().yaw.to_degrees(),
                            actor.flight().pitch.to_degrees(),
                            actor.controller().experience().level,
                            actor
                                .awareness()
                                .current_observations()
                                .filter(|o| o.target.side != actor.identity().side)
                                .map(|o| (o.target.id, o.target.position))
                                .collect::<Vec<_>>(),
                            actor.missile_threats().any(|r| {
                                r.missile_id == projectile.id && r.targeting_receiver && !r.stale
                            }),
                        )
                    };
                let pilot_sees = |point| {
                    let clear = super::terrain_visible(position, point, ground);
                    if let Some(lookout) = self.mission.actor(receiver).and_then(AiActor::lookout) {
                        lookout.check(skill, point, None, clear) == awareness::VisualResult::Visible
                    } else {
                        awareness::visual_eligible(
                            skill, position, heading, pitch, point, None, clear,
                        )
                    }
                };
                if !pilot_sees(projectile.position) {
                    continue;
                }
                let mut launch_sources = possible_shooters.into_iter().filter(|(_, shooter)| {
                    missiles::length(missiles::sub(*shooter, projectile.previous)) <= 1000.
                        && pilot_sees(*shooter)
                });
                let Some((shooter, _)) = launch_sources.next() else {
                    continue;
                };
                if launch_sources.next().is_some() {
                    continue;
                }
                let gun_incoming = if gun && !humans.iter().any(|h| h.slot.id == receiver) {
                    self.mission
                        .actor(receiver)
                        .and_then(AiActor::incoming_fire_cue)
                        .is_some_and(|cue| {
                            cue.round == Some(projectile.id)
                                && cue.observed_tick + 1 == self.mission.tick()
                        })
                } else if gun {
                    let delta = missiles::sub(projectile.position, position);
                    let movement = std::array::from_fn::<_, 3, _>(|i| {
                        (projectile.position[i] - projectile.previous[i]) * 120. - velocity[i]
                    });
                    let vv = tore_sim::attitude::dot(movement, movement);
                    let time = if vv > 0. {
                        -tore_sim::attitude::dot(delta, movement) / vv
                    } else {
                        -1.
                    };
                    let closest = std::array::from_fn::<_, 3, _>(|i| delta[i] + movement[i] * time);
                    (0. ..=15.).contains(&time) && missiles::length(closest) <= 1000.
                } else {
                    false
                };
                if !(incoming || gun_incoming) {
                    continue;
                }
                let delta = missiles::sub(projectile.position, position);
                reports.push((
                    receiver,
                    ThreatReport {
                        attacker_id: Some(shooter),
                        defended_id: receiver,
                    },
                    Some(delta[0].atan2(delta[2]).to_degrees().rem_euclid(360.)),
                    Some(projectile.id),
                ));
            }
        }
        reports
    }
}
