//! Slice F2-O1's client tests: a game watches a mission on the network
//! simulator, a bot flying in it. Synthetic resources.

use super::observe::{CAMERA_INTERVAL, ObserverFrame};
use super::tests::{Rig, level_script, spec};
use super::*;
use crate::host::{OpenPlanes, StartMode};
use crate::wire::from_world::NO_PLANE;
use crate::wire::messages::{Subject, kind};
use tore_net::sim::LinkConfig;

const MS: Duration = Duration::from_millis(1);

/// A dedicated server flying from the start, a bot in plane 0 and a game
/// that drives its own lobby, ready to watch.
fn rig() -> (Rig, usize, usize) {
    let mut rig = Rig::with_config(
        spec(2, 2, 20),
        LinkConfig::for_round_trip(40 * MS, 0., 0., 0.),
        5,
        |config| {
            config.start = StartMode::Now;
            config.open_planes = OpenPlanes::All;
        },
    );
    rig.watch = true;
    let bot = rig.join(|c| c.plane = Some(0), level_script());
    let owl = rig.join(
        |c| {
            c.callsign = "Owl".into();
            c.auto_ready = false;
        },
        Box::new(|_, _, _| Controls::default()),
    );
    assert!(rig.run_until(Duration::from_secs(5), |r| {
        r.seated(bot)
            && r.players[owl]
                .client
                .lobby()
                .is_some_and(|l| l.phase == LobbyPhase::Flying)
    }));
    (rig, bot, owl)
}

/// Runs `time`, taking the observer's frames every 16 ms.
fn watch_for(rig: &mut Rig, owl: usize, time: Duration) -> Vec<ObserverFrame> {
    let mut frames = Vec::new();
    let end = rig.net.now() + time;
    let mut last = rig.net.now();
    while rig.net.now() < end {
        rig.step();
        let now = rig.net.now();
        if now - last >= Duration::from_millis(16) {
            last = now;
            frames.extend(rig.players[owl].client.observer_frame(now));
        }
    }
    frames
}

#[test]
fn a_game_watches_the_mission_in_real_plane_ids_and_stops() {
    let (mut rig, _, owl) = rig();
    rig.players[owl].client.watch(Subject::Aircraft(0));
    assert_eq!(
        rig.players[owl].client.watching().unwrap().sent(),
        Some(Subject::Aircraft(0))
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.players[owl]
            .client
            .watching()
            .is_some_and(|w| w.flight.is_some())
    }));
    let frames = watch_for(&mut rig, owl, Duration::from_secs(5));
    let player = &rig.players[owl];
    assert!(
        player.events.iter().any(
            |e| matches!(e, ClientEvent::Observing(o) if matches!(**o, Observing::Started(_)))
        ),
        "{:?}",
        player.events
    );
    assert_eq!(player.client.phase(), ClientPhase::Lobby);
    assert!(player.client.seat().is_none(), "no plane of its own");
    let lobby = player.client.lobby().unwrap();
    assert!(lobby.me().unwrap().observing);
    assert!(frames.len() > 250, "{} frames", frames.len());
    let mut checked = 0;
    for pair in frames.windows(2) {
        assert!(pair[1].render_tick >= pair[0].render_tick);
    }
    // The first half second draws what has arrived by then.
    for frame in &frames[30..] {
        assert_eq!(frame.picture.player.aircraft, None);
        assert_eq!(frame.picture.player.id, NO_PLANE);
        let aircraft: Vec<u32> = frame
            .picture
            .targets
            .iter()
            .filter(|t| t.aircraft.is_some())
            .map(|t| t.id)
            .collect();
        assert_eq!(aircraft, [0, 1, 2, 3], "every plane, by its own id");
        // Plane 0 is the bot's: drawn where the host had it.
        let pose = frame.picture.target(0).unwrap();
        if let Some(truth) = rig.truth_at(0, frame.render_tick) {
            let off = (0..3)
                .map(|i| (pose.position[i] - truth[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(off < 30., "{off} ft at tick {}", frame.render_tick);
            checked += 1;
        }
        // Only the mission's events, never a seat's own.
        assert!(frame.events.iter().all(|e| !matches!(
            e.event,
            WireEvent::Message { .. }
                | WireEvent::Radio { .. }
                | WireEvent::Release { .. }
                | WireEvent::Link(_)
        )));
    }
    assert!(checked > 220);

    // Stop: the host ends the flight and the game is back in the lobby.
    rig.players[owl].client.stop_watching();
    assert!(rig.players[owl].client.watching().is_none());
    assert!(rig.run_until(Duration::from_secs(2), |r| {
        r.players[owl]
            .events
            .iter()
            .any(|e| matches!(e, ClientEvent::Observing(o) if **o == Observing::Ended))
    }));
    rig.run(Duration::from_millis(1500));
    let now = rig.net.now();
    assert!(rig.players[owl].client.observer_frame(now).is_none());
    assert!(
        !rig.players[owl]
            .client
            .lobby()
            .unwrap()
            .me()
            .unwrap()
            .observing
    );
}

#[test]
fn the_camera_is_sent_again_only_when_it_moves_and_at_most_twice_a_second() {
    let (mut rig, _, owl) = rig();
    let sent = |rig: &Rig| rig.players[owl].client.watching().and_then(|w| w.sent());
    let client = &mut rig.players[owl].client;
    client.watch(Subject::Point([0, 10_000, 0]));
    assert_eq!(sent(&rig), Some(Subject::Point([0, 10_000, 0])));
    // A mile away within the interval: not sent, and not sent later.
    rig.players[owl]
        .client
        .watch(Subject::Point([6_000, 10_000, 0]));
    rig.run(CAMERA_INTERVAL + 50 * MS);
    assert_eq!(sent(&rig), Some(Subject::Point([0, 10_000, 0])));
    // Three miles away: sent at once, the interval having passed.
    rig.players[owl]
        .client
        .watch(Subject::Point([18_300, 10_000, 0]));
    assert_eq!(sent(&rig), Some(Subject::Point([18_300, 10_000, 0])));
    // Another subject at once waits for the interval, then goes.
    rig.players[owl].client.watch(Subject::Aircraft(1));
    assert_eq!(sent(&rig), Some(Subject::Point([18_300, 10_000, 0])));
    rig.run(CAMERA_INTERVAL + 50 * MS);
    assert_eq!(sent(&rig), Some(Subject::Aircraft(1)));
    assert!(
        !rig.players[owl]
            .events
            .iter()
            .any(|e| matches!(e, ClientEvent::Refused { .. })),
        "{:?}",
        rig.players[owl].events
    );
}

#[test]
fn a_refused_watch_leaves_the_game_not_watching() {
    // The mission waits in the lobby: watching is refused.
    let mut rig = Rig::with_config(
        spec(2, 2, 20),
        LinkConfig::for_round_trip(40 * MS, 0., 0., 0.),
        5,
        |_| {},
    );
    let owl = rig.join(
        |c| c.auto_ready = false,
        Box::new(|_, _, _| Controls::default()),
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.players[owl].client.lobby().is_some()
    }));
    rig.players[owl].client.watch(Subject::None);
    assert!(rig.players[owl].client.watching().is_some());
    assert!(rig.run_until(Duration::from_secs(1), |r| {
        r.players[owl]
            .events
            .iter()
            .any(|e| matches!(e, ClientEvent::Refused { request, .. } if *request == kind::OBSERVE))
    }));
    assert!(rig.players[owl].client.watching().is_none());
    let now = rig.net.now();
    assert!(rig.players[owl].client.observer_frame(now).is_none());
}

#[test]
fn the_delayed_watch_countdown_uses_the_live_deadline() {
    let mut rig = Rig::with_config(
        spec(2, 1, 150),
        LinkConfig::for_round_trip(40 * MS, 0., 0., 0.),
        35,
        |c| {
            c.start = StartMode::Now;
            c.time_limit = Some(Duration::from_secs(360));
            c.settings = vec![
                (crate::settings::number::MODE, 1),
                (crate::settings::number::OBSERVER_DELAY, 10),
            ];
        },
    );
    let pilot = rig.join(|c| c.plane = Some(0), level_script());
    let owl = rig.join(
        |c| {
            c.auto_ready = false;
            c.callsign = "Owl".into();
        },
        level_script(),
    );
    assert!(rig.run_until(Duration::from_secs(5), |r| r.seated(pilot)
        && r.players[owl].client.phase() == ClientPhase::Lobby));
    rig.players[owl].client.watch(Subject::Aircraft(0));
    let frames = watch_for(&mut rig, owl, Duration::from_millis(14500));
    assert_eq!(frames.last().unwrap().flight.delay_seconds, 10);
    let expected = super::scores::remaining_seconds(Some(360), rig.host.world().tick());
    assert_eq!(rig.players[pilot].client.seconds_left(), expected);
    assert_eq!(rig.players[owl].client.seconds_left(), expected);
}
