//! A networked flight in the game: joining from the command line, the
//! session's turn each frame, the flight it starts when the host seats the
//! player, what the screen presents from the client's frame (the events as
//! HUD lines, speech, sounds and rumble, and what the single-player tick
//! presenter does once a tick), and how the flight ends.
//!
//! The flight screen itself is `main.rs`'s: it draws a [`FlightFrame`], and
//! here the frame comes from the client session (`ClientFrame`) instead of
//! the world. The game keeps its own copy of the mission, never stepped, with
//! the player's plane taken as a handoff so every screen that reads a
//! cockpit finds one; each frame the cockpit is overwritten with the
//! client's prediction (agent decision).
use crate::{
    App, OWN, SEAT, Screen, TickPresenter,
    aircraft::Airframe,
    combat_view::CombatView,
    comms, flight, flight_views,
    frame::FlightFrame,
    net::{
        debrief,
        session::{EffectContext, GunContext, NetSession},
    },
    scenery, seats, show_selected_weapon_page,
};
use std::time::{Duration, Instant};
use tore_session::{
    ClientEvent, Controls,
    client::ended_text,
    wire::{
        events::{ReceivedEvent, WireEvent},
        names::NameIndex,
    },
};
use winit::event_loop::ActiveEventLoop;

/// The most client ticks a frame presents at tick rate (camera weather,
/// blackout, vapor, the situation music): a stall is not caught up on.
const MAX_PRESENTED_TICKS: u64 = 8;
/// How long a message that ends a session stays on the main menu.
const MESSAGE_SECONDS: u64 = 12;
/// How often the revival prompt is said again while the plane is lost
/// (agent decision, F2-V: the HUD's message lines fade).
const REVIVAL_PROMPT_EVERY: Duration = Duration::from_secs(5);

/// The flight frame of the moment between redraws, for the commands that
/// read what the screen shows: the client's newest frame in a networked
/// flight, the world's otherwise.
pub(crate) fn current_frame<'a>(
    world: &'a crate::world::World,
    flight: &'a Option<NetFlight>,
    net: &'a Option<NetSession>,
) -> FlightFrame<'a> {
    match (flight.as_ref().and_then(|f| f.frame.as_ref()), net.as_ref()) {
        (Some(client), Some(net)) => client.flight_frame(
            [&net.effects.smoke, &net.effects.contrails],
            &net.effects.devices,
            || {
                world
                    .cockpit_readout(SEAT, crate::combat::launcher(&client.presented))
                    .expect("the seat flies a plane")
            },
        ),
        _ => crate::tick_frame(world, SEAT, &[]),
    }
}

/// The flight menu on screen: a session's has no time or mission-changing
/// rows ([`crate::flight_ui::session_menu`]).
pub(crate) fn menu<'a>(
    hornet: &'a Airframe,
    flight: &'a Option<NetFlight>,
) -> &'a [tore_formats::ui::MenuNode] {
    flight.as_ref().map_or(&hornet.flight_menu, |f| &f.menu)
}

/// What the app set aside to show a networked flight and puts back when it
/// ends, so single player starts from the state it left.
pub struct Stash {
    markup: Option<crate::flight_map::Plan>,
    world: crate::world::World,
    combat_view: CombatView,
    hornet: Airframe,
    scenery: scenery::Scenery,
}

/// A networked flight on screen.
pub struct NetFlight {
    stash: Stash,
    /// The newest frame the client gave, which the screen draws.
    pub frame: Option<tore_session::ClientFrame>,
    /// The flight menu a session shows.
    pub menu: Vec<tore_formats::ui::MenuNode>,
    /// What the last frame showed, to see what changed.
    last_tick: u64,
    weather_steps: u64,
    was_dead: bool,
    was_crashed: bool,
    was_burning: bool,
    /// K's score board is open (`scoreboard`).
    pub score_board: bool,
    pub score_page: usize,
    /// When the revival prompt was last said, and whether it said the
    /// player may fly again (stage F phase 2, slice F2-V).
    revival_said: Option<(Instant, bool)>,
    pub(crate) respawn_menu: Option<super::respawn::Menu>,
    pub(crate) respawn_seen: bool,
    pub(crate) respawn_choice: Option<(
        tore_formats::aircraft::AircraftId,
        tore_world::mission::LoadoutSpec,
    )>,
    /// The AI flies the plane while the player is away (slice F2-A).
    pub(crate) idle: crate::net::away::Idle,
    /// The AI test settings last sent to the game this player hosts, so a
    /// change in the Debug menu is sent once.
    ai_test_sent: tore_sim::ai::test_settings::AiTestSettings,
    /// The surface units' states last put into the game's copy of the
    /// mission (protocol 22).
    surface_units: std::collections::BTreeMap<u32, tore_session::wire::events::SurfaceUnitView>,
}

impl App {
    /// Starts joining the server the command line named.
    pub(crate) fn start_session(&mut self, options: crate::net::options::ConnectOptions) {
        let data = match crate::assets::data_directory() {
            Ok(data) => data,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let server = options.server();
        crate::net::settings::remember_join(&data, &options);
        match crate::net::session::Join::connect(&options) {
            Ok(join) => {
                self.start_join(join, &server);
            }
            Err(error) => self.message(error),
        }
    }

    /// Starts the session over `join`, naming `server` to the player. False
    /// (with the reason shown) when it could not start.
    pub(crate) fn start_join(&mut self, join: crate::net::session::Join, server: &str) -> bool {
        if !crate::net::session::content_ready(&self.theater_resources) {
            self.pending_join = Some((join, server.to_owned()));
            return true;
        }
        let data = match crate::assets::data_directory() {
            Ok(data) => data,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        match NetSession::start(
            join,
            std::sync::Arc::clone(&self.theater_resources),
            &data,
            self.replay_library.as_ref(),
        ) {
            Ok(session) => {
                self.net = Some(session);
                self.message(format!("Joining {server}..."));
                true
            }
            Err(error) => {
                self.message(error);
                false
            }
        }
    }

    /// A message for the player where they are: a HUD line in flight, the
    /// menu's message line otherwise, which stays long enough to read.
    pub(crate) fn message(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::info!("Network: {text}");
        if self.screen == Screen::Flight {
            self.flight_ui.message(text);
        } else if self.screen == Screen::Replay
            && let Some(replay) = &mut self.replay
        {
            // The observer screen is where the player is looking (an away
            // player's own aircraft, slice F2-O4, or a watch).
            replay.viewer.message(text);
        } else if let Some(screen) = &mut self.lobby.screen {
            // The lobby is where the player is looking, over Direct
            // Connection (EF8).
            screen.say(&text);
        } else if let Some(screen) = &mut self.direct.screen {
            // The Direct Connection screen is where the player is looking.
            screen.say(&text);
        } else if let Some(screen) = &mut self.internet.screen {
            // Or the Internet Lobby screen (I4).
            screen.say(&text);
        } else {
            self.menu.state.toast = Some((
                text,
                Instant::now() + Duration::from_secs(MESSAGE_SECONDS - 3),
            ));
        }
    }

    /// The session's turn: controls in, the network pumped, what happened
    /// handled, and, once flying, the frame the screen draws and what that
    /// frame presents. In flight it is called at the start of every redraw;
    /// everywhere else from the event loop's wait on the session's own wake
    /// ([`super::pace`]). True when events were handled or the session
    /// ended.
    pub(crate) fn net_tick(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.net.is_none() {
            return false;
        }
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Network);
        let flying = self.net_flight.is_some() && self.screen == Screen::Flight;
        let sensors = self.instruments.controls();
        let mut controls = if flying && !self.flight_ui.frozen() && self.focused {
            self.session_controls(sensors)
        } else {
            // A paused game keeps its gunsight zoom, as the host's stall
            // rule does.
            Controls {
                sight_zoom: self.input.sight().1,
                ..Controls::neutral(sensors)
            }
        };
        if flying {
            crate::queue_gun_key(
                &mut self.seat_commands,
                &mut self.gun_key_sent,
                !self.flight_ui.frozen() && self.focused && self.input.resolver.held("fire-gun"),
            );
            controls.commands = std::mem::take(&mut self.seat_commands);
        }
        let session = self.net.as_mut().expect("a session");
        let before_tick = if crate::frame_log::enabled() {
            session.client.prediction().map(|p| p.tick())
        } else {
            None
        };
        session.pump(&controls);
        if let Some(before) = before_tick
            && let Some(after) = session.client.prediction()
        {
            crate::frame_log::count(
                crate::frame_log::Count::NetworkTicks,
                after.tick().saturating_sub(before) as usize,
            );
        }
        if let Some(failure) = session.take_host_failure() {
            self.net_ending = Some(failure);
            self.end_session(event_loop);
            return true;
        }
        let mut news = false;
        for event in session.take_events() {
            news = true;
            if !self.net_event(event) {
                self.end_session(event_loop);
                return true;
            }
        }
        // Stage F phase 2: the observer screen (F2-O2), and the AI flying an
        // idle player's aircraft (F2-A).
        self.observe_turn();
        self.net_idle(&controls);
        // A game hosted from the command line starts each mission as soon as
        // everyone holding a slot is ready and its player has closed the
        // debrief.
        let reading = self.quick.debrief.is_some();
        if let Some(session) = &mut self.net {
            session.auto_start(!reading && self.net_flight.is_none());
        }
        if self.loading.is_none()
            && self.net_flight.is_none()
            && !self.begin_session_flight(event_loop)
        {
            return true;
        }
        if self.net_flight.is_some() && self.screen == Screen::Flight {
            self.net_present();
        }
        news
    }

    /// Whether the session is pumped from the event loop's wait on its own
    /// wake ([`super::pace`]) rather than at the start of a redraw: outside
    /// flight and the replay viewer, and while a load runs with a world.
    /// Flight pumps in its frame, as it always did.
    pub(crate) fn net_pumped_in_wait(&self) -> bool {
        self.net.is_some()
            && if self.loading.is_some() {
                self.world.0.is_some()
            } else {
                !matches!(self.screen, Screen::Flight | Screen::Replay)
            }
    }

    /// Whether the screen is a menu drawn on the menu pace: a session open
    /// over the main menu, the creator or a viewer, or the lobby, Direct or
    /// Internet screen with or without one. Loads draw on their own pace.
    pub(crate) fn menu_paced(&self) -> bool {
        self.loading.is_none()
            && self.pending_host.is_none()
            && self.pending_join.is_none()
            && ((self.net.is_some()
                && matches!(self.screen, Screen::Main | Screen::Quick | Screen::Viewer))
                || (self.screen == Screen::Main
                    && (self.lobby.screen.is_some()
                        || self.direct.screen.is_some()
                        || self.internet.screen.is_some())))
    }

    /// Whether the session is connecting or loading its mission, which the
    /// network loading screen shows.
    pub(crate) fn net_preparing(&self) -> bool {
        (self.net_built.is_some() && self.network_loading_stage.is_some())
            || self.net.as_ref().is_some_and(|s| {
                s.client.is_building()
                    || matches!(
                        s.client.phase(),
                        tore_session::ClientPhase::Connecting | tore_session::ClientPhase::Loading
                    )
            })
    }

    /// The session's turn when it is due, from the event loop's wait.
    pub(crate) fn pump_in_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.net_pumped_in_wait() || !self.menu_pace.pump_due(Instant::now()) {
            return;
        }
        let news = self.net_tick(event_loop);
        let (wake, phase) = self.net.as_ref().map_or((Duration::ZERO, None), |s| {
            (s.next_wake(), Some(s.client.phase()))
        });
        let lobby = self.net.as_ref().and_then(|s| s.client.lobby());
        let unable = self.net.as_ref().and_then(|s| s.client.unable());
        // A session that ended is news too: the screens close on it.
        let news = news || self.net.is_none();
        self.menu_pace
            .pumped(Instant::now(), wake, news, lobby, unable, phase);
    }

    /// The player did something on a menu: frames at the display's pace for
    /// a moment, starting now. The network screens do not ask for their own
    /// frames, so every event that can change them does.
    pub(crate) fn note_menu_input(&mut self, event: &winit::event::WindowEvent) {
        use winit::event::WindowEvent as W;
        if !matches!(
            event,
            W::CursorMoved { .. }
                | W::CursorLeft { .. }
                | W::MouseInput { .. }
                | W::MouseWheel { .. }
                | W::KeyboardInput { .. }
                | W::Ime(_)
                | W::Touch(_)
                | W::Focused(_)
                | W::Resized(_)
                | W::ScaleFactorChanged { .. }
        ) || !self.menu_paced()
        {
            return;
        }
        self.menu_pace.activity(Instant::now());
        if let Some(renderer) = &self.renderer {
            renderer.window.request_redraw();
        }
    }

    /// The end of an about_to_wait on a loading screen: its frame at 20 Hz
    /// (at once when `now`), the loader polled every 16 ms, and the session
    /// pumped on its own wake.
    pub(crate) fn loading_wait(&mut self, event_loop: &ActiveEventLoop, now: bool) {
        let at = Instant::now();
        // With the frame-rate counter shown a loading screen draws at the
        // frame rate setting (`loading_next_frame` schedules it); the 20 Hz
        // pace is the fallback before the first frame and when hidden.
        let timed = self.counter_shown() && self.next_frame.is_some();
        let due = if timed {
            self.next_frame.is_some_and(|next| at >= next)
        } else {
            self.menu_pace.loading_redraw_due(at)
        };
        if (due || now)
            && let Some(renderer) = &self.renderer
        {
            renderer.window.request_redraw();
            if timed {
                self.next_frame = None;
            }
        }
        let mut next = at + Duration::from_millis(16);
        if timed && let Some(frame) = self.next_frame {
            next = next.min(frame);
        }
        if self.net_pumped_in_wait() {
            next = next.min(self.menu_pace.next_pump());
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next));
    }

    /// After a loading screen's frame: the next one at the frame rate
    /// setting while the frame-rate counter is shown; otherwise the 20 Hz pace
    /// of `loading_wait` stands.
    pub(crate) fn loading_next_frame(&mut self) {
        self.next_frame = if self.counter_shown() {
            self.pace(crate::frame_pacing::Want::Live)
        } else {
            None
        };
    }

    /// The pilot's controls for this frame from the keyboard, the mouse and
    /// the controllers, as a tick of single player takes them.
    fn session_controls(&mut self, sensors: tore_sim::sensors::Controls) -> Controls {
        let frame = self.net_flight.as_ref().and_then(|f| f.frame.as_ref());
        if let Some(frame) = frame
            && let Some(service) = frame
                .readout
                .as_ref()
                .and_then(|r| r.airport.service.as_ref())
        {
            self.instruments.navigation.refresh(
                &self.world.terrain.airport_scene,
                service,
                self.world.cockpits[OWN].flight.position,
                self.world.roster.redfor(frame.plane),
            );
            for button in std::mem::take(&mut self.instruments.navigation.pending) {
                if let Some(id) = self.instruments.navigation.control(button) {
                    crate::queue_command(
                        &mut self.seat_commands,
                        seats::SeatCommand::Airport(crate::world::AirportInput::Command(
                            tore_sim::airport::Command::SelectAirport(id),
                        )),
                    );
                }
            }
        }
        crate::sync_stability(
            &self.world.cockpits[OWN].flight,
            &mut self.input,
            self.flight_ui.stability,
        );
        let flight = &self.world.cockpits[OWN].flight;
        let lever = self
            .input
            .throttle_reference(flight.throttle, flight.lift_controls.collective);
        self.input.console.open = self.console_open();
        let (pilot, _) = self.input.frame(&self.camera.keys, lever);
        for command in self.input.console.take() {
            crate::queue_command(
                &mut self.seat_commands,
                seats::SeatCommand::Console(command),
            );
        }
        let (sight, sight_zoom) = self.input.sight();
        Controls {
            pilot,
            trigger: self.input.resolver.held("fire"),
            sensors,
            sight,
            sight_zoom,
            commands: Vec::new(),
            view_subject: None,
        }
    }

    /// One session event. `false` ends the session.
    fn net_event(&mut self, event: ClientEvent) -> bool {
        match event {
            ClientEvent::Connected { .. } => {
                if self.lobby.screen.is_some()
                    || self.direct.screen.is_some()
                    || self.internet.screen.is_some()
                {
                    self.message("Connected. Loading the game's mission...");
                }
            }
            ClientEvent::MissionLoaded => {
                let session = self.net.as_mut().expect("a session");
                match session.take_built() {
                    Some(Ok(built)) => self.net_built = Some(built),
                    Some(Err(error)) => {
                        self.net_ending = Some(format!("The mission could not be built: {error}"));
                        return false;
                    }
                    None => {}
                }
                // A lobby screen has the player take a plane itself.
                if self.net_flight.is_none() && self.lobby.screen.is_none() {
                    self.message("Mission loaded; taking a plane...");
                }
            }
            ClientEvent::ContentRefused { names, reason } => {
                log::warn!("Network: {reason} ({})", names.join(", "));
                // With a lobby screen the player stays in the lobby, marked
                // unable with the reason (the screen reads it from the
                // client); with none to wait in, the game leaves.
                if self.lobby.screen.is_none() {
                    self.net_ending = Some(reason);
                    return false;
                }
            }
            ClientEvent::MissionFailed(text) => {
                self.net_ending = Some(format!("The mission could not be built: {text}"));
                return false;
            }
            ClientEvent::SeatRefused(text) => {
                // The client asks for any free plane by itself.
                self.message(format!("No plane: {text}"));
            }
            ClientEvent::Seated { plane, .. } => {
                if let Some(session) = &mut self.net {
                    session.ended = None;
                }
                log::info!("Network: seated in plane {plane}");
                // A new plane starts with the gunsight's default zoom.
                self.input.reset_sight();
                // Seated again in flight (a revival, slice F2-V): this
                // flight is put away and the next turn starts the new one,
                // as a player who left its flight and joined again starts.
                if self.net_flight.is_some() {
                    self.end_net_flight();
                }
            }
            ClientEvent::Roster => {}
            ClientEvent::Notice(text) => self.message(text),
            ClientEvent::Chat(line) => {
                if let Some(screen) = &mut self.lobby.screen {
                    screen.chat_line(&line);
                }
                self.chat_line(line);
            }
            ClientEvent::Debrief(debrief) => {
                // A player leaving the game sees it when the session ends; one
                // who stays sees it now, back in the lobby.
                let leaving = self.net.as_ref().is_some_and(|s| s.left_at.is_some());
                if !leaving {
                    if let Some(session) = &mut self.net {
                        session.debrief = None;
                    }
                    // A player who left its flight (the others fly on) sees
                    // no end of the mission: its flight ends here.
                    self.end_net_flight();
                    let cause = self.net_cause();
                    self.show_net_debrief(&debrief, cause);
                }
            }
            ClientEvent::MissionEnded(ended) => {
                if let Some(session) = &mut self.net {
                    session.ended = Some(ended.reason);
                }
                let text = ended_text(&ended);
                self.end_net_flight();
                self.message(text);
            }
            ClientEvent::Lobby => {
                self.refresh_feedback();
                if let Some(line) = self.net.as_mut().and_then(|s| s.lobby_change()) {
                    log::info!("Network: lobby: {line}");
                }
            }
            ClientEvent::Refused { request, reason } => {
                // Away and Back are the flight's (slice F2-A).
                use tore_session::wire::messages::kind;
                if matches!(request, kind::AWAY | kind::BACK) {
                    self.message(reason);
                } else if self.lobby.screen.is_some() {
                    self.lobby_refused(request, &reason);
                } else {
                    self.message(reason);
                }
            }
            ClientEvent::Goodbye(_) => {}
            // Stage F phase 2 (slice F2-V): the revival prompt, and a
            // revival's plane added to the flight's copy of the mission.
            ClientEvent::Revival(_) => {
                if let Some(flight) = &mut self.net_flight {
                    flight.revival_said = None;
                }
            }
            ClientEvent::Spawned(spawned) => {
                if self.net_flight.is_some() {
                    add_spawned(&mut self.world, std::slice::from_ref(&spawned));
                    if let Err(error) = self.combat_view.prepare_spawn(
                        &self.world,
                        &spawned,
                        &self.theater_resources,
                    ) {
                        log::warn!("Network: revival art could not be loaded: {error}");
                    }
                }
            }
            // The slices that build each part show them.
            ClientEvent::Scores(_) | ClientEvent::Results(_) => {}
            // The observer screen (slice F2-O2).
            ClientEvent::Observing(observing) => self.observe_event(matches!(
                *observing,
                tore_session::wire::messages::Observing::Started(_)
            )),
            ClientEvent::Closed(reason) => {
                let left = self.net.as_ref().is_some_and(|s| s.left_at.is_some());
                let text = self
                    .net
                    .as_ref()
                    .map(|s| s.close_text(&reason))
                    .unwrap_or_default();
                self.net_ending = Some(if left { String::new() } else { text });
                return false;
            }
        }
        true
    }

    /// Who ended the flight a debrief reports: the host, when it has just
    /// ended the mission, else the player itself.
    fn net_cause(&mut self) -> debrief::Cause {
        match self.net.as_mut().and_then(|s| s.ended.take()) {
            Some(reason) => debrief::Cause::Host(reason),
            None => debrief::Cause::Left,
        }
    }

    /// The screen for a debrief the host sent: a mission its objectives did
    /// not decide says it ended, and by whom, instead of a failure.
    fn net_debrief_screen(
        &self,
        debrief: &tore_session::wire::messages::Debrief,
        cause: debrief::Cause,
    ) -> crate::AppResult<crate::debrief::Debrief> {
        let lobby = self.net.as_ref().and_then(|s| s.client.lobby());
        let record = self.net.as_ref().map(|s| s.client.flight_record());
        let partial = matches!(cause, debrief::Cause::Left);
        let ended = if partial {
            Some(debrief::partial_ending(record.map_or(0, |r| r.tick), false))
        } else {
            debrief::ending(debrief, cause, lobby)
        };
        let extra = if partial {
            record.map(debrief::local_pages).unwrap_or_default()
        } else {
            self.net
                .as_ref()
                .and_then(|s| s.client.results())
                .map(debrief::extra_pages)
                .unwrap_or_default()
        };
        crate::debrief::Debrief::networked(
            debrief::report(debrief),
            &self.theater_resources,
            None,
            ended.as_ref(),
            &extra,
        )
    }

    /// The debrief the host sent, on its screen, which closes to the main
    /// menu.
    fn show_net_debrief(
        &mut self,
        debrief: &tore_session::wire::messages::Debrief,
        cause: debrief::Cause,
    ) {
        match self.net_debrief_screen(debrief, cause) {
            Ok(screen) => {
                self.quick.debrief = Some(screen);
                self.quick.debrief_to_menu = true;
                self.screen = Screen::Quick;
                if let Some(renderer) = &self.renderer {
                    renderer.window.set_title("T.O.R.E-Fighters - Debrief");
                    renderer.window.request_redraw();
                }
            }
            Err(error) => self.message(error.to_string()),
        }
    }

    /// The flight is over but the session goes on (the mission ended, back
    /// in the lobby): the flight is put away and single player's state put
    /// back, as when a session ends.
    pub(crate) fn end_net_flight(&mut self) {
        self.feedback = None;
        let Some(flight) = self.net_flight.take() else {
            return;
        };
        // An away player's observer screen goes with the flight it covers
        // (slice F2-O3).
        self.close_away_watch();
        if let Some(session) = &mut self.net {
            session.chat.flight_ended();
        }
        let stash = flight.stash;
        self.mission_markup = stash.markup;
        self.flight_ui.map.brief = None;
        self.world.0 = Some(stash.world);
        self.combat_view = stash.combat_view;
        self.hornet = stash.hornet;
        self.scenery = stash.scenery;
        if let Some(renderer) = &mut self.renderer {
            renderer.set_scenery(&self.scenery);
            renderer.prepare_aircraft(&self.hornet);
            renderer.combat(&Default::default());
            renderer
                .window
                .set_title("T.O.R.E-Fighters - Choose Activity");
        }
        self.flight_ui.reset_for_flight();
        self.camera.keys.clear();
        self.seat_commands.clear();
        self.gun_key_sent = false;
        self.input.context(true, self.focused);
        self.screen = Screen::Main;
        if let Some(audio) = &self.audio {
            audio.scene(crate::audio::music::Scene::Main);
        }
    }

    /// Starts the flight once the host has seated the player and the mission
    /// is built. `false` when the session ended instead.
    fn begin_session_flight(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(session) = &self.net else {
            return true;
        };
        let Some((_, plane)) = session.client.seat() else {
            return true;
        };
        // The build of the mission the host sent serves the first seating; a
        // player who left its flight and takes a plane in the same running
        // mission again (the others fly on) has the game build it again.
        let built = match self.net_built.take() {
            Some(built) => built,
            None => {
                match self.net.as_ref().and_then(NetSession::rebuild_background) {
                    Some(Ok(job)) => self.loading = Some(job),
                    Some(Err(error)) => {
                        self.net_ending = Some(format!("The mission could not be built: {error}"));
                        self.end_session(event_loop);
                        return false;
                    }
                    None => {}
                }
                return true;
            }
        };
        let plane = plane.0;
        // A page of the lobby still open gives way to the flight.
        self.close_lobby_page();
        let crate::net::session::Built {
            mut world,
            models,
            markup,
        } = built;
        // The planes revivals have added to the mission so far (slice F2-V).
        if let Some(session) = &self.net {
            add_spawned(&mut world, session.client.spawned());
        }
        let aircraft = match world.ai_wings.as_ref().and_then(|wings| wings.slot(plane)) {
            Some(slot) => slot.aircraft,
            None => {
                self.net_ending = Some(format!(
                    "The server seated plane {plane}, which this mission does not have"
                ));
                self.end_session(event_loop);
                return false;
            }
        };
        let resources = std::sync::Arc::clone(&self.theater_resources);
        let spawned = self
            .net
            .as_ref()
            .map(|session| session.client.spawned().to_vec())
            .unwrap_or_default();
        let graphics = self
            .renderer
            .as_ref()
            .map(crate::renderer::Renderer::loading_factory);
        let job = crate::loading::Job::continue_load(
            self.network_loading_stage.take(),
            true,
            move |progress| {
                let result = (|| -> crate::AppResult<_> {
                    world
                        .take_plane(SEAT, crate::seats::PlaneId(plane))
                        .map_err(|error| error.to_string())?;
                    progress.stage(crate::loading::Stage::Aircraft);
                    let hornet = Airframe::load(&resources, aircraft)?;
                    let mut view =
                        CombatView::with_models(&world.combat, plane, &resources, models)
                            .map_err(|error| error.to_string())?;
                    {
                        for spawned in &spawned {
                            view.prepare_spawn(&world, spawned, &resources)?;
                        }
                    }
                    progress.stage(crate::loading::Stage::Scenery);
                    let mut scenery = scenery::Scenery::build(&resources, &world.terrain)?;

                    progress.stage(crate::loading::Stage::Graphics);
                    let geometry = std::sync::Arc::clone(
                        scenery.static_geometry(&world.combat.state.targets, &world.combat.surface),
                    );
                    let graphics = graphics.map(|build| {
                        build(
                            crate::renderer::LoadingScene {
                                scenery: &scenery,
                                world: &world.terrain,
                                eye: world.cockpits[usize::from(SEAT.0)].flight.position,
                            },
                            &hornet,
                            &view.models,
                            &view.art,
                            Some(&geometry),
                        )
                    });
                    Ok(crate::loading::Prepared::Net(Box::new((
                        world, markup, hornet, view, scenery, graphics,
                    ))))
                })();
                result.map_err(|e| e.to_string())
            },
        );
        match job {
            Ok(job) => self.loading = Some(job),
            Err(error) => {
                self.net_ending = Some(error);
                self.end_session(event_loop);
                return false;
            }
        }
        true
    }

    pub(crate) fn finish_session_loading(&mut self, parts: crate::loading::NetParts) {
        let _timing = crate::loading::timing("multiplayer flight installation");
        let (world, markup, hornet, view, scenery, graphics) = parts;
        self.flight_ui.map.brief = None;
        let plane = world.picture_plane().0;
        // Single player's state waits aside.
        let stash = Stash {
            markup: self.mission_markup.replace(markup),
            world: std::mem::replace(&mut *self.world, world),
            combat_view: std::mem::replace(&mut self.combat_view, view),
            hornet: std::mem::replace(&mut self.hornet, hornet),
            scenery: std::mem::replace(&mut self.scenery, scenery),
        };
        let retired = if let Some(renderer) = &mut self.renderer {
            if let Some(graphics) = graphics {
                renderer.install_loading(graphics)
            } else {
                renderer.set_scenery(&self.scenery);
                renderer.prepare_aircraft(&self.hornet);
                None
            }
        } else {
            None
        };
        if let Some(handle) = retired {
            self.shelve_background(handle);
        }
        if let Some(audio) = &self.audio {
            audio.restart_flight();
        }
        self.input.context(true, self.focused);
        self.seat_commands.clear();
        self.gun_key_sent = false;
        self.cheats_sent = None;
        self.instruments.navigation = crate::navigation::Navigation::default();
        self.scenery.reset_presentations();
        self.flight_music = crate::flight_music::Observer::new();
        self.rwr_warnings = Default::default();
        self.reset_vapor();
        self.g_effects = Default::default();
        self.flight_clock.remainder = 0.;
        self.flight_view = 0;
        self.view_rig = flight_views::Rig::for_plane(plane);
        let saved = crate::preferences::Preferences::capture(
            &self.flight_ui,
            &self.instruments,
            self.fullscreen_preference,
        );
        self.flight_ui.reset_for_flight();
        self.live_debug.reset();
        saved.apply(&mut self.flight_ui, &mut self.instruments);
        self.flight_ui.enter_session();
        self.screen = Screen::Flight;
        self.camera.keys.clear();
        self.quick.cancel();
        self.frame_time = Instant::now();
        if let Some(renderer) = &self.renderer {
            renderer.window.set_title(&format!(
                "T.O.R.E-Fighters - {} Multiplayer",
                self.hornet.profile.id.label()
            ));
        }
        if let Some(audio) = &self.audio {
            audio.scene(crate::audio::music::Scene::Score(0));
        }
        self.net_flight = Some(NetFlight {
            stash,
            frame: None,
            menu: {
                let mut tree = crate::flight_ui::session_menu(&self.hornet.flight_menu);
                crate::feedback::menu(
                    &mut tree,
                    crate::feedback::enabled(self.net.as_ref().and_then(|s| s.client.lobby())),
                );
                tree
            },
            last_tick: 0,
            weather_steps: 0,
            was_dead: false,
            was_crashed: false,
            was_burning: false,
            score_board: false,
            score_page: 0,
            revival_said: None,
            respawn_menu: None,
            respawn_seen: false,
            respawn_choice: None,
            idle: Default::default(),
            ai_test_sent: Default::default(),
            surface_units: Default::default(),
        });
    }

    /// The Debug menu's AI test settings reach the game this player hosts,
    /// where the host's setting governs the simulation; in a joined game the
    /// rows are locked. Nothing about it travels to other players.
    fn sync_ai_test(&mut self) {
        let hosting = self.net.as_ref().and_then(|net| net.hosting.as_ref());
        self.flight_ui.lock_ai_test(hosting.is_none());
        let (Some(hosting), Some(flight)) = (hosting, self.net_flight.as_mut()) else {
            return;
        };
        let wanted = self.flight_ui.ai_test;
        if flight.ai_test_sent != wanted && hosting.send(super::hosting::Command::SetAiTest(wanted))
        {
            flight.ai_test_sent = wanted;
        }
    }

    /// This frame's client frame, its effects, and everything it presents.
    fn net_present(&mut self) {
        self.sync_ai_test();
        let Some(session) = self.net.as_mut() else {
            return;
        };
        let Some(mut frame) = session.frame() else {
            return;
        };
        session.step_guns(
            &mut frame,
            &GunContext {
                terrain: &self.world.terrain,
                configurations: self.world.combat.dummy_configurations(),
            },
        );
        let flight = self.net_flight.as_mut().expect("a networked flight");
        let ticks = frame
            .tick
            .saturating_sub(flight.last_tick)
            .min(MAX_PRESENTED_TICKS)
            .max(u64::from(flight.last_tick == 0));
        flight.last_tick = frame.tick;

        // The screens that read a cockpit read the client's prediction.
        let cockpit = &mut self.world.cockpits[OWN];
        cockpit.previous_flight.clone_from(&frame.previous);
        cockpit.flight.clone_from(&frame.flight);
        if let Some(readout) = &frame.readout {
            cockpit.airport_nav_mode = readout.airport.nav_mode;
            if let Some(service) = &readout.airport.service {
                cockpit.airport_service.clone_from(service);
            }
        }
        // Ground objects the host has destroyed stop standing.
        for pose in frame
            .picture
            .targets
            .iter()
            .filter(|pose| !pose.airborne && pose.damage.hp <= 0)
        {
            if let Some(target) = self
                .world
                .combat
                .state
                .targets
                .iter_mut()
                .find(|target| target.id == pose.id)
            {
                target.hp = 0;
            }
        }
        // The surface units as the host has them: hit points, radars,
        // launcher rails and spare magazines (protocol 22).
        if flight.surface_units != frame.surface_units {
            surface_units_to_world(&mut self.world, &frame.surface_units);
            flight.surface_units.clone_from(&frame.surface_units);
        }
        // The city fill buildings the host's ground bursts knocked flat
        // (protocol 25), as the session's copy of the mission has them: the
        // scenery draws them flat.
        if let (Some(ours), Some(told)) = (
            &self.world.terrain.city_fill,
            session
                .client
                .mission()
                .and_then(|world| world.terrain.city_fill.as_ref()),
        ) {
            ours.copy_flattened(told);
        }

        // The weather clock follows the host's tick.
        let behind = frame.tick.saturating_sub(flight.weather_steps);
        let bulk = behind.saturating_sub(ticks);
        for _ in 0..bulk {
            self.world.terrain.weather.step();
        }
        flight.weather_steps += bulk;

        // Smoke, contrails, chaff and flares.
        {
            let models: Vec<&Airframe> = self
                .combat_view
                .models
                .iter()
                .chain(std::iter::once(&self.hornet))
                .collect();
            session.step_effects(
                frame.tick,
                &frame.picture,
                &EffectContext {
                    terrain: &self.world.terrain,
                    models: &models,
                    resources: &self.theater_resources,
                    sortie: self.world.combat.contrail_sortie(),
                },
            );
            // The motors that burn draw their flame (bug B7).
            session.mark_powered(&mut frame.picture, &self.theater_resources);
        }
        // The seat's own pylons and stores (B3): its loadout from the
        // session's copy of the mission, the rounds left from its readout,
        // which is newer than the drawn tick the session puts every other
        // aircraft's stores at (protocol 27).
        let mut picture = frame.picture.clone();
        if let Some(readout) = &frame.readout
            && let Some(own) = self.world.combat.state.ownship(readout.plane)
        {
            picture.player.stores = tore_world::snapshot::store_poses(
                own.configuration(),
                |station| u32::from(readout.stores.rounds(station)),
                |station| station < 32 && readout.stores.loaded & (1 << station) != 0,
            );
        }
        self.combat_view.show_picture(picture);

        // What the frame shows, presented.
        let session = self.net.as_ref().expect("a session");
        let effects = &session.effects;
        let world = &self.world;
        let presented = frame.flight_frame(
            [&effects.smoke, &effects.contrails],
            &effects.devices,
            || {
                let launcher = crate::combat::launcher(&frame.presented);
                world
                    .cockpit_readout(SEAT, launcher)
                    .expect("the seat flies a plane")
            },
        );
        let name = |index: NameIndex| session.client.name(index).map(str::to_owned);
        let mut presenter = TickPresenter {
            world: &mut self.world,
            scenery: &mut self.scenery,
            flight_ui: &mut self.flight_ui,
            input: &mut self.input,
            audio: self.audio.as_ref(),
            recorder: None,
            instruments: &mut self.instruments,
            flight_view: &mut self.flight_view,
            view_rig: &mut self.view_rig,
            hornet: &self.hornet,
            combat_view: &self.combat_view,
            head_look: self.head_look,
            g_effects: &mut self.g_effects,
            perf_blackout: self.performance.veil_level(),
            vapor: &mut self.vapor,
            flight_music: &mut self.flight_music,
            rwr_warnings: &mut self.rwr_warnings,
            launch_watch: &mut self.launch_watch,
            engine_sounds: &mut self.engine_sounds,
            spatial: &mut self.spatial_steps,
        };
        let mut state = Seen {
            was_dead: flight.was_dead,
            was_crashed: flight.was_crashed,
            was_burning: flight.was_burning,
        };
        presenter.present_net(
            &presented,
            &frame.events,
            &name,
            session.launch_sounds(),
            ticks,
            &mut state,
        );
        flight.was_dead = state.was_dead;
        flight.was_crashed = state.was_crashed;
        flight.was_burning = state.was_burning;
        drop(presented);
        flight.frame = Some(frame);
        self.revival_prompt();
    }

    /// While the plane is lost, the host's Revival in words on the HUD
    /// ("Press Enter to fly again (2 lives left)", "You can fly again in
    /// 0:45"), said again every few seconds and when the player may fly
    /// again (slice F2-V).
    fn revival_prompt(&mut self) {
        self.refresh_respawn_menu();
        let Some(session) = &self.net else {
            return;
        };
        let (Some(text), may) = (
            session.client.revival_prompt(),
            session.client.may_fly_again(),
        ) else {
            return;
        };
        let Some(flight) = &mut self.net_flight else {
            return;
        };
        let now = Instant::now();
        let due = flight
            .revival_said
            .is_none_or(|(at, said)| said != may || now.duration_since(at) >= REVIVAL_PROMPT_EVERY);
        if due {
            flight.revival_said = Some((now, may));
            self.flight_ui.message(text);
        }
    }

    /// Enter (the visual designation key) after a loss: fly again, if the
    /// host's Revival allows it, else say why not. `false` when the plane is
    /// not lost, and the key designates as ever (slice F2-V).
    pub(crate) fn fly_again(&mut self) -> bool {
        if self.net_flight.is_none()
            || self.flight_ui.frozen()
            || self.flight_ui.map.open
            || self.respawn_open()
        {
            return false;
        }
        let Some(session) = &mut self.net else {
            return false;
        };
        let Some(text) = session.client.revival_prompt() else {
            return false;
        };
        if session.client.may_fly_again() {
            if let Some((aircraft, loadout)) = self
                .net_flight
                .as_ref()
                .and_then(|f| f.respawn_choice.clone())
            {
                log::info!("Respawn: confirmed choice {aircraft:?}");
                session.client.revive_choice(Some(aircraft), Some(loadout));
            } else {
                log::info!("Respawn: confirmed original aircraft");
                session.client.revive();
            }
            self.flight_ui.message("Flying again...");
        } else {
            self.flight_ui.message(text);
        }
        true
    }

    /// End Mission: the hosting player ends the mission for everyone; any
    /// other leaves the game with its debrief.
    pub(crate) fn leave_session(&mut self) {
        // While the AI flies the plane there is no flight to leave (F2-A).
        if self.leave_away() {
            return;
        }
        let hosting = self.net.as_ref().is_some_and(NetSession::hosting);
        if let Some(session) = &mut self.net {
            session.leave();
        }
        self.message(if hosting {
            "Ending the mission for everyone..."
        } else {
            "Leaving the mission..."
        });
    }

    /// The session is over: its files are done, the flight is put away, and
    /// the player sees the debrief the host sent, or the reason in plain
    /// words on the main menu.
    pub(crate) fn end_session(&mut self, event_loop: &ActiveEventLoop) {
        self.feedback = None;
        self.pending_join = None;
        self.pending_host = None;
        let reason = self.net_ending.take().unwrap_or_default();
        self.end_observing();
        // The lobby goes with the session: its pages are put away, and the
        // reason shows on Direct Connection.
        self.close_lobby();
        let received = self.net.as_mut().and_then(|session| session.debrief.take());
        let cause = self.net_cause();
        let screen = if matches!(cause, debrief::Cause::Host(_)) && received.is_some() {
            received
                .as_ref()
                .map(|wire| self.net_debrief_screen(wire, cause))
        } else {
            self.net.as_ref().and_then(|session| {
                let record = session.client.flight_record();
                if record.sorties.is_empty()
                    || (record.finished && record.debrief_seen && received.is_none())
                {
                    return None;
                }
                let wire = received
                    .as_ref()
                    .or_else(|| record.latest.as_ref().map(|r| &r.debrief));
                let report = wire.map(debrief::report).unwrap_or_default();
                log::info!(
                    "Network: partial report: {} kills, {} sorties, tick {}",
                    report.player.kills.iter().sum::<u32>(),
                    record.sorties.len(),
                    record.tick
                );
                let mut ended = debrief::partial_ending(record.tick, session.left_at.is_none());
                if record.finished {
                    ended.sentence =
                        "The mission ended.\nFinal personal report unavailable.".into();
                }
                Some(crate::debrief::Debrief::networked(
                    report,
                    &self.theater_resources,
                    None,
                    Some(&ended),
                    &debrief::local_pages(record),
                ))
            })
        };
        let capture = self.net.as_ref().and_then(|s| s.capture.clone());
        // A capture of a session that flew converts into a replay once it is
        // closed (below).
        let convert_with = self
            .net
            .as_ref()
            .filter(|s| s.client.clone_stats().snapshots > 0)
            .map(|s| s.resources());
        if let Some(session) = &mut self.net {
            session.disconnect();
        }
        if let Some(host) = self.net.as_mut().and_then(|s| s.hosting.take()) {
            // Host teardown may wait for an in-progress mission build. Keep
            // that wait away from the window when cancelling a loading host.
            match std::thread::Builder::new()
                .name("host-finish".into())
                .spawn(move || drop(host))
            {
                Ok(handle) => self.shelve_background(handle),
                Err(error) => log::warn!("Host cleanup worker: {error}"),
            }
        }
        self.net = None;
        self.net_built = None;
        if let Some(flight) = self.net_flight.take() {
            let stash = flight.stash;
            self.mission_markup = stash.markup;
            self.flight_ui.map.brief = None;
            self.world.0 = Some(stash.world);
            self.combat_view = stash.combat_view;
            self.hornet = stash.hornet;
            self.scenery = stash.scenery;
            if let Some(renderer) = &mut self.renderer {
                renderer.set_scenery(&self.scenery);
                renderer.prepare_aircraft(&self.hornet);
                renderer.combat(&Default::default());
            }
            self.flight_ui.reset_for_flight();
            self.camera.keys.clear();
            self.seat_commands.clear();
            self.gun_key_sent = false;
            self.input.context(true, self.focused);
            self.screen = Screen::Main;
            if let Some(audio) = &self.audio {
                audio.scene(crate::audio::music::Scene::Main);
            }
        }
        if let Some(path) = capture {
            // The finished capture counts in the replays' pruning from now.
            log::info!("Network capture: {}", path.display());
            if let Some(resources) = convert_with {
                crate::replay::net_convert::spawn(path, resources);
            }
        }
        if let Some(screen) = screen {
            match screen {
                Ok(screen) => {
                    self.quick.debrief = Some(screen);
                    self.quick.debrief_to_menu = true;
                    self.screen = Screen::Quick;
                }
                Err(error) => self.message(error.to_string()),
            }
        }
        if !reason.is_empty() {
            self.message(reason);
        }
        if let Some(renderer) = &self.renderer {
            renderer.window.set_title(&match self.screen {
                Screen::Quick => "T.O.R.E-Fighters - Debrief".to_owned(),
                _ => "T.O.R.E-Fighters - Choose Activity".to_owned(),
            });
            renderer.window.request_redraw();
        }
        let _ = event_loop;
    }
}

/// What the presenter saw last frame, to tell what changed.
pub struct Seen {
    pub was_dead: bool,
    pub was_crashed: bool,
    pub was_burning: bool,
}

/// Puts the surface units' states a client frame holds into the game's
/// never-stepped copy of the mission (protocol 22), so every screen that
/// reads a combat row or the surface state (the scenery's rails and wrecks,
/// the flight map) sees the host's: each told unit's hit points, radar and
/// stock, and every other unit as the mission built it (a destroyed one
/// stays destroyed).
pub(crate) fn surface_units_to_world(
    world: &mut tore_world::world::World,
    told: &std::collections::BTreeMap<u32, tore_session::wire::events::SurfaceUnitView>,
) {
    use tore_sim::combat::missiles::TargetRole;
    use tore_world::surface::{MountStock, UnitId};
    let surface = &world.terrain.surface;
    let combat = &mut world.combat;
    for target in combat
        .state
        .targets
        .iter_mut()
        .filter(|t| t.role == TargetRole::Surface)
    {
        match told.get(&target.id) {
            Some(view) => {
                target.hp = if target.hp <= 0 { target.hp } else { view.hp };
                target.radar_emitting = view.radar;
            }
            None if surface.unit(UnitId(target.id)).is_some() => target.radar_emitting = false,
            None => {}
        }
    }
    for arms in &surface.arsenal.units {
        let Some(state) = combat.surface.unit_mut(arms.unit) else {
            continue;
        };
        match told.get(&arms.unit.0) {
            Some(view) => {
                let mut mounts = arms.loads.clone();
                for (stock, seen) in mounts.iter_mut().zip(&view.mounts) {
                    if let Some(rails) = seen.rails {
                        stock.loaded = u32::from(rails);
                    }
                    if seen.spares.is_some() {
                        stock.reserve = seen.spares.map(u32::from);
                    }
                }
                state.mounts = mounts;
                state.armed = true;
            }
            None => {
                state.mounts = Vec::<MountStock>::new();
                state.armed = false;
            }
        }
    }
}

impl TickPresenter<'_> {
    /// Presents one networked frame as single player presents ticks: the
    /// events the host sent as HUD lines, speech, sounds and rumble, and
    /// `ticks` steps of what follows the flight (camera weather, blackout,
    /// vapor, the situation music and the warning tones).
    pub(crate) fn present_net(
        &mut self,
        frame: &FlightFrame,
        events: &[ReceivedEvent],
        name: &dyn Fn(NameIndex) -> Option<String>,
        launches: &crate::audio::LaunchSounds,
        ticks: u64,
        seen: &mut Seen,
    ) {
        let plane = frame.plane.0;
        let mut weapon_cycled = false;
        let mut releases: Vec<String> = Vec::new();
        // Other aircraft's launches: who, from where, the recording and its
        // reach.
        let mut others = self.launch_watch.fresh_guns(frame.picture, plane, launches);
        let mut emissions: Vec<tore_sim::acoustics::Emission> = Vec::new();
        for received in events {
            match &received.event {
                WireEvent::Message { text } => self.flight_ui.message(text.clone()),
                WireEvent::Radio {
                    route,
                    label,
                    text,
                    stems,
                    ..
                } => {
                    let stems: Vec<String> = stems.iter().filter_map(|stem| name(*stem)).collect();
                    match route {
                        comms::Route::Radio | comms::Route::Airport => {
                            self.flight_ui.message(format!("{label}: '{text}'"));
                            if let Some(audio) = self.audio {
                                if *route == comms::Route::Airport {
                                    audio.airport_speech(&stems);
                                } else {
                                    audio.speech(&stems);
                                }
                            }
                        }
                        comms::Route::Direct => {
                            if let (Some(audio), Some(stem)) = (self.audio, stems.first()) {
                                audio.direct_voice(stem);
                            }
                        }
                    }
                }
                WireEvent::Tower { stem } => {
                    if let Some(audio) = self.audio {
                        match stem.and_then(name) {
                            Some(stem) => audio.airport_radio(&[stem.as_str()]),
                            None => audio.cancel_airport_radio(),
                        }
                    }
                }
                WireEvent::OrderVoice { stems } => {
                    if let Some(audio) = self.audio {
                        let stems: Vec<String> =
                            stems.iter().filter_map(|stem| name(*stem)).collect();
                        let refs: Vec<&str> = stems.iter().map(String::as_str).collect();
                        audio.radio(&refs, true);
                    }
                }
                WireEvent::OrderReply { .. } => {}
                WireEvent::WeaponCycled => weapon_cycled = true,
                WireEvent::Release { sound, .. } => {
                    if let Some(sound) = name(*sound) {
                        releases.push(sound);
                    }
                }
                // The player's own launch is its Release; any other
                // shooter's (aircraft or surface unit) is heard from where
                // it fired. The shared mixer groups gun rounds into bursts.
                WireEvent::Launch {
                    shooter,
                    projectile,
                    weapon,
                } => {
                    if *shooter != plane
                        && let Some(record) = name(*weapon)
                        && let Some(position) = crate::audio::shooter_position(
                            frame.picture,
                            *shooter,
                            Some(*projectile),
                        )
                        && let Some(heard) = launches.heard(&record, *shooter, position)
                    {
                        others.push(heard);
                    }
                }
                WireEvent::Feedback { rumble } => self.input.feedback(rumble.event()),
                WireEvent::YourAircraftExploded { on_impact } => {
                    self.flight_ui.message(if *on_impact {
                        "Your aircraft exploded on impact"
                    } else {
                        "Your aircraft exploded"
                    });
                }
                WireEvent::WingEjection {
                    message, friendly, ..
                } => {
                    self.flight_ui.message(message.clone());
                    if *friendly && let Some(audio) = &self.audio {
                        audio.wingman_ejected();
                    }
                }
                WireEvent::Sound {
                    kind,
                    position,
                    arrived,
                    from,
                } => emissions.push(tore_sim::acoustics::Emission {
                    kind: *kind,
                    position: position
                        .map(|v| v as f64 * tore_session::wire::entity::POSITION_STEP),
                    arrived: *arrived,
                    own: *from == Some(plane),
                }),
                // The data link's changes: what they show is in the
                // readout, and the sort warning's line and beep are Message
                // and Radio events (slice G7).
                WireEvent::Link(_) => {}
                // Their effects are in the picture, the regenerated devices
                // and the view rig, or arrive as the entities they make.
                WireEvent::Effect { .. }
                | WireEvent::Mark { .. }
                | WireEvent::GroundDestroyed { .. }
                | WireEvent::Countermeasure { .. }
                | WireEvent::GunBurst { .. }
                | WireEvent::SurfaceBurst { .. }
                | WireEvent::SurfaceBurstEnd { .. }
                | WireEvent::SurfaceUnit(_)
                | WireEvent::CityFlattened { .. }
                | WireEvent::Stores { .. } => {}
            }
        }
        if weapon_cycled {
            show_selected_weapon_page(frame, &self.world.combat, self.instruments);
        }

        let flight = frame.flight;
        let dead = flight.systems.pilot.dead || flight.escape.is_some();
        if let Some(view) = self.flight_ui.pilot_death_view(seen.was_dead, dead) {
            *self.flight_view = view;
            self.view_rig.select(flight_views::Reference::Player);
        }
        seen.was_dead = dead;

        // The tick-rate presentation of the flight.
        self.flown_net(frame, ticks);
        let danger = tore_sim::ejection::assess(flight, |x, z| {
            f64::from(self.world.terrain.height(x as f32, z as f32))
        })
        .is_some();
        if let Some(audio) = self.audio {
            audio.ejection(frame.previous, flight, danger);
        }
        if let Some(audio) = self.audio {
            for _ in 0..ticks {
                let music = self.flight_music.step(frame, &[], &self.world.terrain);
                audio.situation(&music.inputs, music.now);
                audio.rwr(self.rwr_warnings.step(
                    frame.readout.tick,
                    crate::rwr_tone::inbound(&frame.readout, flight.position),
                    &frame.readout.rwr.locks,
                    flight.escape.is_some() || flight.systems.pilot.dead,
                ));
            }
            audio.seeker(frame.readout.seeker.tone);
        }
        if let Some(audio) = self.audio {
            let scene =
                flight_views::Scene::from_frame(frame, flight, self.world.ai_wings.as_ref());
            let scene = self
                .view_rig
                .prepare(scene, *self.flight_view, &self.world.terrain);
            let listener_camera = self
                .view_rig
                .clone()
                .camera(
                    *self.flight_view,
                    &scene,
                    self.hornet
                        .camera(flight, *self.flight_view, Default::default()),
                    crate::look::combine(
                        self.flight_ui.look,
                        self.head_look,
                        matches!(*self.flight_view, 1 | 2),
                    ),
                    self.flight_ui.zoom,
                )
                .unwrap_or_else(|_| self.hornet.camera(flight, 0, Default::default()));
            let basis = tore_sim::attitude::Basis::new(
                f64::from(listener_camera.yaw),
                f64::from(listener_camera.pitch),
                -f64::from(listener_camera.roll),
            );
            let loops = crate::audio::loop_sources(
                frame.picture,
                self.engine_sounds.of(self
                    .combat_view
                    .models
                    .iter()
                    .map(|model| &*model.kind)
                    .chain([&**self.hornet])),
            );
            let sources = crate::audio::snapshot_sources(frame.picture);
            // The player's own release names the recording only: it fades
            // over the default distance. Another aircraft's launch names
            // its weapon record, which gives both.
            let own_releases: Vec<crate::audio::Launch> = releases
                .iter()
                .map(|sound| crate::audio::Launch::named(sound))
                .chain(others.iter().map(|h| {
                    crate::audio::Launch::by(h.shooter, h.position, &h.sound, h.max_distance_ft)
                        .with_gun(h.gun)
                }))
                .collect();
            for step in 0..ticks.max(1) {
                let first = step == 0;
                audio.spatial_tick(
                    tore_sim::acoustics::Listener {
                        position: listener_camera.position,
                        right: basis.right,
                        view: *self.flight_view,
                        external: !self.view_rig.cockpit(*self.flight_view),
                        own: Some(plane),
                    },
                    &sources,
                    if first { &emissions } else { &[] },
                    if first { &own_releases } else { &[] },
                    flight.position,
                    Some((flight.position, flight.velocity)),
                    &loops,
                );
            }
        }
        if flight.crashed && !seen.was_crashed && flight.escape.is_none() {
            self.input.feedback(tore_input::FeedbackEvent::Crash);
        }
        seen.was_crashed = flight.crashed;
        let burning = flight.afterburner_active();
        if burning && !seen.was_burning {
            self.input
                .feedback(tore_input::FeedbackEvent::AfterburnerEngaged);
        }
        seen.was_burning = burning;
        self.input.afterburner_feedback(burning);
        self.input.feedback_tick();
    }

    /// `flown` for a networked frame: the camera slots' weather, the view
    /// rig, blackout and redout, the wing vapor and the control sounds,
    /// stepped `ticks` times where single player steps them once a tick.
    fn flown_net(&mut self, frame: &FlightFrame, ticks: u64) {
        let flight = frame.flight;
        let speed = flight.speed;
        let scene = flight_views::Scene::from_frame(frame, flight, self.world.ai_wings.as_ref());
        let scene = self
            .view_rig
            .prepare(scene, *self.flight_view, &self.world.terrain);
        let weather_view = self
            .view_rig
            .camera(
                *self.flight_view,
                &scene,
                self.hornet
                    .camera(flight, *self.flight_view, Default::default()),
                crate::look::combine(
                    self.flight_ui.look,
                    self.head_look,
                    matches!(*self.flight_view, 1 | 2),
                ),
                self.flight_ui.zoom,
            )
            .unwrap_or_else(|_| self.hornet.camera(flight, 0, Default::default()));
        self.view_rig.observe(&scene);
        let other = self
            .view_rig
            .other_camera(
                &scene,
                self.hornet
                    .camera(flight, self.view_rig.other_view(), Default::default()),
            )
            .ok();
        let target = self
            .combat_view
            .target_camera(&self.world.combat, &frame.readout, flight);
        let mirror = crate::mirrors::camera(flight);
        let panel = self.hornet.panel_camera(flight, 2);
        for _ in 0..ticks {
            self.world.terrain.weather.step();
            self.scenery
                .step_view_weather(&self.world.terrain, &weather_view, speed);
            self.scenery
                .step_view_weather(&self.world.terrain, &mirror, speed);
            self.scenery
                .step_view_weather(&self.world.terrain, &panel, speed);
            for camera in other.iter().chain(target.iter()) {
                self.scenery
                    .step_view_weather(&self.world.terrain, camera, speed);
            }
            self.g_effects.step(
                flight.g,
                !self.flight_ui.cheats.no_g_effects && !flight.crashed && flight.native.is_none(),
            );
            if let Some(level) = self.perf_blackout {
                self.g_effects.blackout = level;
            }
            if let Some(points) = self.hornet.streamer_points(flight) {
                self.vapor.step(self.world.terrain.weather.ticks(), points);
            }
        }
        if let Some(audio) = self.audio {
            audio.controls(frame.previous, flight);
        }
        let _ = flight::DT;
    }
}

/// Adds each of `spawned`, a revival's new plane, to `world`, a copy of the
/// mission the game built, unless it holds that plane already.
fn add_spawned(world: &mut crate::world::World, spawned: &[tore_session::wire::messages::Spawned]) {
    for spawned in spawned {
        let plane = crate::seats::PlaneId(spawned.plane);
        if world.roster.plane(plane).is_some() {
            continue;
        }
        let new = crate::world::revive::NewPlane {
            plane,
            slot: crate::seats::Slot {
                wing: spawned.wing,
                member: spawned.member,
            },
            aircraft: spawned.aircraft,
            spawn: spawned.spawn.clone(),
        };
        if let Err(error) = world.add_plane(&new) {
            log::warn!(
                "Network: plane {} could not be added: {error}",
                spawned.plane
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::add_spawned;
    use crate::seats::{PlaneId, SeatId, SeatInput};
    use crate::world::{MissionCommand, Seating, TickOutput, World, revive::RevivalWeapons};
    use tore_formats::aircraft::AircraftId;
    use tore_session::wire::messages::Spawned;
    use tore_world::mission::{MissionSpec, Start};
    use tore_world::test_support::resources::{THEATER, resources};

    fn open_mission() -> World {
        let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
        spec.wings[0].count = 2;
        spec.wings[3].count = 2;
        spec.start = Start::Airborne {
            altitude_ft: 10_000,
        };
        World::new(&spec, &resources(), Seating::Open).unwrap()
    }

    fn step(world: &mut World, commands: &[MissionCommand]) {
        let input = SeatInput {
            seat: SeatId(0),
            tick: world.tick(),
            ..SeatInput::default()
        };
        let mut out = TickOutput::default();
        world
            .step_with(commands, &[input], &mut out, |_, _| Ok(()))
            .unwrap();
    }

    /// A game's copy of the mission gets each revival's plane once, from the
    /// Spawned messages the client kept (slice F2-V).
    #[test]
    fn a_games_copy_of_the_mission_adds_each_spawned_plane_once() {
        let mut host = open_mission();
        host.take_plane(SeatId(0), PlaneId(0)).unwrap();
        let start = host
            .side_mean(tore_sim::ai::launch::Side::Friendly)
            .unwrap();
        host.cockpits[0].flight.systems.pilot.dead = true;
        step(&mut host, &[]);
        let spawn = host
            .revival_spawn(SeatId(0), start, 30_000., None, RevivalWeapons::Missiles)
            .unwrap();
        let new = host.revival_plane(SeatId(0), &spawn).unwrap();
        step(
            &mut host,
            &[MissionCommand::Revive {
                seat: SeatId(0),
                spawn: Box::new(spawn.clone()),
            }],
        );
        let spawned = Spawned {
            plane: new.plane.0,
            tick: 2,
            wing: new.slot.wing,
            member: new.slot.member,
            aircraft: new.aircraft,
            spawn,
        };
        let mut copy = open_mission();
        add_spawned(&mut copy, std::slice::from_ref(&spawned));
        add_spawned(&mut copy, &[spawned]);
        assert_eq!(copy.roster.planes().len(), 5);
        assert_eq!(copy.roster.plane(new.plane).unwrap().slot, new.slot);
        // The game seats its player in it as it seats any plane.
        copy.take_plane(SeatId(0), new.plane).unwrap();
    }

    /// Protocol 22: the surface units the host told of take their hit
    /// points, radar and spares in the game's copy of the mission; the
    /// others are as built, and a destroyed one stays destroyed.
    #[test]
    fn told_surface_units_reach_the_games_copy_of_the_mission() {
        use tore_session::wire::events::{MountView, SurfaceUnitView};
        use tore_world::surface::{SURFACE_UNIT_BASE, UnitId};
        use tore_world::test_support::surface::{spec_with_target, surface_resources, target};
        let spec = spec_with_target(&target("QUCITY", 3, 3, 9));
        let mut world = tore_world::world::World::new(
            &spec,
            &surface_resources(),
            tore_world::world::Seating::Open,
        )
        .unwrap();
        let bunker = SURFACE_UNIT_BASE + 12;
        let gun = UnitId(SURFACE_UNIT_BASE + 6);
        // A gun with a magazine and spares, as the surface's arms give one.
        let arms = world
            .terrain
            .surface
            .arsenal
            .units
            .iter()
            .find(|a| a.unit == gun)
            .cloned();
        let row = |world: &tore_world::world::World, id: u32| {
            world
                .combat
                .state
                .targets
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.hp, t.radar_emitting))
                .unwrap()
        };
        let (full, _) = row(&world, bunker);
        let mut told = std::collections::BTreeMap::new();
        told.insert(
            bunker,
            SurfaceUnitView {
                unit: bunker,
                hp: full - 7,
                radar: true,
                mounts: Vec::new(),
            },
        );
        if let Some(arms) = &arms {
            told.insert(
                gun.0,
                SurfaceUnitView {
                    unit: gun.0,
                    hp: row(&world, gun.0).0,
                    radar: false,
                    mounts: arms
                        .loads
                        .iter()
                        .map(|_| MountView {
                            rails: None,
                            spares: Some(0),
                        })
                        .collect(),
                },
            );
        }
        super::surface_units_to_world(&mut world, &told);
        assert_eq!(row(&world, bunker), (full - 7, true));
        if arms.is_some() {
            let state = world.combat.surface.unit(gun).unwrap();
            assert!(state.armed);
            assert!(
                state
                    .mounts
                    .iter()
                    .all(|m| m.reserve.is_none_or(|r| r == 0))
            );
        }
        // Told nothing more: as built again, but a destroyed one stays so.
        world
            .combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == bunker)
            .unwrap()
            .hp = 0;
        super::surface_units_to_world(&mut world, &std::collections::BTreeMap::new());
        assert_eq!(row(&world, bunker), (0, false));
        assert!(!world.combat.surface.unit(gun).unwrap().armed);
    }
}
