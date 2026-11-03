//! Where the server meets the host session: the one function that builds the
//! real [`Host`], `tore_session::Host`, and the adapter that lets the run loop
//! drive it. The host reads no clock and opens no socket; this adapter hands it
//! the time and the bound sockets.
//!
//! It also holds the server's listing on the Internet Lobby
//! (`tore_net::master::HostListing`, slice I3): the socket is read and
//! written through it, so the master's datagrams never reach the host, and
//! it lists the server while `broadcast` is on. A broadcasting server sends
//! the master a Report at the end of each mission when `telemetry` is on.
//!
//! With `port-mapping on` (slice J4b) it also asks the router to forward the
//! game port from the start (`tore_net::portmap::keeper`), gives the listing
//! the Mapped candidate, logs what the router did, and removes the mapping
//! when the server stops. *Agent decision:* the real router is asked only
//! through [`mapping_choice`], which is never the real one in a test build,
//! so no test of the server can reach a router; tests give
//! [`start_host_with`] a fake gateway.

use crate::{
    app::is_release,
    config::{AfterEnd, OpenPlanes, StartMode},
    host::{Event, Host, HostSetup, PlayerFigures, Status, Time},
};
use std::{collections::VecDeque, time::Duration};
use tore_net::master::{
    Build, HostListing, HostRendezvous, HostTally, ListingState, MappingType, PortMapping,
    RendezvousEvent, Role,
};
use tore_net::portmap::keeper::{self, Keeper, KeeperConfig};
use tore_net::{Entropy, Pace, Platform, ServerSocket};
use tore_session::wire::chat::receiver_label;
use tore_session::{BuildId, HostConfig, HostLog, LeaveReason, Phase};

/// The AI test settings the command line asked for (`--ai-observe-hz`,
/// `--ai-lean-sensors`), kept for the host the program builds. Unset, which a
/// test build always is, they are today's behaviour.
static AI_TEST: std::sync::OnceLock<tore_session::AiTestSettings> = std::sync::OnceLock::new();

/// Records the command line's AI test settings; only the first call counts.
pub fn set_ai_test(settings: tore_session::AiTestSettings) {
    let _ = AI_TEST.set(settings);
}

/// The mapper the configuration asks for: the real router when
/// `port-mapping` is on (and `TORE_NO_PORT_MAPPING` is not set), never in a
/// test build.
pub fn mapping_choice(port_mapping: bool) -> Option<KeeperConfig> {
    if cfg!(test) {
        return None;
    }
    keeper::choose(port_mapping)
}

/// Builds the host session for a prepared mission.
pub fn start_host(setup: HostSetup) -> Result<Box<dyn Host>, String> {
    let mapping = mapping_choice(setup.config.port_mapping);
    start_host_with(setup, mapping)
}

/// [`start_host`] with the router asked as `mapping` says (a test's fake
/// gateway), whatever the configuration's `port-mapping` is.
pub fn start_host_with(
    setup: HostSetup,
    mapping: Option<KeeperConfig>,
) -> Result<Box<dyn Host>, String> {
    let HostSetup {
        config: c,
        rotation,
        spec,
        resources,
        socket,
        version,
        commit,
        install_id,
    } = setup;
    let port = socket.local_addresses().first().map_or(0, |a| a.port());
    let rendezvous = HostRendezvous {
        build: Build {
            protocol_version: tore_session::wire::PROTOCOL_VERSION,
            game_version: version.clone(),
            game_commit: commit.clone(),
            compatibility: tore_session::compatibility::current(
                &tore_session::host::content::GameContent::shared(&resources).content,
            ),
            release: is_release(),
        },
        dedicated: true,
        install_id,
        platform: Platform::current().code(),
        entropy: Entropy::System,
    };
    let mut listing = HostListing::new(&c.master, rendezvous, port, Duration::ZERO)?;
    listing.set_listed(c.broadcast, Duration::ZERO);
    let mut config = HostConfig::new(BuildId {
        compatibility: tore_session::compatibility::current(
            &tore_session::host::content::GameContent::shared(&resources).content,
        ),
        version,
        commit,
        release: is_release(),
    });
    config.feedback = c.feedback;
    config.recording = c.record_replays.then(|| tore_session::recording::Options {
        folder: c.replays_folder.unwrap_or_else(|| "replays".into()),
        kept: c.replays_kept,
    });
    config.name = c.name;
    config.password = c.password;
    config.max_players = usize::from(c.max_players);
    config.open_planes = match c.open_planes {
        OpenPlanes::Friendly => tore_session::OpenPlanes::Friendly,
        OpenPlanes::All => tore_session::OpenPlanes::All,
        OpenPlanes::List(planes) => tore_session::OpenPlanes::List(planes),
    };
    config.snapshot_rate = c.snapshot_rate;
    config.start = match c.start {
        StartMode::FirstPlayer => tore_session::StartMode::FirstPlayer,
        StartMode::Now => tore_session::StartMode::Now,
    };
    config.time_limit = (c.time_limit_minutes > 0)
        .then(|| Duration::from_secs(u64::from(c.time_limit_minutes) * 60));
    config.empty_timeout = Duration::from_secs(u64::from(c.empty_timeout_seconds));
    config.after_end = match c.after_end {
        AfterEnd::Restart => tore_session::AfterEnd::Restart,
        AfterEnd::Quit => tore_session::AfterEnd::Quit,
    };
    config.restart_delay = Duration::from_secs(u64::from(c.restart_delay_seconds));
    // Stage F phase 2 (slice F2-1): the King's settings the file gives, and
    // a King when it says `king first-player`. The listing stays the
    // operator's `broadcast`, so the King's `public` is refused.
    config.settings = c.settings;
    config.crown = if c.king {
        tore_session::CrownRule::FirstPlayer
    } else {
        tore_session::CrownRule::None
    };
    config.mission_locked = c.king_mission_locked;
    // No screen draws a dedicated server's world: its contrails are the
    // clients' own (optimization slice S11; John, 2026-10-19).
    config.contrail_puffs = false;
    config.listable = false;
    config.entropy = Entropy::System;
    // The program has already refused the switch; the host refuses it too.
    config.retail_stall_speeds = false;
    config.ai_test = AI_TEST.get().copied().unwrap_or_default();
    let mut host = tore_session::Host::new(spec, resources, config)
        .map_err(|error| format!("The host session cannot start: {error}"))?;
    let rotation = rotation.map(|r| std::sync::Arc::new(std::sync::Mutex::new(r)));
    if let Some(rotation) = &rotation {
        host.set_mission_sequence(Box::new(crate::rotation::Shared(rotation.clone())));
    }
    // Protocol 28 (slice NET2): while the server builds a mission its
    // loading thread keeps every player connected, on a clone of its socket.
    if let Ok(base) = socket.try_clone() {
        host.set_loading_socket(Box::new(move || {
            Ok(Box::new(base.try_clone()?) as Box<dyn tore_net::Datagrams + Send>)
        }));
    }
    // Stage L: the server's own content, which every player's is compared
    // with (`--check` lists its items).
    let own = host.content();
    let content_line = format!(
        "Content: {}, {} items",
        own.source.describe(),
        own.content.items().len()
    );
    // After the checks above: a start that is refused asks the router nothing.
    let keeper = mapping.map(|mapping| Keeper::start(mapping, port));
    Ok(Box::new(SessionHost {
        rotation,
        host,
        socket,
        events: VecDeque::from([Event::Note(content_line)]),
        listing,
        tally: None,
        now: Duration::ZERO,
        keeper,
        port_mapping: PortMapping::NotTried,
    }))
}

struct SessionHost {
    rotation: Option<std::sync::Arc<std::sync::Mutex<crate::rotation::Rotation>>>,
    host: tore_session::Host,
    socket: ServerSocket,
    events: VecDeque<Event>,
    /// The listing on the Internet Lobby, listed while `broadcast` is on.
    listing: HostListing,
    /// The flying mission's figures for its Report.
    tally: Option<HostTally>,
    /// The time of the last poll.
    now: Time,
    /// The router's port mapping, when `port-mapping` is on.
    keeper: Option<Keeper>,
    /// What the mapping came to, for the Report.
    port_mapping: PortMapping,
}

/// A listing event as a log line.
fn listing_text(event: RendezvousEvent, master: &str) -> String {
    match event {
        RendezvousEvent::Listed { seen, .. } => {
            format!("Broadcasting: listed on the Internet Lobby ({master}), seen at {seen}")
        }
        RendezvousEvent::SeenChanged(seen) => {
            format!("Broadcasting: the Internet Lobby now sees this server at {seen}")
        }
        RendezvousEvent::Unlisted => "Broadcasting stopped: off the Internet Lobby".into(),
        RendezvousEvent::MasterSilent => format!(
            "Broadcasting: the Internet Lobby at {master} does not answer, so the server is not \
             listed. Players can still join by address."
        ),
        RendezvousEvent::LookupFailed(why) => {
            format!("Broadcasting: cannot find the Internet Lobby at {master}: {why}")
        }
        RendezvousEvent::Refused(text) => {
            format!("Broadcasting: the Internet Lobby refused the listing: {text}")
        }
        RendezvousEvent::MasterAddress { from, to, why } => format!(
            "Broadcasting: the Internet Lobby at {master}: now trying {to} instead of {from} ({})",
            why.text()
        ),
        RendezvousEvent::SendFailed { to, error } => {
            format!("Broadcasting: cannot send to the Internet Lobby at {to}: {error}")
        }
        RendezvousEvent::MappingTested(mapping) => format!(
            "Broadcasting: the router test says {}",
            match mapping {
                MappingType::Unknown => "nothing yet",
                MappingType::NoTranslation => "no address translation",
                MappingType::SamePort => "one outside port for every destination",
                MappingType::PortPerDestination => "a new outside port for each destination",
            }
        ),
    }
}

/// Where the listing stands, in a few words for the status line.
fn listing_short(state: &ListingState, master: &str) -> Option<String> {
    Some(match state {
        ListingState::Off => return None,
        ListingState::Listed { seen, .. } => format!("listed, seen at {seen}"),
        ListingState::Registering => "registering".into(),
        ListingState::FindingMaster => format!("looking up {master}"),
        ListingState::Silent => "the Internet Lobby does not answer".into(),
        ListingState::Refused(text) => format!("refused: {text}"),
    })
}

fn leave_text(reason: LeaveReason) -> String {
    match reason {
        LeaveReason::Left => "left".into(),
        LeaveReason::Silent => "no packet for 5 seconds".into(),
        LeaveReason::Kicked => "kicked".into(),
        LeaveReason::MissionEnded => "the mission ended".into(),
        LeaveReason::HostLeft => "the host left the game".into(),
        LeaveReason::Replaced => "replaced by a new connection from the same address".into(),
        LeaveReason::Disconnected(why) => format!("disconnected ({why:?})"),
    }
}

fn end_text(reason: tore_session::wire::messages::EndReason) -> &'static str {
    use tore_session::wire::messages::EndReason;
    match reason {
        EndReason::EveryoneLeft => "everyone left",
        EndReason::TimeLimit => "the time limit",
        EndReason::ServerStopping => "the server is stopping",
        EndReason::EndedByServer => "ended from the console",
        EndReason::HostLeft => "the host left the game",
        EndReason::KillLimit => "the kill limit",
    }
}

impl SessionHost {
    /// Takes in what the mapper thread has to say: the candidate to the
    /// listing, the lines to the log.
    fn take_mapping_news(&mut self, now: Time) {
        let Some(keeper) = &self.keeper else {
            return;
        };
        for news in keeper.poll() {
            if let keeper::News::Mapped {
                outside,
                port_mapping,
                ..
            } = &news
            {
                self.port_mapping = *port_mapping;
                self.listing.set_mapped(*outside, now);
            }
            for line in news.lines() {
                self.events
                    .push_back(Event::Note(format!("Port mapping: {line}")));
            }
        }
    }

    fn present(&mut self) {
        if let Some(tally) = self.tally.as_mut() {
            tally.present(self.host.players().into_iter().map(|p| p.address));
        }
    }

    /// The mission's Report to the master, when the server broadcasts.
    fn report_mission(&mut self) {
        let Some(tally) = self.tally.take() else {
            return;
        };
        if !self.listing.rendezvous().listed_wanted() {
            return;
        }
        let mapping = self.listing.rendezvous().mapping();
        let mut report = tally.report(
            self.now,
            Role::DedicatedServer,
            &self.host.config().build.version,
            Platform::current().code(),
            mapping,
        );
        report.port_mapping = self.port_mapping;
        self.listing.rendezvous_mut().report(report);
    }

    fn drain_log(&mut self) {
        while let Some(entry) = self.host.poll_log() {
            match &entry {
                HostLog::MissionStarted { .. } => {
                    self.tally = Some(HostTally::new(self.now));
                    self.present();
                }
                HostLog::Connected { .. } | HostLog::Left { .. } => self.present(),
                HostLog::MissionEnded { .. } => {
                    self.present();
                    self.report_mission();
                }
                _ => {}
            }
            self.events.push_back(match entry {
                HostLog::Connected {
                    address,
                    callsign,
                    path,
                    ..
                } => Event::Joined {
                    address: address.to_string(),
                    callsign,
                    path: tore_session::wire::path_words(path).to_owned(),
                },
                HostLog::Refused {
                    address,
                    callsign,
                    reason,
                    ..
                } => Event::Refused {
                    address: address.to_string(),
                    callsign: Some(callsign).filter(|c| !c.is_empty()),
                    reason,
                },
                HostLog::ContentRefused {
                    callsign, names, ..
                } => Event::Refused {
                    address: String::new(),
                    callsign: Some(callsign),
                    reason: format!("content mismatch: {}", names.join(", ")),
                },
                HostLog::SeatRefused {
                    callsign, reason, ..
                } => Event::Note(format!("{callsign} was refused a plane: {reason}")),
                HostLog::Seated {
                    seat,
                    callsign,
                    plane,
                    ..
                } => match self.host.choice_aircraft(plane) {
                    Some(aircraft) => Event::Note(format!(
                        "{callsign} took plane {plane} (Player choice): {}",
                        aircraft.short_label()
                    )),
                    None => Event::Seated {
                        seat,
                        callsign,
                        plane,
                    },
                },
                HostLog::Left {
                    seat,
                    callsign,
                    plane,
                    reason,
                    ..
                } => Event::Left {
                    seat,
                    callsign,
                    plane,
                    reason: leave_text(reason),
                },
                HostLog::MissionStarted { .. } => Event::MissionStarted,
                HostLog::MissionEnded { reason, .. } => Event::MissionEnded {
                    reason: end_text(reason).into(),
                },
                HostLog::MissionRestarted { .. } => Event::MissionRestarted,
                HostLog::Overloaded { ticks_behind, .. } => Event::Note(format!(
                    "overloaded: the tick loop is {ticks_behind} ticks behind real time"
                )),
                HostLog::Fault { text, .. } => Event::Note(format!("fault: {text}")),
                HostLog::Note { text, .. } => Event::Note(text),
                // The program logs "Stopped" itself.
                HostLog::Stopped { .. } => continue,
                HostLog::Lobby {
                    callsign, event, ..
                } => Event::Note(format!("{callsign} {event}")),
                entry @ (HostLog::Stalled { .. } | HostLog::Resumed { .. }) => {
                    Event::Note(entry.stall_text().unwrap_or_default())
                }
                entry @ HostLog::Loaded { .. } => {
                    Event::Note(entry.loading_text().unwrap_or_default())
                }
                HostLog::Feedback(f) => Event::Note(format!(
                    "Feedback from {} at {:02}:{:02}: {}",
                    f.player,
                    f.tick / 7200,
                    f.tick / 120 % 60,
                    f.text.replace(['\n', '\r'], " ")
                )),
                HostLog::Chat {
                    callsign,
                    receiver,
                    text,
                    heard,
                    ..
                } => Event::Chat {
                    callsign,
                    receiver: receiver_label(receiver).to_ascii_lowercase(),
                    text,
                    heard,
                },
            });
        }
        // Stage L: a `content` line for each player's Content and a `gaps`
        // line when the gaps change (docs/DEDICATED-SERVER.md, "The log").
        while let Some(line) = self.host.poll_content_log() {
            self.events.push_back(Event::Note(line.text));
        }
    }
}

impl Host for SessionHost {
    fn poll(&mut self, now: Time) {
        self.now = now;
        // A build's loading keepalives reach relayed players through the
        // channels open now (protocol 28).
        self.host.set_relay_channels(self.listing.relay_channels());
        // A socket error is not fatal to a server: note it and go on.
        if let Err(error) = self
            .host
            .receive_from(now, &mut self.listing.over(&mut self.socket, now))
        {
            self.events
                .push_back(Event::Note(format!("receive failed: {error}")));
        }
        self.host.update(now);
        self.take_mapping_news(now);
        let host = &self.host;
        self.listing.update(now, || host.discover_answer(0).into());
        if let Err(error) = self
            .host
            .transmit(&mut self.listing.over(&mut self.socket, now))
        {
            self.events
                .push_back(Event::Note(format!("send failed: {error}")));
        }
        self.send_listing();
    }

    fn next_wake(&self, now: Time) -> Duration {
        self.host.next_wake(now)
    }

    fn wake_pace(&self) -> Pace {
        self.host.wake_pace()
    }

    fn take_events(&mut self) -> Vec<Event> {
        self.drain_log();
        if let Some(rotation) = &self.rotation {
            self.events
                .extend(rotation.lock().unwrap().notes.drain(..).map(Event::Note));
        }
        self.events.drain(..).collect()
    }

    fn status(&mut self, now: Time) -> Status {
        let s = self.host.status(now);
        Status {
            tick: s.tick,
            players: s.players,
            capacity: s.capacity,
            aircraft: s.aircraft,
            load_percent: s.load * 100.0,
            tick_ms: s.tick_cost_mean.as_secs_f64() * 1000.0,
            worst_tick_ms: s.tick_cost_max.as_secs_f64() * 1000.0,
            up_bytes_per_second: s.bytes_up_per_second as f64,
            down_bytes_per_second: s.bytes_down_per_second as f64,
        }
    }

    fn players(&self) -> Vec<PlayerFigures> {
        self.host
            .players()
            .into_iter()
            .map(|p| PlayerFigures {
                id: p.id,
                seat: p.seat,
                callsign: p.callsign,
                address: p.address.to_string(),
                plane: p.plane,
                round_trip_ms: p.round_trip.as_secs_f64() * 1000.0,
                loss_percent: p.loss.map(|loss| loss * 100.0),
                arrival_spread_ms: p.spread.as_secs_f64() * 1000.0,
                input_margin_ticks: p.input_margin_ticks.map(f64::from),
                inputs_repeated: p.inputs_repeated,
                bytes_up_per_second: p.bytes_up_per_second as f64,
                bytes_down_per_second: p.bytes_down_per_second as f64,
            })
            .collect()
    }

    fn kick(&mut self, seat: u8) -> Result<String, String> {
        let callsign = self
            .players()
            .into_iter()
            .find(|p| p.seat == Some(seat))
            .map(|p| p.callsign)
            .unwrap_or_default();
        self.host
            .kick(seat)
            .map(|()| callsign)
            .map_err(|error| error.to_string())
    }

    fn kick_player(&mut self, id: u8, reason: &str) -> Result<String, String> {
        let callsign = self
            .players()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.callsign)
            .unwrap_or_default();
        self.host
            .kick_player(id, reason)
            .map(|()| callsign)
            .map_err(|error| error.to_string())
    }

    fn end_mission(&mut self) {
        self.host.end();
    }

    fn rotation_status(&self) -> Option<String> {
        self.rotation.as_ref().map(|r| {
            let r = r.lock().unwrap();
            format!("rotation {}/{}", r.current + 1, r.entries.len())
        })
    }
    fn rotation_command(&mut self, command: &crate::console::Command) -> Result<String, String> {
        let rotation = self.rotation.as_ref().ok_or("No rotation is configured.")?;
        crate::rotation::command(rotation, &mut self.host, command)
    }

    fn restart_mission(&mut self) {
        if let Some(rotation) = &self.rotation {
            let mut r = rotation.lock().unwrap();
            let current = r.current + 1;
            let _ = r.goto(current);
        }
        self.host.restart();
    }

    fn stop(&mut self, now: Time) {
        self.now = now;
        // The mapping comes off while the players are told.
        if let Some(keeper) = self.keeper.as_mut() {
            keeper.begin_stop();
        }
        self.host.stop();
        // The mission's end, and its Report, before the listing goes.
        self.drain_log();
        self.listing.stop(now);
        // Sends the disconnects the stop queued, then the Unregisters.
        if let Err(error) = self
            .host
            .transmit(&mut self.listing.over(&mut self.socket, now))
        {
            self.events
                .push_back(Event::Note(format!("send failed: {error}")));
        }
        self.send_listing();
        if let Some(mut keeper) = self.keeper.take() {
            keeper.finish(keeper::REMOVE_WAIT);
            for news in keeper.poll() {
                for line in news.lines() {
                    self.events
                        .push_back(Event::Note(format!("Port mapping: {line}")));
                }
            }
        }
    }

    fn finished(&self) -> bool {
        self.host.phase() == Phase::Stopped
    }

    fn listing(&self) -> Option<String> {
        listing_short(&self.listing.state(), &self.listing.master_text())
    }

    fn set_broadcast(&mut self, on: bool, now: Time) -> Result<String, String> {
        if on == self.listing.rendezvous().listed_wanted() {
            return Err(format!(
                "broadcast is already {}",
                if on { "on" } else { "off" }
            ));
        }
        self.listing.set_listed(on, now);
        self.send_listing();
        Ok(if on {
            format!(
                "broadcast on: listing the server on the Internet Lobby at {}",
                self.listing.master_text()
            )
        } else {
            "broadcast off: taking the server off the Internet Lobby".into()
        })
    }
}

impl SessionHost {
    /// Sends the listing's datagrams and logs what it says.
    fn send_listing(&mut self) {
        if let Err(error) = self.listing.transmit(&mut self.socket) {
            self.events
                .push_back(Event::Note(format!("send failed: {error}")));
        }
        // The master's text is written only when there is news to say.
        let mut master: Option<String> = None;
        while let Some(event) = self.listing.poll_event() {
            let master = master.get_or_insert_with(|| self.listing.master_text());
            self.events
                .push_back(Event::Note(listing_text(event, master)));
        }
    }
}
