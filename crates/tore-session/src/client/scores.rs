//! The scores a player's game keeps (stage F phase 2, slice F2-S;
//! docs/ARCHITECTURE.md, "Scoring"): the newest Scores message the host
//! sent, with the time left derived from the host mission clock, and the words the
//! score board and the bots use for them.

use super::Client;
use crate::settings::{KillOwner, ScoreTally};
use crate::wire::messages::{Scores, Winner};
use std::time::Duration;
use tore_sim::ai::launch::Side;

/// The newest scores from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Kept {
    pub scores: Scores,
}

impl Client {
    /// The newest scores the host sent for the mission flying, if any yet.
    pub fn scores(&self) -> Option<&Scores> {
        self.scores.as_ref().map(|kept| &kept.scores)
    }

    /// The authoritative lobby time limit, including a server rotation override.
    pub fn time_limit_seconds(&self) -> Option<u32> {
        self.lobby
            .as_ref()?
            .settings
            .iter()
            .find(|(number, _)| *number == crate::settings::number::TIME_LIMIT)
            .map(|(_, seconds)| *seconds)
            .filter(|seconds| *seconds != 0)
    }

    /// Remaining mission time, rounded up from the estimated present host tick.
    /// No per-client stopwatch: late joins, rejoin and migration share the same
    /// mission tick. Delayed observers add the stream's advertised delay.
    pub fn seconds_left(&self) -> Option<u32> {
        let limit = self.time_limit_seconds()?;
        if let Some(kept) = &self.scores
            && (kept.scores.winner != Winner::NoneYet || kept.scores.seconds_left == Some(0))
        {
            return kept.scores.seconds_left;
        }
        let round_trip = self.net.stats().map_or(Duration::ZERO, |s| s.round_trip);
        let tick = self.render_clock.present(self.now, round_trip)?;
        let delay = self
            .watching()
            .and_then(|w| w.flight)
            .map_or(0, |flight| u64::from(flight.delay_seconds) * 120);
        remaining_seconds(Some(limit), (tick.floor() as u64).saturating_add(delay))
    }
}

/// The host's deadline starts at mission tick zero, at 120 ticks per second.
/// Round up so 00:00 first appears on the tick that ends the mission.
pub fn remaining_seconds(limit: Option<u32>, tick: u64) -> Option<u32> {
    limit.map(|seconds| {
        (u64::from(seconds) * 120)
            .saturating_sub(tick)
            .div_ceil(120) as u32
    })
}

/// The score board's heading for a tally, retail's three.
pub fn heading(tally: ScoreTally) -> &'static str {
    match tally {
        ScoreTally::Kills => "PLAYERS RANKED BY KILLS",
        ScoreTally::Ratio => "PLAYERS RANKED BY KILL RATIO",
        ScoreTally::Damage => "PLAYERS RANKED BY TOTAL DAMAGE",
    }
}

/// A side's name in a sentence.
pub fn side_name(side: Side) -> &'static str {
    match side {
        Side::Friendly => "friendly",
        Side::Enemy => "enemy",
    }
}

/// Kills over losses, or kills alone with no loss, as the host ranks them.
pub fn ratio(kills: u32, losses: u32) -> f64 {
    if losses == 0 {
        f64::from(kills)
    } else {
        f64::from(kills) / f64::from(losses)
    }
}

/// Minutes and seconds: "9:05".
pub fn clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The kill limit in words, when there is one: "First side to 5 kills".
pub fn limit_text(scores: &Scores) -> Option<String> {
    let limit = scores.kill_limit;
    if limit == 0 {
        return None;
    }
    let kills = if limit == 1 { "kill" } else { "kills" };
    Some(match scores.kill_owner {
        KillOwner::Total => format!("Ends at {limit} {kills} in all"),
        KillOwner::Side => format!("First side to {limit} {kills}"),
        KillOwner::Player => format!("First player to {limit} {kills}"),
    })
}

/// Who won, in words, once the host has said: "The friendly side wins",
/// "Hawk wins", "A draw".
pub fn winner_text(scores: &Scores) -> Option<String> {
    match scores.winner {
        Winner::NoneYet => None,
        Winner::Side(side) => Some(format!("The {} side wins", side_name(side))),
        Winner::Player(id) => Some(scores.players.iter().find(|p| p.id == id).map_or_else(
            || "A player who left wins".to_owned(),
            |p| format!("{} wins", p.callsign),
        )),
        Winner::Aircraft(root) => Some(
            scores
                .players
                .iter()
                .find(|p| p.ai == Some(root))
                .map_or_else(
                    || "An AI aircraft wins".into(),
                    |p| format!("{} (AI) wins", p.callsign),
                ),
        ),
        Winner::Draw => Some("A draw".to_owned()),
    }
}

/// One line for a log or a bot's output: the heading, each player in order
/// with its side, kills, losses and damage, the sides, the time left and the
/// winner.
pub fn summary(scores: &Scores) -> String {
    let players: Vec<String> = scores
        .players
        .iter()
        .enumerate()
        .map(|(n, p)| {
            format!(
                "{} {}{} {}/{} {:.2}",
                n + 1,
                if p.ai.is_some() {
                    format!("{} (AI)", p.callsign)
                } else if p.left {
                    format!("{} (left)", p.callsign)
                } else {
                    p.callsign.clone()
                },
                p.side
                    .map_or(String::new(), |s| format!(" ({})", side_name(s))),
                p.kills,
                p.losses,
                f64::from(p.damage) / 1000.
            )
        })
        .collect();
    let [friendly, enemy] = scores.sides;
    let mut line = format!(
        "{}: {}; sides {}/{} to {}/{}",
        heading(scores.tally).to_lowercase(),
        if players.is_empty() {
            "nobody".to_owned()
        } else {
            players.join(", ")
        },
        friendly.kills,
        friendly.losses,
        enemy.kills,
        enemy.losses,
    );
    if let Some(left) = scores.seconds_left {
        line.push_str(&format!("; {} left", clock(left)));
    }
    if let Some(limit) = limit_text(scores) {
        line.push_str(&format!("; {}", limit.to_lowercase()));
    }
    if let Some(winner) = winner_text(scores) {
        line.push_str(&format!("; {}", winner.to_lowercase()));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Fight;
    use crate::wire::messages::{PlayerScore, SideScore};

    fn scores() -> Scores {
        Scores {
            tally: ScoreTally::Kills,
            fight: Fight::Sides,
            seconds_left: Some(545),
            kill_limit: 5,
            kill_owner: KillOwner::Side,
            players: vec![
                PlayerScore {
                    hits: 0,
                    ai: None,
                    left: false,
                    id: 2,
                    callsign: "Hawk".into(),
                    side: Some(Side::Enemy),
                    kills: 3,
                    losses: 1,
                    damage: 2_250,
                },
                PlayerScore {
                    hits: 0,
                    ai: None,
                    left: false,
                    id: 0,
                    callsign: "Viper".into(),
                    side: None,
                    kills: 0,
                    losses: 0,
                    damage: 0,
                },
            ],
            sides: [
                SideScore::default(),
                SideScore {
                    hits: 0,
                    kills: 3,
                    losses: 1,
                    damage: 2_250,
                },
            ],
            winner: Winner::NoneYet,
        }
    }

    #[test]
    fn the_deadline_rounds_up_and_has_no_timer_without_a_limit() {
        for (tick, expected) in [
            (0, 360),
            (119, 360),
            (120, 359),
            (7199, 301),
            (7200, 300),
            (43199, 1),
            (43200, 0),
            (u64::MAX, 0),
        ] {
            assert_eq!(remaining_seconds(Some(360), tick), Some(expected));
            assert_eq!(remaining_seconds(None, tick), None);
        }
    }

    #[test]
    fn the_words_for_scores() {
        let mut scores = scores();
        assert_eq!(
            summary(&scores),
            "players ranked by kills: 1 Hawk (enemy) 3/1 2.25, 2 Viper 0/0 0.00; \
             sides 0/0 to 3/1; 9:05 left; first side to 5 kills"
        );
        assert_eq!(heading(ScoreTally::Ratio), "PLAYERS RANKED BY KILL RATIO");
        assert_eq!(
            heading(ScoreTally::Damage),
            "PLAYERS RANKED BY TOTAL DAMAGE"
        );
        scores.kill_limit = 1;
        scores.kill_owner = KillOwner::Total;
        assert_eq!(limit_text(&scores).unwrap(), "Ends at 1 kill in all");
        scores.kill_owner = KillOwner::Player;
        assert_eq!(limit_text(&scores).unwrap(), "First player to 1 kill");
        scores.kill_limit = 0;
        assert_eq!(limit_text(&scores), None);
        for (winner, text) in [
            (Winner::Side(Side::Friendly), "The friendly side wins"),
            (Winner::Player(2), "Hawk wins"),
            (Winner::Player(9), "A player who left wins"),
            (Winner::Draw, "A draw"),
        ] {
            scores.winner = winner;
            assert_eq!(winner_text(&scores).unwrap(), text);
        }
        assert_eq!(ratio(3, 0), 3.);
        assert_eq!(ratio(3, 2), 1.5);
        assert_eq!(clock(60), "1:00");
    }
}

/// The real client against the host on the network simulator: the scores
/// arrive, are kept, count their time down and start afresh with the next
/// flight.
#[cfg(test)]
mod rig_tests {
    use super::super::tests::{Rig, spec, weave};
    use super::*;
    use crate::client::{ClientEvent, ClientPhase};
    use crate::host::{AfterEnd, HostConfig};
    use tore_net::sim::LinkConfig;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn late_join_and_rejoin_use_the_ongoing_host_deadline() {
        let mut rig = Rig::with_config(
            spec(3, 1, 150),
            LinkConfig::for_round_trip(40 * MS, 0., 0., 0.),
            29,
            |c| {
                c.time_limit = Some(Duration::from_secs(360));
            },
        );
        let first = rig.join(|_| {}, super::super::tests::level_script());
        assert!(rig.run_until(Duration::from_secs(5), |r| r.seated(first)));
        rig.run(Duration::from_millis(2500));
        let late = rig.join(
            |c| c.callsign = "Late".into(),
            super::super::tests::level_script(),
        );
        assert!(rig.run_until(Duration::from_secs(5), |r| r.seated(late)));
        rig.run(Duration::from_millis(500));
        let expected = remaining_seconds(Some(360), rig.host.world().tick());
        assert_eq!(rig.players[first].client.seconds_left(), expected);
        assert_eq!(rig.players[late].client.seconds_left(), expected);
        let token = rig.players[first].client.token().unwrap().token;
        rig.players[first].stalled = true;
        rig.run(Duration::from_secs(6));
        let back = rig.join(
            |c| c.token = Some(token),
            super::super::tests::level_script(),
        );
        assert!(rig.run_until(Duration::from_secs(5), |r| r.seated(back)));
        rig.run(Duration::from_millis(500));
        let expected = remaining_seconds(Some(360), rig.host.world().tick());
        assert_eq!(rig.players[back].client.seconds_left(), expected);
        assert_eq!(rig.players[late].client.seconds_left(), expected);
        // Delay/jitter aside, the estimate follows the host within one tick.
        let c = &rig.players[back].client;
        let tick = c
            .render_clock
            .present(c.now, c.net.stats().unwrap().round_trip)
            .unwrap();
        assert!(
            (tick - rig.host.world().tick() as f64).abs() <= 1.,
            "{tick} vs {}",
            rig.host.world().tick()
        );
    }

    #[test]
    fn an_unlimited_flight_has_no_client_countdown() {
        let mut rig = Rig::with_config(spec(1, 1, 150), LinkConfig::one_way(10 * MS), 30, |_| {});
        let player = rig.join(|_| {}, super::super::tests::level_script());
        assert!(rig.run_until(Duration::from_secs(5), |r| r.seated(player)));
        rig.run(Duration::from_secs(1));
        assert_eq!(rig.players[player].client.seconds_left(), None);
    }

    #[test]
    fn the_client_keeps_the_newest_scores_and_counts_the_time_down() {
        let mut rig = Rig::with_config(
            spec(2, 2, 50),
            LinkConfig::for_round_trip(40 * MS, 0., 0., 0.),
            5,
            |config: &mut HostConfig| {
                config.time_limit = Some(Duration::from_secs(6));
                config.after_end = AfterEnd::Restart;
                config.restart_delay = Duration::from_secs(1);
            },
        );
        let viper = rig.join(|_| {}, Box::new(|now, _, _| weave(now.as_secs_f64())));
        let hawk = rig.join(
            |c| c.callsign = "Hawk".into(),
            Box::new(|now, _, _| weave(now.as_secs_f64())),
        );
        assert!(rig.run_until(Duration::from_secs(4), |r| {
            [viper, hawk]
                .iter()
                .all(|&p| r.players[p].client.scores().is_some())
        }));
        let client = &rig.players[viper].client;
        let scores = client.scores().unwrap();
        let mut callsigns: Vec<&str> = scores
            .players
            .iter()
            .filter(|p| p.ai.is_none())
            .map(|p| p.callsign.as_str())
            .collect();
        callsigns.sort_unstable();
        assert_eq!(callsigns, ["Hawk", "Viper"]);
        assert_eq!(scores.winner, Winner::NoneYet);
        let left = client.seconds_left().unwrap();
        assert!(left <= 6, "{left}");
        // Nothing changes, so nothing more arrives: the client counts down.
        rig.run(Duration::from_millis(2000));
        let later = rig.players[viper].client.seconds_left().unwrap();
        assert!(
            later + 2 <= left && later + 3 >= left,
            "{left} then {later}"
        );
        // The time limit ends the mission; the final scores came with it.
        assert!(rig.run_until(Duration::from_secs(6), |r| {
            r.players[viper]
                .events
                .iter()
                .any(|e| matches!(e, ClientEvent::MissionEnded(_)))
        }));
        let finals = rig.players[viper]
            .events
            .iter()
            .filter(|e| matches!(e, ClientEvent::Scores(_)))
            .count();
        assert!(finals >= 2, "the first scores and the final ones");
        assert_eq!(rig.players[viper].client.seconds_left(), Some(0));
        // The next flight starts its scores afresh.
        assert!(rig.run_until(Duration::from_secs(4), |r| {
            r.players[viper].client.phase() == ClientPhase::Flying && r.host.world().tick() > 0
        }));
        assert!(
            rig.players[viper]
                .client
                .scores()
                .is_none_or(|s| s.seconds_left.is_some_and(|left| left > 3))
        );
    }
}
