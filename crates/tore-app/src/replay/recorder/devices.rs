//! Released chaff and flares: one `combat.countermeasure` entry per device,
//! exact enough for a replay to fly the device again, and the player's own
//! decoy rolls, which explain the shots they decoyed. The AI's rolls come
//! from the bridge in [`super::why`]. Everything here drains write-only
//! records; nothing feeds back. Opinionated addition requested by John on
//! 2026-09-26; the entries' layout is an agent decision. See
//! docs/REPLAYS.md.

use super::Recorder;
use crate::ai_wings::{AiWings, DecoyRoll};
use crate::combat::Combat;
use crate::replay::convert;
use std::collections::BTreeMap;
use tore_replay::{Event, vocab::kind};
use tore_sim::ai::threat::SeekerClass;
use tore_sim::combat::live::{self, DeviceNote};

/// Chaff cartridges and flares each AI aircraft has left, by aircraft and
/// whether they are flares.
pub(super) type Dispensers = BTreeMap<(u32, bool), u32>;

/// What every AI aircraft's dispensers hold now.
pub(super) fn dispensers(wings: Option<&AiWings>) -> Dispensers {
    wings
        .into_iter()
        .flat_map(|wings| wings.mission().actors())
        .flat_map(|actor| {
            actor.dispensers().iter().map(move |store| {
                (
                    (actor.id(), store.class == SeekerClass::Infrared),
                    store.count,
                )
            })
        })
        .collect()
}

/// The seeker class a device decoys: chaff radar, flares infrared.
fn class(kind: live::EffectKind) -> SeekerClass {
    if kind == live::EffectKind::Chaff {
        SeekerClass::Radar
    } else {
        SeekerClass::Infrared
    }
}

/// The player's roll as the AI's rolls are kept, for the same reasons,
/// guidance trees and `weapon.decoyed` entries.
fn player_roll(roll: &live::DecoyRoll) -> DecoyRoll {
    DecoyRoll {
        projectile: roll.projectile,
        releaser: roll.aircraft,
        class: class(roll.kind),
        device: roll.device,
        susceptibility: roll.susceptibility,
        effectiveness: roll.effectiveness,
        draw: Some(tore_sim::ai::Draw {
            site: Some("decoy roll"),
            location: std::panic::Location::caller(),
            bound: u32::from(roll.bound),
            value: u32::from(roll.roll),
            offset: 0,
            threshold: Some(u32::from(roll.threshold)),
        }),
        decoyed: roll.decoyed,
    }
}

/// Devices of its kind each release left its aircraft with: the player's as
/// combat noted them, an AI aircraft's from what its dispensers hold now,
/// counting back over its later releases among `notes`.
fn devices_left(notes: &[DeviceNote], dispensers: &Dispensers) -> Vec<Option<u32>> {
    let key = |device: &live::DeviceRelease| (device.owner, device.kind == live::EffectKind::Flare);
    let mut later: BTreeMap<(u32, bool), u32> = BTreeMap::new();
    for note in notes {
        if let DeviceNote::Released(device) = note
            && device.left.is_none()
        {
            *later.entry(key(device)).or_default() += 1;
        }
    }
    notes
        .iter()
        .map(|note| match note {
            DeviceNote::Released(device) => match device.left {
                Some(left) => Some(u32::from(left)),
                None => {
                    let after = later.get_mut(&key(device)).map_or(0, |count| {
                        *count -= 1;
                        *count
                    });
                    dispensers.get(&key(device)).map(|count| count + after)
                }
            },
            DeviceNote::Cleared(_) => None,
        })
        .collect()
}

impl Recorder {
    /// The devices released, any range reset, and the player's decoy rolls
    /// since the last look. An AI aircraft's devices left come from
    /// [`dispensers`] as its tick was recorded.
    pub(super) fn countermeasures(&mut self, combat: &mut Combat) {
        let notes = combat.state.take_device_notes();
        let lefts = devices_left(&notes, &self.dispensers);
        for (note, left) in notes.into_iter().zip(lefts) {
            let event = match note {
                DeviceNote::Released(device) => convert::device_event(
                    &convert::DeviceRelease {
                        owner: device.owner,
                        kind: device.kind,
                        release: device.release,
                        number: device.number,
                        tick: device.tick,
                    },
                    left,
                ),
                DeviceNote::Cleared(tick) => Event::new(kind::COMBAT_COUNTERMEASURES_CLEARED)
                    .with(tore_replay::vocab::field::AFTER_TICK, tick as i64)
                    .with_text("a range reset removed every chaff cloud and flare"),
            };
            self.note(event);
        }
        for roll in combat.state.take_decoy_rolls() {
            let roll = player_roll(&roll);
            if roll.decoyed {
                let owner = self.shots.get(&roll.projectile).map(|shot| shot.owner);
                let event = super::why::decoyed_event(&roll, owner);
                self.note(event);
            }
            // The next tick's reasons and guidance trees read it.
            self.why.pending_rolls.push(roll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_release_says_how_many_its_aircraft_had_left() {
        let release = |owner, kind, left| {
            DeviceNote::Released(live::DeviceRelease {
                owner,
                kind,
                release: tore_sim::combat::countermeasures::Release {
                    position: [0.; 3],
                    velocity: [0.; 3],
                    basis: tore_sim::attitude::Basis::new(0., 0., 0.),
                },
                number: 1,
                tick: 0,
                left,
            })
        };
        use live::EffectKind::{Chaff, Flare};
        let notes = [
            release(7, Flare, None),
            release(0, Chaff, Some(3)),
            release(7, Flare, None),
            DeviceNote::Cleared(0),
            release(7, Chaff, None),
            release(9, Flare, None),
        ];
        // Aircraft 7 now holds 4 flares after releasing two, and 1 chaff.
        let dispensers = Dispensers::from([((7, true), 4), ((7, false), 1)]);
        assert_eq!(
            devices_left(&notes, &dispensers),
            [Some(5), Some(3), Some(4), None, Some(1), None]
        );
    }

    #[test]
    fn fine_chaff_chances_keep_their_units_in_recordings() {
        for (value, succeeded) in [(129, true), (130, false)] {
            let roll = player_roll(&live::DecoyRoll {
                aircraft: 0,
                projectile: 12,
                kind: live::EffectKind::Chaff,
                device: 9,
                susceptibility: 50,
                effectiveness: 35,
                threshold: 130,
                bound: 10000,
                roll: value,
                decoyed: succeeded,
            });
            let draw = roll.draw.unwrap();
            assert_eq!(draw.bound, 10000);
            assert_eq!(draw.passed(), Some(succeeded));
            let event = super::super::why::decoyed_event(&roll, Some(1));
            assert_eq!(
                event.num(tore_replay::vocab::field::ROLL_BOUND),
                Some(10000.)
            );
            assert_eq!(event.num(tore_replay::vocab::field::THRESHOLD), Some(130.));
        }
    }
    #[test]
    fn a_players_roll_reads_as_the_ai_rolls_do() {
        let roll = player_roll(&live::DecoyRoll {
            aircraft: 0,
            projectile: 12,
            kind: live::EffectKind::Flare,
            device: 9,
            susceptibility: 70,
            effectiveness: 60,
            threshold: 42,
            bound: 100,
            roll: 17,
            decoyed: true,
        });
        assert_eq!(
            (roll.projectile, roll.releaser, roll.class, roll.decoyed),
            (12, 0, SeekerClass::Infrared, true)
        );
        let draw = roll.draw.unwrap();
        assert_eq!(draw.passed(), Some(true));
        assert_eq!(
            crate::replay::trees::draw_text(&draw),
            "roll 17 < 42: decoy roll"
        );
    }
}
