//! The bridge half of the gunship's guns (docs/spec/ai-strike.md, "AC-130";
//! slice ST-F). The gunship's tactics ([`tore_sim::ai::strike::gunship`])
//! fly the orbit and say when the trigger is held and where the burst aims
//! ([`tore_sim::ai::strike::Strike::gunship_fire`]); this module, before
//! every mission step, trains each of its linked guns on that aim with the
//! gun's own ballistics, inside its mount's arcs
//! ([`live::ai_gun_train`]), and fires the guns that bear at their records'
//! cadence through combat's shared release ([`live::State::release_ai_gun_round`]).
//! The rounds are the AI's own, debited from its stations and credited to
//! it. With no gunship firing it costs one scan of the actors.
use super::{AiWings, aim_latest};
use tore_sim::ai::strike::StoreKind;
use tore_sim::ai::weapon_service::StationId;
use tore_sim::combat::gunship;
use tore_sim::combat::gunsight::TargetObservation;
use tore_sim::combat::live::{self, AiGunRound};
use tore_sim::models::strike_class::StrikeClass;

/// One step of every AI gunship's guns.
pub(super) fn step(
    wings: &mut AiWings,
    state: &mut live::State,
    ground: &(dyn Fn(f64, f64) -> f64 + Sync),
) {
    let tick = wings.mission.tick();
    let firing: Vec<_> = wings
        .mission
        .actors()
        .iter()
        .filter(|a| a.alive() && a.strikes())
        .filter(|a| {
            wings
                .configs
                .get(&a.id())
                .is_some_and(|c| c.strike_class == StrikeClass::Gunship)
        })
        .filter_map(|a| a.strike()?.gunship_fire(tick).map(|fire| (a.id(), fire)))
        .collect();
    for (id, fire) in firing {
        let Some(config) = wings.configs.get(&id) else {
            continue;
        };
        let Some(actor) = wings.mission.actor(id) else {
            continue;
        };
        let Some(row) = state
            .targets
            .iter()
            .find(|t| t.id == fire.target && t.hp > 0)
        else {
            continue;
        };
        let x = row.position[0] + fire.aim_ft[0];
        let z = row.position[2] + fire.aim_ft[1];
        // The burst's aim: the target's offset point on the ground (the
        // target itself when the offset is nil), moving with it.
        let aim = TargetObservation {
            position: if fire.aim_ft == [0.; 2] {
                row.position
            } else {
                [x, ground(x, z).max(row.position[1] - 50.), z]
            },
            velocity: row.velocity,
        };
        let launcher = crate::combat::launcher(actor.flight());
        let Some(guns) = gunship::State::new(config) else {
            continue;
        };
        for (slot, station) in guns.stations.iter().enumerate() {
            let Some(station) = *station else {
                continue;
            };
            let w = &config.stations[station].weapon;
            // The fast gun fires in bursts; the slow ones every round due.
            if !live::ai_gun_due(w, tick) || (!fire.held && !live::ai_gun_slow(w)) {
                continue;
            }
            let Ok(train) = live::ai_gun_train(slot, w, &launcher, aim, state.tick(), &ground)
            else {
                continue;
            };
            if !wings
                .mission
                .actor_mut(id)
                .is_some_and(|a| a.debit_gun_round(StationId(station as u8)))
            {
                continue;
            }
            let shot = wings.next_projectile_id;
            let ordinal = wings.gun_ordinals.entry((id, station as u8)).or_default();
            let round = AiGunRound {
                id: shot,
                aircraft: id,
                config,
                station,
                slot,
                launcher,
                train,
                ordinal: *ordinal,
            };
            if !state.release_ai_gun_round(round, &ground) {
                wings.dropped_launches += 1;
                continue;
            }
            *ordinal += 1;
            wings.next_projectile_id = wings.next_projectile_id.wrapping_add(1);
            wings.ai_shots.insert(shot, id);
            wings.realised_launches += 1;
            aim_latest(state, 1, fire.target);
        }
    }
}

/// The gunship's linked guns as strike stores, for its tactics: each gun
/// with its rounds and its zone-1 range (the reach the orbit is planned
/// from).
pub(super) fn gun_stores(
    config: &live::Configuration,
    specs: &[tore_sim::ai::mission::StationSpec],
) -> Vec<tore_sim::ai::strike::Store> {
    let Some(guns) = gunship::State::new(config) else {
        return Vec::new();
    };
    guns.stations
        .iter()
        .flatten()
        .filter_map(|&index| {
            let w = &config.stations.get(index)?.weapon;
            let spec = specs.iter().find(|s| usize::from(s.station.0) == index)?;
            let rounds = match spec.store.rounds {
                tore_sim::ai::weapon_service::Rounds::Finite(n) => n,
                tore_sim::ai::weapon_service::Rounds::Unlimited => u32::MAX,
            };
            let zone = w.seeker.zones[1];
            Some(tore_sim::ai::strike::Store {
                station: StationId(index as u8),
                kind: StoreKind::Gun,
                rounds: if spec.store.inhibited { 0 } else { rounds },
                cadence_ticks: tore_sim::ai::QUARTER_SECOND_TICKS,
                fire_range_ft: f64::from(zone.maximum_range.max(0)),
                minimum_range_ft: f64::from(zone.minimum_range.max(0)),
                maximum_range_ft: f64::from(zone.maximum_range.max(0)),
                round_speed_fps: Some(f64::from(w.movement.initial_speed)),
                release_speed_share: 1.,
                bomb_body: Default::default(),
                fuze_arm_s: 0.,
                frag_radius_ft: 0.,
                falls_free: false,
                flight: None,
            })
        })
        .collect()
}
