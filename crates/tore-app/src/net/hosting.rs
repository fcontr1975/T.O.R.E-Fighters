//! The host inside the game (slice EF3 of stage E): the hosting player's game
//! runs the stage D `tore_session::Host` on a thread of its own, with its
//! own fixed 120 Hz clock and its UDP socket on the game port, exactly as the
//! dedicated server runs it, and joins it as an ordinary client over the
//! in-process link (`tore_net::link`). See docs/ARCHITECTURE.md, "The host
//! inside the game (stage E)".
//!
//! The thread builds the `Host` itself, so the window never waits for the
//! mission to build, and then runs the dedicated server's loop: receive,
//! update, transmit, wait until the next wake (`tore_net::wait_until`, which
//! sleeps and then spins). The game talks to it through two channels:
//! [`Command`]s in and [`Report`]s out. Every choice here is an agent
//! decision unless it is credited.
//!
//! - **Stop.** [`Command::Stop`] is the hosting player leaving the game
//!   (EF4): a flying mission ends for everyone with "the host left the
//!   game" and their debriefs, every player is told the host left and is
//!   disconnected once that is acknowledged. The thread waits at most
//!   [`STOP_GRACE`] for that, then disconnects whoever is left with "server
//!   stopping", closes its socket and ends; the game waits at most
//!   [`JOIN_LIMIT`] for it. A game that drops its end of the channel stops
//!   the thread the same way.
//! - **The lobby** (EF4). The hosting player's own connection is the house
//!   ([`HostConfig::house`] is the link's address), which wears the crown
//!   when it joins, so the King's verbs (change the mission and the
//!   settings, start, end the mission, kick, pass the crown) go over that
//!   connection as messages, as a remote King's do since phase 2, and not
//!   through [`Command`].
//! - **Panic.** A panic on the thread is caught: it tries once to disconnect
//!   every player with "server stopping" (best effort: the host may be
//!   broken), closes the socket and reports [`End::Panicked`], which the game
//!   shows as "The game you were hosting stopped: ...". A remote player the
//!   disconnect does not reach sees the client's own timeout message.
//! - **The port** is closed before the thread reports its end, so a new host
//!   on the same port binds at once.
//! - **On time on macOS** (EF-M). The thread holds an `NSProcessInfo`
//!   activity, latency-critical and user-initiated, from its start to its
//!   end, so App Nap does not slow a hidden hosting game, and once the
//!   mission is built it gives itself a Mach time-constraint policy for the
//!   120 Hz tick (`tore_realtime_native`), whose timers macOS does not
//!   coalesce. It logs once what took ("Host: macOS real-time scheduling
//!   on", or why not). Both do nothing on Linux and Windows.
//! - **Listing** (slice I3). A game started with a [`Listing`] reads and
//!   writes its socket through a `tore_net::master::HostListing`, so the
//!   master's datagrams never reach the host, and lists itself on the
//!   Internet Lobby while it is told to ([`Command::SetListed`]). It reports
//!   where the listing stands ([`Report::Listing`]) and unregisters as the
//!   thread stops, before its socket closes. A game started without one
//!   (Direct Connection) never talks to the master until its King makes it
//!   public. *Agent decision:* the listing goes beside [`HostSetup`]
//!   ([`HostThread::start_listed`]) rather than in it, so the hosts the
//!   tests build stay as they are.
//! - **The King's Visibility** (stage F phase 2, slice F2-1). The thread
//!   follows the host's `visibility` setting as it follows
//!   [`Command::SetListed`]: when the King makes the game public it is
//!   listed, and when the King makes it local or hidden it is taken off.
//!   A game started without a listing gets one then, on the public master
//!   and with no telemetry (agent decision: the King asked for the listing;
//!   the statistics wait for the Internet Lobby's notice). Only a change
//!   counts, so a game started listed while its setting says local stays
//!   listed until the King or the screen says otherwise.
//! - **Port mapping** (stage J, slice J4b). A game started with a
//!   [`Forward`] ([`HostThread::start_forwarded`]) asks the router to
//!   forward the game port on a thread of its own ([`forward::Forwarder`]),
//!   from the moment the socket is bound. The thread's news reaches the
//!   host thread's loop, which gives the listing the Mapped candidate
//!   (`HostListing::set_mapped`), keeps the telemetry value for the Report
//!   and tells the game ([`Report::Forward`], whose lines the lobby shows).
//!   When the host stops, the mapping is removed before the thread reports
//!   its end, in at most [`forward::REMOVE_WAIT`]; the removal starts as
//!   soon as the stop is asked for, so it runs while the players are told.
//! - **Host migration** (stage K, slice K7a; docs/ARCHITECTURE.md, "Host
//!   migration and rejoin"). The host keeps standbys
//!   (`Host::set_standbys_enabled`) and journals the listing's part
//!   (`Host::set_listing_part`) whenever the listing may have changed. A
//!   stop with a ready standby hands the game over (`Host::hand_over`)
//!   instead of ending it, and the thread ends without a word to anyone
//!   once the standby has the last tick ([`End::HandedOver`]), the listing
//!   released for the new host. A host that loses every player at once or
//!   hears it was taken over asks standby 1, every [`ASK_EVERY`], whether it
//!   hosts now; when it answers so the host steps down and the thread hands
//!   the socket back to the game ([`HostThread::take_socket`],
//!   [`End::SteppedDown`]), whose client resumes with the new host. A game
//!   that takes the game over starts its thread with
//!   [`HostThread::take_over`]: the thread builds the new host from the
//!   game's standby (`standby::takeover::take_over_thread`), on the socket
//!   the game joined with, behind the game's peers router, which still
//!   hands the old host's datagrams to the game's client ([`Side`]), and it
//!   resumes the old host's listing from the listing part.
pub mod forward;

use crate::net::{
    options::HostOptions,
    session::{Join, NetSession, Transport, build_id},
};
use forward::{Forward, Forwarder, News};
use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    io,
    net::SocketAddr,
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tore_net::master::candidate::canonical;
use tore_net::master::{
    Build, HostListing, HostRendezvous, HostTally, ListingState, PortMapping, RendezvousEvent,
    Role,
    rendezvous::{ListingPart, state_text},
};
use tore_net::peers::{Peers, Route};
use tore_net::{
    Datagrams, Entropy, LINK_ADDRESS, LinkEnd, Linked, Listen, Platform, RealClock, SPIN_MARGIN,
    ServerSocket, plan_wake, wait_until,
};
use tore_realtime_native::{Activity, real_time_thread, summary};
use tore_session::{
    AfterEnd, CrownRule, Host, HostConfig, HostLog, LeaveReason, OpenPlanes, Phase, StartMode,
    host::{Resumption, TICKS_PER_SECOND, content::ContentLog, hosting_answer, reach_packet},
    settings::{Visibility, number},
    standby::{StandbyThread, takeover::take_over_thread},
    wire::messages::EndReason,
};
use tore_world::mission::MissionSpec;

/// How long the thread waits, after a stop, for the players to acknowledge
/// their debriefs before it disconnects them anyway.
pub const STOP_GRACE: Duration = Duration::from_millis(1500);
/// How long the game waits for the thread to end after asking it to stop.
pub const JOIN_LIMIT: Duration = Duration::from_secs(3);
/// How often a host that lost its players asks standby 1 whether it hosts
/// now (stage K; agent decision, as slice K4's tests ask).
pub const ASK_EVERY: Duration = Duration::from_secs(1);
/// How often the thread looks at its standbys for the log (agent decision).
const FIGURES_EVERY: Duration = Duration::from_millis(250);
/// The thread's stack: a main thread's 8 MiB, as the dedicated server's host
/// has, not a spawned thread's 2 MiB.
const STACK: usize = 8 << 20;

/// What the thread builds the host from.
pub struct HostSetup {
    pub spec: MissionSpec,
    /// The import, which the host builds the mission from.
    pub resources: Arc<BTreeMap<String, Vec<u8>>>,
    pub config: HostConfig,
    /// Where the socket listens.
    pub listen: Listen,
    pub port: u16,
}

/// How a hosted game is listed on the Internet Lobby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The master server, `HOST` or `HOST:PORT`.
    pub master: String,
    /// Listed from the start; [`Command::SetListed`] changes it.
    pub listed: bool,
    /// The anonymous install id when telemetry is on, else `None`: then
    /// nothing but the listing is sent. *Agent decision:* `--host --list`
    /// gives none until the Internet Lobby's one-time notice and switch
    /// exist (slice I4).
    pub install_id: Option<u64>,
}

impl Listing {
    /// Listed from the start on `master` (the public one when `None`), with
    /// no telemetry.
    pub fn listed(master: Option<&str>) -> Self {
        Self {
            master: master
                .unwrap_or(tore_net::master::DEFAULT_MASTER)
                .to_owned(),
            listed: true,
            install_id: None,
        }
    }
}

/// What the game asks of the thread. The lobby's verbs are the King's
/// messages over the game's own connection, not commands.
#[derive(Debug)]
pub enum Command {
    /// The hosting player leaves: end the game for everyone, politely, and
    /// stop.
    Stop,
    /// List the game on the Internet Lobby, or take it off. Ignored by a
    /// game started without a [`Listing`].
    // The Internet Lobby (I4) sends it; until then only the tests do. The
    // King's Visibility does the same from the host's own setting
    // (`follow_visibility`).
    #[cfg_attr(not(test), allow(dead_code))]
    SetListed(bool),
    /// The Debug menu's AI test settings (observation rate, lean sensors):
    /// the host's setting governs the simulation, from the next tick.
    SetAiTest(tore_sim::ai::test_settings::AiTestSettings),
    /// Panic on the thread, for the tests of the panic rule.
    #[cfg(test)]
    Panic,
    /// End the thread at once without a word to anyone, as a machine that
    /// loses its power or its network would (the tests of stage K).
    #[cfg(test)]
    Vanish,
    /// Cut the game port off the network for a while: nothing arrives on
    /// it and nothing leaves it, while the link to the game's own player
    /// works on (the tests of stage K).
    #[cfg(test)]
    Cut(Duration),
}

/// What the thread tells the game, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// The mission is built and the host takes joins.
    Started { aircraft: usize, capacity: usize },
    /// The host's log: joins, refusals, seats, departures, the mission's
    /// start and end, overloads and faults.
    Log(HostLog),
    /// A line about a player's content or the gaps (stage L), with the host
    /// tick, for the game's log as the dedicated server's has it.
    Content(ContentLog),
    /// The host's phase changed.
    Phase(Phase),
    /// A socket error, noted and survived, as the dedicated server notes it.
    Note(String),
    /// Where the game's listing on the Internet Lobby stands, when it
    /// changes (a game started with a [`Listing`]).
    Listing(ListingState),
    /// What the router did about the game port (a game started with a
    /// [`Forward`]): the lines for the player, and the Mapped candidate.
    Forward(News),
    /// The thread took the game over (stage K): the new host is built and
    /// its world stands at `tick`, T, where the game's own player resumes
    /// (`Client::host_here`).
    TookOver { tick: u64 },
    /// A line about host migration for the game's log: the standbys, a
    /// takeover, players resuming, a handover, a step-down.
    Migration(String),
    /// A line about host migration for the player to read, in Messages (stage
    /// K, slice K7b): a player who came to the new host, or did not.
    Said(String),
    /// Whether a standby is ready to take the game over now, when that
    /// changes (slice F2-O4: the hosting player's Leave Game says whether it
    /// hands the game over or ends it).
    Standby { ready: bool },
    /// The thread has ended, and why. Nothing follows.
    Ended(End),
}

/// Why the thread ended.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    /// The game asked it to stop.
    Stopped,
    /// The host stopped by itself: its mission ended.
    Finished,
    /// The mission could not be built, or the settings were refused.
    BuildFailed(String),
    /// The thread panicked, with the panic's message.
    Panicked(String),
    /// Stopped with a ready standby: the game was handed over to it, and
    /// the thread ended without a word to anyone (stage K).
    HandedOver,
    /// Another game hosts the session now, at the address that said so:
    /// the host stepped down and the socket is the game's again
    /// ([`HostThread::take_socket`]).
    SteppedDown(SocketAddr),
    /// The takeover could not build its host: the socket is the game's
    /// again, and its client joins whoever else takes over.
    TakeoverFailed(String),
    /// The host moved the game in the lobby to a better machine (K6's
    /// calculated host, by a handover) while its player stays: the socket
    /// is the game's again, and its client races standby 1 at these
    /// addresses like any other player.
    MovedOn(Vec<SocketAddr>),
}

impl End {
    /// What the hosting player is told when the end is not one their own
    /// session reports: a failed build or a panic.
    pub fn failure(&self) -> Option<String> {
        match self {
            Self::Stopped
            | Self::Finished
            | Self::HandedOver
            | Self::SteppedDown(_)
            | Self::TakeoverFailed(_)
            | Self::MovedOn(_) => None,
            Self::BuildFailed(text) => Some(format!("The game could not be hosted: {text}")),
            Self::Panicked(text) => Some(format!("The game you were hosting stopped: {text}")),
        }
    }
}

/// The host thread, as the game holds it. Dropping it stops the thread.
pub struct HostThread {
    commands: Sender<Command>,
    reports: Receiver<Report>,
    handle: Option<JoinHandle<()>>,
    addresses: Vec<SocketAddr>,
    end: Option<End>,
    /// Lines for the hosting player, not yet taken ([`HostThread::take_notes`]).
    notes: Vec<String>,
    /// The socket, handed back when the thread ends without ending the
    /// game (a step-down, a takeover that failed).
    handback: Arc<Mutex<Option<ServerSocket>>>,
}

/// What a game that takes the game over gives its new hosting thread
/// (stage K, slice K7a).
pub struct TakeoverSetup {
    /// The game's standby, which holds the world and the parts.
    pub standby: StandbyThread,
    pub resources: Arc<BTreeMap<String, Vec<u8>>>,
    /// The hosting game's settings ([`takeover_config`]).
    pub config: HostConfig,
    /// Its own player, the moment and the old host's clock, on `clock`.
    pub resumption: Resumption,
    /// The socket the game joined with.
    pub socket: ServerSocket,
    /// The game's session clock: the new host keeps the old one's pace from
    /// the game's own estimate of it, on the same clock.
    pub clock: RealClock,
    /// The router's port mapping, as hosting asks for it.
    pub forward: Option<Forward>,
}

/// The game's side of the socket a game that took over hosts on: the old
/// host's datagrams, which still go to the game's client while its old
/// connection lasts, and that address as the game's client last named it.
#[derive(Debug, Default)]
pub struct Side {
    state: Mutex<SideState>,
}

#[derive(Debug, Default)]
struct SideState {
    inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    old_host: Option<SocketAddr>,
}

/// Datagrams the side holds at most, as a link's queue (agent decision).
const SIDE_QUEUE: usize = tore_net::link::LINK_QUEUE;

impl Side {
    fn state(&self) -> std::sync::MutexGuard<'_, SideState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The old host's address while the game's client keeps its connection
    /// to it (`Client::old_host`), set by the game on every turn.
    pub fn set_old_host(&self, old_host: Option<SocketAddr>) {
        self.state().old_host = old_host;
    }

    fn old_host(&self) -> Option<SocketAddr> {
        self.state().old_host
    }

    fn push(&self, from: SocketAddr, datagram: &[u8]) {
        let mut state = self.state();
        if state.inbox.len() < SIDE_QUEUE {
            state.inbox.push_back((from, datagram.to_vec()));
        }
    }

    /// The next datagram for the game's client, into `buf`.
    pub fn pop(&self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        let (from, datagram) = self.state().inbox.pop_front()?;
        let length = datagram.len().min(buf.len());
        buf[..length].copy_from_slice(&datagram[..length]);
        Some((length, from))
    }
}

/// An old host asking standby 1 whether it hosts now.
#[derive(Debug)]
struct Asking {
    session: u64,
    nonce: u64,
    /// Standby 1's addresses when the host first asked.
    to: Vec<SocketAddr>,
    last: Option<Duration>,
}

/// The hosting thread's socket: the game port, with what stage K puts in
/// front of it. An old host's answer from standby 1 is taken here; a game
/// that took over has its peers router here, which hands the old host's
/// datagrams to the game's [`Side`] and everything else to the host.
#[derive(Debug)]
pub struct GamePort {
    socket: ServerSocket,
    now: Duration,
    peers: Option<Peers>,
    side: Option<Arc<Side>>,
    asking: Option<Asking>,
    answered: Option<SocketAddr>,
    /// Cut off the network until then, on the thread's clock (tests).
    #[cfg(test)]
    cut_until: Option<Duration>,
}

impl GamePort {
    fn new(socket: ServerSocket) -> Self {
        Self {
            socket,
            now: Duration::ZERO,
            peers: None,
            side: None,
            asking: None,
            answered: None,
            #[cfg(test)]
            cut_until: None,
        }
    }

    /// Whether the port is cut off the network now (tests only).
    fn cut(&self) -> bool {
        #[cfg(test)]
        if self.cut_until.is_some_and(|until| self.now < until) {
            return true;
        }
        false
    }
}

impl Datagrams for GamePort {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        if self.cut() {
            return Ok(());
        }
        self.socket.send_datagram(to, datagram)
    }

    fn can_send_to(&self, to: SocketAddr) -> bool {
        self.socket.can_send_to(to)
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        loop {
            let Some((length, from)) = self.socket.recv_datagram(buf)? else {
                return Ok(None);
            };
            if self.cut() {
                continue;
            }
            // An IPv4 peer on a dual-stack socket arrives at its mapped
            // address: the peers router and the clients name it plainly.
            let from = canonical(from);
            let datagram = &buf[..length];
            if let Some(asking) = &self.asking
                && hosting_answer(datagram, asking.session, asking.nonce)
            {
                self.answered.get_or_insert(from);
                continue;
            }
            let Some(peers) = self.peers.as_mut() else {
                return Ok(Some((length, from)));
            };
            match peers.route(self.now, from, datagram) {
                Route::Host => return Ok(Some((length, from))),
                Route::Client => {
                    if let Some(side) = &self.side {
                        side.push(from, datagram);
                    }
                }
                Route::Taken => {}
            }
        }
    }
}

/// How the thread gets its host.
enum Start {
    /// A new host for `spec`.
    New {
        spec: Box<MissionSpec>,
        config: HostConfig,
    },
    /// The game takes the game over from its standby.
    TakeOver {
        standby: StandbyThread,
        config: HostConfig,
        resumption: Resumption,
    },
}

impl HostThread {
    /// Binds the socket, so a port in use is refused at once, and starts the
    /// thread, which builds the host. Returns the game's end of the link.
    // The game hosts through `start_listed`; the tests start plain hosts.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn start(setup: HostSetup) -> Result<(Self, LinkEnd), String> {
        Self::start_listed(setup, None)
    }

    /// [`HostThread::start`], with the game listed on the Internet Lobby as
    /// `listing` says. A master address that cannot be read is refused at
    /// once.
    pub fn start_listed(
        setup: HostSetup,
        listing: Option<Listing>,
    ) -> Result<(Self, LinkEnd), String> {
        Self::start_forwarded(setup, listing, None)
    }

    /// [`HostThread::start_listed`], with the router asked to forward the
    /// game port as `forward` says (slice J4b): `None` asks nothing.
    pub fn start_forwarded(
        setup: HostSetup,
        listing: Option<Listing>,
        forward: Option<Forward>,
    ) -> Result<(Self, LinkEnd), String> {
        let HostSetup {
            spec,
            resources,
            mut config,
            listen,
            port,
        } = setup;
        let content = config
            .content
            .clone()
            .unwrap_or_else(|| tore_session::host::content::GameContent::shared(&resources));
        config.build.compatibility = tore_session::compatibility::current(&content.content);
        config.content = Some(content);
        let socket = ServerSocket::bind(listen, port).map_err(|error| {
            format!(
                "Cannot host on UDP port {port}: {error}. Is another game or server using it? \
                 Choose another with --port."
            )
        })?;
        let addresses = socket.local_addresses();
        let port = addresses.first().map_or(port, |a| a.port());
        let listing = match listing {
            Some(listing) => {
                let mut host_listing = new_listing(
                    &listing.master,
                    &config.build,
                    listing.install_id,
                    port,
                    Duration::ZERO,
                )?;
                host_listing.set_listed(listing.listed, Duration::ZERO);
                Some(host_listing)
            }
            None => None,
        };
        // After the checks above: a refused start asks the router nothing.
        let forwarder = forward.map(|forward| Forwarder::start(forward, port));
        Self::spawn(
            Start::New {
                spec: Box::new(spec),
                config,
            },
            resources,
            GamePort::new(socket),
            Listings {
                listing,
                port,
                forwarder,
                mapped: None,
                port_mapping: PortMapping::NotTried,
            },
            RealClock::new(),
            addresses,
        )
    }

    /// Takes the game over (stage K, slice K7a): the thread builds the new
    /// host from the game's standby on the socket the game joined with, and
    /// reports [`Report::TookOver`] once it hosts. Returns the game's end of
    /// the link and the game's side of the socket, which carries the old
    /// host's datagrams while the game's client keeps its old connection.
    pub fn take_over(setup: TakeoverSetup) -> Result<(Self, LinkEnd, Arc<Side>), String> {
        let TakeoverSetup {
            standby,
            resources,
            config,
            resumption,
            socket,
            clock,
            forward,
        } = setup;
        let addresses = socket.local_addresses();
        let port = addresses.first().map_or(0, |a| a.port());
        let side = Arc::new(Side::default());
        let mut game_port = GamePort::new(socket);
        let mut peers = Peers::new(tore_session::wire::PROTOCOL_VERSION, Entropy::System);
        peers.set_hosting(true, None);
        game_port.peers = Some(peers);
        game_port.side = Some(Arc::clone(&side));
        let forwarder = forward.map(|forward| Forwarder::start(forward, port));
        let (thread, link) = Self::spawn(
            Start::TakeOver {
                standby,
                config,
                resumption,
            },
            resources,
            game_port,
            Listings {
                listing: None,
                port,
                forwarder,
                mapped: None,
                port_mapping: PortMapping::NotTried,
            },
            clock,
            addresses,
        )?;
        Ok((thread, link, side))
    }

    /// Starts the thread on `port`'s socket.
    fn spawn(
        start: Start,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        port: GamePort,
        listings: Listings,
        clock: RealClock,
        addresses: Vec<SocketAddr>,
    ) -> Result<(Self, LinkEnd), String> {
        let (host_end, game_end) = tore_net::link::pair();
        let (command_sender, commands) = mpsc::channel();
        let (report_sender, reports) = mpsc::channel();
        let handback = Arc::new(Mutex::new(None));
        let theirs = Arc::clone(&handback);
        let transport = Linked::new(port, host_end);
        let handle = thread::Builder::new()
            .name("tore-host".into())
            .stack_size(STACK)
            .spawn(move || {
                run(
                    start,
                    resources,
                    transport,
                    listings,
                    Channels {
                        commands,
                        reports: report_sender,
                        handback: theirs,
                    },
                    clock,
                )
            })
            .map_err(|error| format!("Cannot start the host: {error}"))?;
        Ok((
            Self {
                commands: command_sender,
                reports,
                handle: Some(handle),
                addresses,
                end: None,
                notes: Vec::new(),
                handback,
            },
            game_end,
        ))
    }

    /// The socket, once the thread has ended without ending the game
    /// ([`End::SteppedDown`], [`End::TakeoverFailed`]): the game joins with
    /// it again. `None` before then, and after it was taken.
    pub fn take_socket(&self) -> Option<ServerSocket> {
        self.handback
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The addresses the socket listens on.
    pub fn addresses(&self) -> &[SocketAddr] {
        &self.addresses
    }

    /// Sends `command`; false when the thread has ended.
    // The lobby (EF4) sends its verbs through this; until then only the
    // tests do.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    /// Every report since the last call, each also written to the game's
    /// log.
    pub fn poll(&mut self) -> Vec<Report> {
        let mut reports = Vec::new();
        loop {
            match self.reports.try_recv() {
                Ok(report) => {
                    log_report(&report);
                    if let Report::Forward(news) = &report {
                        self.notes.extend(news.lines().iter().cloned());
                    }
                    if let Report::Ended(end) = &report {
                        self.end = Some(end.clone());
                    }
                    reports.push(report);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // A thread that ends always says why first; this is a
                    // thread that could not even do that.
                    if self.end.is_none() {
                        let end = End::Panicked("the host thread ended without a word".into());
                        let report = Report::Ended(end.clone());
                        log_report(&report);
                        self.end = Some(end);
                        reports.push(report);
                    }
                    break;
                }
            }
        }
        reports
    }

    /// The lines about the router's port forward since the last call, for
    /// the lobby's Messages.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Why the thread ended, once it has said.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// Asks the thread to stop and waits at most `limit` for it to end. True
    /// when it ended in time (or had ended already); otherwise it is left to
    /// finish on its own.
    pub fn stop(&mut self, limit: Duration) -> bool {
        let Some(handle) = self.handle.take() else {
            return true;
        };
        let _ = self.commands.send(Command::Stop);
        let deadline = Instant::now() + limit;
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        let ended = handle.is_finished();
        if ended {
            let _ = handle.join();
        } else {
            log::warn!(
                "Host: the host thread did not stop within {} ms; leaving it to finish",
                limit.as_millis()
            );
        }
        self.poll();
        ended
    }
}

impl Drop for HostThread {
    fn drop(&mut self) {
        self.stop(JOIN_LIMIT);
    }
}

/// A hosted game's listing, if it has one, and the game port a listing made
/// later (the King's Visibility) registers from.
struct Listings {
    listing: Option<HostListing>,
    port: u16,
    /// The router's port mapping, when the game asked for one.
    forwarder: Option<Forwarder>,
    /// The Mapped candidate the router gave, which a listing made later
    /// starts with.
    mapped: Option<SocketAddr>,
    /// What the mapping came to, for the telemetry Report.
    port_mapping: PortMapping,
}

impl Listings {
    /// Takes in what the mapper thread has to say: the candidate to the
    /// listing, and the lines to the game.
    fn take_news(&mut self, now: Duration, reports: &Sender<Report>) {
        let Some(forwarder) = &self.forwarder else {
            return;
        };
        for news in forwarder.poll() {
            self.absorb(&news, now);
            let _ = reports.send(Report::Forward(news));
        }
    }

    fn absorb(&mut self, news: &News, now: Duration) {
        if let News::Mapped {
            outside,
            port_mapping,
            ..
        } = news
        {
            self.mapped = *outside;
            self.port_mapping = *port_mapping;
            if let Some(listing) = self.listing.as_mut() {
                listing.set_mapped(*outside, now);
            }
        }
    }

    /// Removes the mapping, waiting for it at most
    /// [`forward::REMOVE_WAIT`], and passes on what it says.
    fn finish_forward(&mut self, now: Duration, reports: &Sender<Report>) {
        if let Some(mut forwarder) = self.forwarder.take() {
            forwarder.finish(forward::REMOVE_WAIT);
            for news in forwarder.poll() {
                self.absorb(&news, now);
                let _ = reports.send(Report::Forward(news));
            }
        }
    }
}

/// A listing of the game on `master` from the game port `port`, as the
/// thread starts it ([`HostThread::start_listed`]) or the King's Visibility
/// asks for it.
fn new_listing(
    master: &str,
    build: &tore_session::BuildId,
    install_id: Option<u64>,
    port: u16,
    now: Duration,
) -> Result<HostListing, String> {
    HostListing::new(master, rendezvous_config(build, install_id), port, now)
        .map_err(|error| format!("Cannot list the game: {error}"))
}

/// What a hosting game tells the master about itself.
fn rendezvous_config(build: &tore_session::BuildId, install_id: Option<u64>) -> HostRendezvous {
    HostRendezvous {
        build: Build {
            protocol_version: tore_session::wire::PROTOCOL_VERSION,
            game_version: build.version.clone(),
            game_commit: build.commit.clone(),
            compatibility: build.compatibility,
            release: build.release,
        },
        dedicated: false,
        install_id,
        platform: Platform::current().code(),
        entropy: tore_net::Entropy::System,
    }
}

/// The King's Visibility (slice F2-1): lists the game when the host's
/// setting turns public and takes it off when it turns local or hidden, as
/// [`Command::SetListed`] does; `applied` is whether the setting was
/// public when last seen, so only a change counts.
fn follow_visibility(host: &Host, listings: &mut Listings, applied: &mut bool, now: Duration) {
    let public = host.settings().visibility() == Visibility::Public;
    if public == *applied {
        return;
    }
    *applied = public;
    match listings.listing.as_mut() {
        Some(listing) => listing.set_listed(public, now),
        None if public => match new_listing(
            tore_net::master::DEFAULT_MASTER,
            &host.config().build,
            None,
            listings.port,
            now,
        ) {
            Ok(mut listing) => {
                listing.set_mapped(listings.mapped, now);
                listing.set_listed(true, now);
                listings.listing = Some(listing);
            }
            Err(error) => log::warn!("Host: the King made the game public, but {error}"),
        },
        None => {}
    }
}

/// The thread's channels to the game.
struct Channels {
    commands: Receiver<Command>,
    reports: Sender<Report>,
    handback: Arc<Mutex<Option<ServerSocket>>>,
}

/// The host thread's transport: the game port and the host's end of the
/// link.
type HostTransport = Linked<GamePort>;

/// The thread: serve until stopped, catching a panic.
fn run(
    start: Start,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    mut transport: HostTransport,
    mut listings: Listings,
    channels: Channels,
    clock: RealClock,
) {
    let mut host: Option<Host> = None;
    let served = panic::catch_unwind(AssertUnwindSafe(|| {
        serve(
            &mut host,
            start,
            resources,
            &mut transport,
            &mut listings,
            &channels,
            clock,
        )
    }));
    let end = match served {
        Ok(end) => end,
        Err(payload) => {
            if let Some(host) = host.as_mut() {
                // Best effort: the host may be broken, so a second panic is
                // caught and ignored.
                let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                    host.stop();
                    let _ = host.transmit(&mut transport);
                }));
            }
            End::Panicked(panic_text(payload.as_ref()))
        }
    };
    // The listing goes before the socket closes, a panic included (a
    // handover or a step-down has released it already).
    if let Some(listing) = listings.listing.as_mut() {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            listing.stop(Duration::ZERO);
            let _ = listing.transmit(&mut transport);
        }));
    }
    // The router's mapping goes too (it was asked to when the stop began,
    // so this waits only for the last of it).
    listings.finish_forward(Duration::ZERO, &channels.reports);
    drop(host);
    // The port is free, or the game's again, before the game hears the end.
    if matches!(
        end,
        End::SteppedDown(_) | End::TakeoverFailed(_) | End::MovedOn(_)
    ) {
        let Linked { socket: port, .. } = transport;
        *channels
            .handback
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(port.socket);
    } else {
        drop(transport);
    }
    let _ = channels.reports.send(Report::Ended(end));
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "an unknown error".into())
}

/// The host's receive, update and transmit, through the listing when the
/// game has one.
fn turn(
    host: &mut Host,
    transport: &mut HostTransport,
    listing: &mut Option<HostListing>,
    now: Duration,
    reports: &Sender<Report>,
) {
    // A build's loading keepalives reach relayed players through the
    // channels open now (protocol 28).
    if let Some(listing) = listing.as_ref() {
        host.set_relay_channels(listing.relay_channels());
    }
    let received = match listing.as_mut() {
        Some(listing) => host.receive_from(now, &mut listing.over(transport, now)),
        None => host.receive_from(now, transport),
    };
    if let Err(error) = received {
        let _ = reports.send(Report::Note(format!("receive failed: {error}")));
    }
    host.update(now);
    if let Some(listing) = listing.as_mut() {
        let host = &*host;
        listing.update(now, || host.discover_answer(0).into());
    }
    send(host, transport, listing, now, reports);
}

/// Sends what the host and the listing have queued.
fn send(
    host: &mut Host,
    transport: &mut HostTransport,
    listing: &mut Option<HostListing>,
    now: Duration,
    reports: &Sender<Report>,
) {
    let sent = match listing.as_mut() {
        Some(listing) => host.transmit(&mut listing.over(transport, now)),
        None => host.transmit(transport),
    };
    if let Err(error) = sent {
        let _ = reports.send(Report::Note(format!("send failed: {error}")));
    }
    if let Some(listing) = listing.as_mut()
        && let Err(error) = listing.transmit(transport)
    {
        let _ = reports.send(Report::Note(format!("send failed: {error}")));
    }
}

/// The listing's news: its events to the game's log, and its state to the
/// game when it changed.
fn forward_listing(
    listing: &mut Option<HostListing>,
    reports: &Sender<Report>,
    state: &mut Option<ListingState>,
) {
    let Some(listing) = listing.as_mut() else {
        return;
    };
    let master = listing.master_text();
    while let Some(event) = listing.poll_event() {
        match event {
            RendezvousEvent::MappingTested(mapping) => {
                log::info!("Host: the Internet Lobby's router test: {mapping:?}");
            }
            RendezvousEvent::LookupFailed(why) => {
                log::warn!("Host: cannot find the Internet Lobby at {master}: {why}");
            }
            RendezvousEvent::MasterAddress { from, to, why } => log::info!(
                "Host: the Internet Lobby at {master}: now trying {to} instead of {from} ({})",
                why.text()
            ),
            RendezvousEvent::SendFailed { to, error } => {
                log::warn!("Host: cannot send to the Internet Lobby at {to}: {error}");
            }
            other => log::info!("Host: Internet Lobby: {other:?}"),
        }
    }
    let now = listing.state();
    if state.as_ref() != Some(&now) {
        *state = Some(now.clone());
        let _ = reports.send(Report::Listing(now));
    }
}

/// The hosting game's Report to the master as hosting stops, when the game
/// is listed and telemetry is on (the rendezvous sends nothing without an
/// install id).
fn report_session(
    host: &Host,
    listing: &mut Option<HostListing>,
    tally: &HostTally,
    port_mapping: PortMapping,
    now: Duration,
) {
    let Some(listing) = listing.as_mut() else {
        return;
    };
    if !listing.rendezvous().listed_wanted() || !tally.anyone() {
        return;
    }
    let mapping = listing.rendezvous().mapping();
    let mut report = tally.report(
        now,
        Role::HostingGame,
        &host.config().build.version,
        Platform::current().code(),
        mapping,
    );
    // What the router did about the port, when the game asked it to.
    report.port_mapping = port_mapping;
    listing.rendezvous_mut().report(report);
}

/// Builds the host, or takes the game over, and runs the dedicated server's
/// loop until the host stops, the game stops it, it hands the game over or
/// it steps down.
fn serve(
    slot: &mut Option<Host>,
    start: Start,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    transport: &mut HostTransport,
    listings: &mut Listings,
    channels: &Channels,
    mut clock: RealClock,
) -> End {
    let (commands, reports) = (&channels.commands, &channels.reports);
    // Held until the thread stops hosting, a panic included.
    let activity = Activity::begin("Hosting a T.O.R.E-Fighters game");
    let took_over = matches!(start, Start::TakeOver { .. });
    let host = match start {
        Start::New { spec, config } => match Host::new(*spec, resources, config) {
            Ok(host) => host,
            Err(error) => return End::BuildFailed(error.to_string()),
        },
        Start::TakeOver {
            standby,
            config,
            resumption,
        } => match take_over_thread(standby, resources, config, resumption) {
            Ok(host) => host,
            Err(error) => return End::TakeoverFailed(error),
        },
    };
    let planes = host.world().roster.planes().len() as u32;
    if let OpenPlanes::List(list) = &host.config().open_planes
        && let Some(plane) = list.iter().find(|plane| **plane >= planes)
    {
        return End::BuildFailed(format!(
            "--open-planes names plane {plane}, but the mission has planes 0 to {}",
            planes.saturating_sub(1)
        ));
    }
    let host = slot.insert(host);
    // Stage K: a game a player hosts keeps standbys, so it can be taken
    // over when it is lost or leaves.
    host.set_standbys_enabled(true);
    // Protocol 28 (slice NET2): while the host builds a mission its loading
    // thread keeps every player connected, this game's own over the link.
    if let Ok(base) = transport.socket.socket.try_clone() {
        let link = transport.link.sender();
        host.set_loading_socket(Box::new(move || {
            Ok(Box::new(Linked::new(base.try_clone()?, link.sender()))
                as Box<dyn Datagrams + Send>)
        }));
    }
    if took_over {
        let tick = host.world().tick();
        resume_listing(host, listings, clock.now());
        let _ = reports.send(Report::TookOver { tick });
    }
    // After the build: a real-time thread that runs long without blocking is
    // demoted for a while.
    let tick = Duration::from_nanos(1_000_000_000 / TICKS_PER_SECOND);
    if let Some(line) = summary(&real_time_thread(tick), activity.outcome()) {
        log::info!("Host: {line}");
    }
    let capacity = host.status(clock.now()).capacity;
    let _ = reports.send(Report::Started {
        aircraft: planes as usize,
        capacity,
    });
    let mut phase = None;
    let mut listing_state = None;
    let mut tally = HostTally::new(clock.now());
    let mut stop_by: Option<Duration> = None;
    let mut handing = false;
    let measure = std::env::var_os("TORE_PERF_HOST").is_some();
    let mut report_at = clock.now() + Duration::from_secs(1);
    let mut public = host.settings().visibility() == Visibility::Public;
    let mut migration = Migration::default();
    loop {
        let now = clock.now();
        listings.take_news(now, reports);
        follow_visibility(host, listings, &mut public, now);
        migration.listing_part(host, listings);
        let port = &mut transport.socket;
        port.now = now;
        if let (Some(peers), Some(side)) = (port.peers.as_mut(), port.side.as_ref()) {
            peers.set_hosting(true, side.old_host());
        }
        let listing = &mut listings.listing;
        turn(host, transport, listing, now, reports);
        if forward(host, reports, &mut phase) {
            tally.present(host.players().into_iter().map(|p| p.address));
        }
        forward_listing(listing, reports, &mut listing_state);
        migration.log(host, now, reports);
        if measure && now >= report_at {
            let status = host.status(now);
            println!(
                "Host performance: tick={} players={} mean_ms={:.3} max_ms={:.3} overloads={}",
                status.tick,
                status.players,
                status.tick_cost_mean.as_secs_f64() * 1000.,
                status.tick_cost_max.as_secs_f64() * 1000.,
                status.overloads,
            );
            report_at = now + Duration::from_secs(1);
        }
        // Stage K: another game hosts the session now.
        if let Some(to) = transport.socket.answered {
            host.step_down();
            forward(host, reports, &mut phase);
            migration.log(host, now, reports);
            release_listing(listings);
            return End::SteppedDown(to);
        }
        if !handing && stop_by.is_none() {
            ask_standby(host, &mut transport.socket, now);
        }
        // A handover's records are through: the house leaving (a stop), or
        // the host moving the lobby to a better machine by itself.
        if host.handed_over() {
            forward(host, reports, &mut phase);
            migration.log(host, now, reports);
            release_listing(listings);
            return if handing {
                End::HandedOver
            } else {
                End::MovedOn(migration.standby_one.clone())
            };
        }
        if host.phase() == Phase::Stopped {
            let listing = &mut listings.listing;
            report_session(host, listing, &tally, listings.port_mapping, now);
            if let Some(listing) = listing.as_mut() {
                listing.stop(now);
            }
            send(host, transport, listing, now, reports);
            forward_listing(listing, reports, &mut listing_state);
            return if stop_by.is_some() {
                End::Stopped
            } else {
                End::Finished
            };
        }
        let mut stop = false;
        loop {
            match commands.try_recv() {
                Ok(Command::Stop) => stop = true,
                Ok(Command::SetAiTest(settings)) => host.set_ai_test(settings),
                Ok(Command::SetListed(listed)) => match listings.listing.as_mut() {
                    Some(listing) => listing.set_listed(listed, now),
                    None => log::info!(
                        "Host: this game was not started for the Internet Lobby; it stays unlisted"
                    ),
                },
                #[cfg(test)]
                Ok(Command::Panic) => panic!("a test asked the host thread to panic"),
                #[cfg(test)]
                Ok(Command::Vanish) => {
                    // The machine is gone: not a word to anyone, and the
                    // listing and the socket with it.
                    listings.listing = None;
                    return End::Stopped;
                }
                #[cfg(test)]
                Ok(Command::Cut(time)) => transport.socket.cut_until = Some(now + time),
                Err(TryRecvError::Empty) => break,
                // The game has gone without a word: stop as it would.
                Err(TryRecvError::Disconnected) => {
                    stop = true;
                    break;
                }
            }
        }
        if stop && stop_by.is_none() {
            // The mapping comes off while the players are told.
            if let Some(forwarder) = listings.forwarder.as_mut() {
                forwarder.begin_stop();
            }
            // Stage K: with a ready standby the game goes on there.
            match host.hand_over() {
                Ok(to) => {
                    handing = true;
                    log::info!("Host: handing the game over to player {to}");
                }
                Err(why) => {
                    log::info!("Host: the game ends with this one ({why})");
                    host.host_left();
                }
            }
            stop_by = Some(now + STOP_GRACE);
            let _ = host.transmit(transport);
            forward(host, reports, &mut phase);
        }
        if stop_by.is_some_and(|by| now >= by) {
            if handing {
                // The standby has had its time: the game is its own now.
                release_listing(listings);
                return End::HandedOver;
            }
            host.stop();
            let listing = &mut listings.listing;
            report_session(host, listing, &tally, listings.port_mapping, now);
            if let Some(listing) = listing.as_mut() {
                listing.stop(now);
            }
            send(host, transport, listing, now, reports);
            forward(host, reports, &mut phase);
            forward_listing(listing, reports, &mut listing_state);
            return End::Stopped;
        }
        // A tick deadline is spun to; a housekeeping nap is slept to.
        let wake = plan_wake(now, host.next_wake(now), host.wake_pace(), SPIN_MARGIN);
        wait_until(&mut clock, wake.at, wake.spin_margin);
    }
}

/// A host that took the game over carries on the old host's listing from
/// the listing part it restored (slice K8's `HostListing::resume`).
fn resume_listing(host: &Host, listings: &mut Listings, now: Duration) {
    let Some(bytes) = host.listing_part() else {
        return;
    };
    let part = match ListingPart::decode(bytes) {
        Ok(part) => part,
        Err(error) => {
            log::warn!("Host: the listing part cannot be read ({error:?}); the game is not listed");
            return;
        }
    };
    let config = rendezvous_config(&host.config().build, None);
    match HostListing::resume(&part, config, listings.port, now) {
        Ok(mut listing) => {
            listing.set_mapped(listings.mapped, now);
            log::info!(
                "Host: carrying on the game's listing on {}",
                listing.master_text()
            );
            listings.listing = Some(listing);
        }
        Err(error) => log::warn!("Host: the listing cannot be carried on: {error}"),
    }
}

/// Lets the listing go without a word (no Unregister): the game's new host
/// carries it on.
fn release_listing(listings: &mut Listings) {
    if let Some(mut listing) = listings.listing.take() {
        let _ = listing.release();
    }
}

/// An old host that lost every player at once, or heard it was taken over,
/// asks standby 1 whether it hosts now, every [`ASK_EVERY`], at the
/// addresses standby 1 had when the host first asked (its own standbys time
/// out later). The answer is [`GamePort::answered`].
fn ask_standby(host: &Host, port: &mut GamePort, now: Duration) {
    if port.asking.is_none() && (host.lost_everyone() || host.moved_to().is_some()) {
        let to = host.standby_addresses();
        if to.is_empty() {
            return;
        }
        log::info!(
            "Host: every player is lost or another game took over; asking standby 1 at {} whether it hosts now",
            to.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        port.asking = Some(Asking {
            session: host.session(),
            nonce: nonce(),
            to,
            last: None,
        });
    }
    let Some(asking) = port.asking.as_mut() else {
        return;
    };
    if asking
        .last
        .is_some_and(|last| now.saturating_sub(last) < ASK_EVERY)
    {
        return;
    }
    asking.last = Some(now);
    let reach = reach_packet(asking.session, asking.nonce, 0);
    for to in asking.to.clone() {
        let _ = port.send_datagram(to, &reach);
    }
}

/// A nonce for the old host's question, from the standard library's
/// randomly keyed hasher.
fn nonce() -> u64 {
    use std::hash::BuildHasher;
    std::collections::hash_map::RandomState::new().hash_one(Instant::now())
}

/// What the thread tracks for host migration: the listing part last handed
/// to the host, and the standbys last logged.
#[derive(Default)]
struct Migration {
    part_version: Option<Option<u64>>,
    figures: Vec<(u8, tore_session::wire::messages::StandbyMark, bool, bool)>,
    figures_at: Option<Duration>,
    /// Whether a ready standby was last reported ([`Report::Standby`]).
    ready_sent: Option<bool>,
    /// Standby 1's addresses as last known, for a move in the lobby.
    standby_one: Vec<SocketAddr>,
}

impl Migration {
    /// Hands the host the listing's part whenever it may have changed.
    fn listing_part(&mut self, host: &mut Host, listings: &Listings) {
        let version = listings.listing.as_ref().map(HostListing::part_version);
        if self.part_version == Some(version) {
            return;
        }
        self.part_version = Some(version);
        host.set_listing_part(
            listings
                .listing
                .as_ref()
                .and_then(HostListing::part)
                .map(|part| part.encode()),
        );
    }

    /// The host's migration notes, and its standbys when they change, to the
    /// game's log.
    fn log(&mut self, host: &mut Host, now: Duration, reports: &Sender<Report>) {
        for note in host.take_resume_notes() {
            let _ = reports.send(Report::Migration(crate::net::standby::resume_line(&note)));
            if let Some(line) = crate::net::standby::said_line(&note) {
                let _ = reports.send(Report::Said(line));
            }
        }
        if self
            .figures_at
            .is_some_and(|at| now.saturating_sub(at) < FIGURES_EVERY)
        {
            return;
        }
        self.figures_at = Some(now);
        let ready = host.standby_figures().iter().any(|f| f.ready());
        if self.ready_sent != Some(ready) {
            self.ready_sent = Some(ready);
            let _ = reports.send(Report::Standby { ready });
        }
        let addresses = host.standby_addresses();
        if !addresses.is_empty() {
            self.standby_one = addresses;
        }
        let figures = host.standby_figures();
        let shape: Vec<_> = figures
            .iter()
            .map(|f| (f.player, f.role, f.warm, f.ready()))
            .collect();
        if shape != self.figures {
            self.figures = shape;
            let _ = reports.send(Report::Migration(crate::net::standby::figures_line(
                &figures,
            )));
        }
    }
}

/// Sends the host's new log entries and its phase when it changed; true when
/// a player joined or left.
fn forward(host: &mut Host, reports: &Sender<Report>, phase: &mut Option<Phase>) -> bool {
    let mut players_changed = false;
    while let Some(entry) = host.poll_log() {
        players_changed |= matches!(entry, HostLog::Connected { .. } | HostLog::Left { .. });
        let _ = reports.send(Report::Log(entry));
    }
    while let Some(entry) = host.poll_content_log() {
        let _ = reports.send(Report::Content(entry));
    }
    let now = host.phase();
    // The seconds to the next mission count down; only the kind of phase is
    // news.
    let kind = |phase: Phase| std::mem::discriminant(&phase);
    if phase.is_none_or(|before| kind(before) != kind(now)) {
        *phase = Some(now);
        let _ = reports.send(Report::Phase(now));
    }
    players_changed
}

/// The settings of a game a player hosts (EF4). *Agent decisions:* the
/// server guide's defaults, except that the hosting player's own connection
/// is the house, which wears the crown and starts each mission
/// (`StartMode::King`); a mission's
/// end returns everyone to the lobby at once (`after-end restart` with no
/// delay); and a mission nobody flies any more ends at once (no empty
/// timeout), so the lobby returns when the last player leaves the flight.
pub fn config(options: &HostOptions) -> HostConfig {
    let mut config = hosted();
    config.name = options.name.clone();
    config.password = options.password.clone();
    config.open_planes = options.open_planes.clone();
    if options
        .listing
        .as_ref()
        .is_some_and(|listing| listing.listed)
    {
        config
            .settings
            .push((number::VISIBILITY, Visibility::Public.value()));
    }
    config
}

/// What every game a player hosts has, as [`config`] says.
fn hosted() -> HostConfig {
    let mut config = HostConfig::new(build_id());
    config.house = Some(LINK_ADDRESS);
    config.crown = CrownRule::FirstPlayer;
    // The King's Visibility can list the game: the thread lists it while
    // it is public (slice F2-1), and a game hosted to be listed starts so.
    config.listable = true;
    config.start = StartMode::King;
    config.after_end = AfterEnd::Restart;
    config.restart_delay = Duration::ZERO;
    config.empty_timeout = Duration::ZERO;
    config.retail_stall_speeds = tore_sim::flight::retail_stall_speeds();
    config
}

/// The settings of a game that takes another's game over (stage K, slice
/// K7a): a game a player hosts, as [`config`] makes it, with this game's
/// content as the reference. *Agent decision:* the name, the password and
/// every King's setting come from the session part (`Host::resume`); what
/// only the configuration holds, the open planes, takes the defaults.
pub fn takeover_config(resources: &Arc<BTreeMap<String, Vec<u8>>>) -> HostConfig {
    let mut config = hosted();
    config.content = Some(crate::net::session::game_content(resources));
    config
}

/// Writes a report to the game's log.
fn log_report(report: &Report) {
    match report {
        Report::Started { aircraft, capacity } => log::info!(
            "Host: the mission is built ({aircraft} aircraft, {capacity} players at most); taking joins"
        ),
        Report::Log(entry) => log::info!("Host: {}", log_line(entry)),
        Report::Content(entry) => log::info!("Host: tick {}: {}", entry.tick, entry.text),
        Report::Phase(phase) => log::info!("Host: phase {phase:?}"),
        Report::Note(text) => log::warn!("Host: {text}"),
        Report::Listing(state) => log::info!("Host: {}", state_text(state)),
        Report::Forward(news) => {
            for line in news.lines() {
                log::info!("Host: {line}");
            }
        }
        Report::TookOver { tick } => {
            log::info!("Host: this game took the game over at tick {tick}");
        }
        Report::Migration(line) => log::info!("Host: {line}"),
        Report::Said(line) => log::info!("Host: said to the player: {line}"),
        Report::Standby { ready } => log::info!("Host: a ready standby: {ready}"),
        Report::Ended(end) => match end {
            End::Stopped => log::info!("Host: stopped"),
            End::HandedOver => log::info!("Host: handed the game over; stopped"),
            End::SteppedDown(to) => {
                log::info!("Host: another game hosts now, at {to}; stepped down")
            }
            End::TakeoverFailed(text) => log::warn!("Host: could not take the game over: {text}"),
            End::MovedOn(_) => log::info!("Host: moved the game to a better machine; stopped"),
            End::Finished => log::info!("Host: the mission ended; the host has stopped"),
            End::BuildFailed(text) => log::warn!("Host: could not start: {text}"),
            End::Panicked(text) => log::error!("Host: the host thread panicked: {text}"),
        },
    }
}

/// A host log entry as a line of the game's log, in the dedicated server's
/// words; the hosting player's own connection is "this game".
pub fn log_line(entry: &HostLog) -> String {
    let who = |address: &SocketAddr| {
        if *address == LINK_ADDRESS {
            "this game".to_owned()
        } else {
            address.to_string()
        }
    };
    let text = match entry {
        HostLog::Connected {
            address, callsign, ..
        } => format!("{} joined as {callsign}", who(address)),
        HostLog::Refused {
            address,
            callsign,
            reason,
            ..
        } if callsign.is_empty() => format!("{} refused: {reason}", who(address)),
        HostLog::Refused {
            address,
            callsign,
            reason,
            ..
        } => format!("{} ({callsign}) refused: {reason}", who(address)),
        HostLog::ContentRefused {
            callsign, names, ..
        } => format!("{callsign} refused: content mismatch: {}", names.join(", ")),
        HostLog::SeatRefused {
            callsign, reason, ..
        } => format!("{callsign} was refused a plane: {reason}"),
        HostLog::Seated {
            seat,
            callsign,
            plane,
            ..
        } => format!("seat {seat} {callsign} took plane {plane}"),
        HostLog::Left {
            seat,
            callsign,
            plane,
            reason,
            ..
        } => {
            let seat = seat.map_or_else(String::new, |seat| format!("seat {seat} "));
            let plane = plane.map_or_else(String::new, |plane| format!(" (plane {plane})"));
            let why = match reason {
                LeaveReason::Left => "left".to_owned(),
                LeaveReason::Silent => "no packet for 5 seconds".to_owned(),
                LeaveReason::Kicked => "kicked".to_owned(),
                LeaveReason::MissionEnded => "the mission ended".to_owned(),
                LeaveReason::HostLeft => "the host left the game".to_owned(),
                LeaveReason::Replaced => {
                    "replaced by a new connection from the same address".to_owned()
                }
                LeaveReason::Disconnected(why) => format!("disconnected ({why:?})"),
            };
            format!("{seat}{callsign}{plane} left: {why}")
        }
        HostLog::MissionStarted { .. } => "mission started".to_owned(),
        HostLog::MissionEnded { reason, .. } => format!(
            "mission ended: {}",
            match reason {
                EndReason::EveryoneLeft => "everyone left",
                EndReason::TimeLimit => "the time limit",
                EndReason::ServerStopping => "the host is stopping",
                EndReason::EndedByServer => "ended by the host",
                EndReason::HostLeft => "the host left the game",
                EndReason::KillLimit => "the kill limit",
            }
        ),
        HostLog::MissionRestarted { .. } => "mission restarted".to_owned(),
        HostLog::Overloaded { ticks_behind, .. } => {
            format!("overloaded: the tick loop is {ticks_behind} ticks behind real time")
        }
        HostLog::Fault { text, .. } => format!("fault: {text}"),
        HostLog::Note { text, .. } => text.clone(),
        HostLog::Feedback(f) => {
            format!("Feedback from {} at {}: {}", f.player, f.tick / 120, f.text)
        }
        HostLog::Stopped { .. } => "stopped".to_owned(),
        HostLog::Lobby {
            callsign, event, ..
        } => format!("lobby: {callsign} {event}"),
        HostLog::Chat {
            callsign,
            receiver,
            text,
            heard,
            ..
        } => format!(
            "chat: {callsign} to {} ({heard} heard): {text}",
            tore_session::wire::chat::receiver_label(*receiver).to_ascii_lowercase()
        ),
        HostLog::Stalled { .. } | HostLog::Resumed { .. } => entry.stall_text().unwrap_or_default(),
        HostLog::Loaded { .. } => entry.loading_text().unwrap_or_default(),
    };
    format!("tick {}: {text}", entry.tick())
}

impl crate::App {
    /// Starts hosting the game the command line described: the host thread,
    /// then this game's own join over the in-process link.
    pub(crate) fn start_hosting(&mut self, options: HostOptions) {
        if let Err(error) = self.begin_hosting(options, false) {
            self.message(error);
        }
    }

    /// Starts hosting `options`, naming why it could not (a port in use, a
    /// mission that does not build) in plain words. The Direct Connection
    /// screen's New calls this (with `lobby`, so its lobby screen drives the
    /// game, [`Join::lobby`]) and shows the reason in its Messages.
    pub(crate) fn begin_hosting(
        &mut self,
        options: HostOptions,
        lobby: bool,
    ) -> Result<(), String> {
        if !crate::net::session::content_ready(&self.theater_resources) {
            self.pending_host = Some((options, lobby));
            return Ok(());
        }
        let data = match crate::assets::data_directory() {
            Ok(data) => data,
            Err(error) => {
                self.error = Some(error);
                return Ok(());
            }
        };
        crate::net::settings::remember_host(&data, &options);
        let resources = Arc::clone(&self.theater_resources);
        // Stage L: the game's own content is the reference every player's
        // is compared with, worked out once for the game and the host.
        let mut host_config = config(&options);
        host_config.content = Some(crate::net::session::game_content(&resources));
        // Port mapping is on while hosting unless Options turned it off.
        let forward = forward::choose(crate::net::settings::Remembered::load(&data).port_forward);
        let (thread, link) = HostThread::start_forwarded(
            HostSetup {
                spec: options.spec.clone(),
                resources: Arc::clone(&resources),
                config: host_config,
                listen: Listen::Any,
                port: options.port,
            },
            options.listing.clone(),
            forward,
        )?;
        let listening: Vec<String> = thread.addresses().iter().map(ToString::to_string).collect();
        log::info!(
            "Host: hosting {} from {} on UDP {}",
            options.name,
            options.mission.display(),
            listening.join(" and ")
        );
        let join = Join {
            server: LINK_ADDRESS,
            transport: Transport::Link(link),
            callsign: options.callsign.clone(),
            slot: options.slot,
            password: options.password.clone().unwrap_or_default(),
            label: "hosted".into(),
            lobby,
            token: None,
        };
        // Dropping the thread, on an error, stops it.
        let mut session = NetSession::start(join, resources, &data, self.replay_library.as_ref())?;
        session.hosting = Some(thread);
        self.net = Some(session);
        let listed = if options.listing.as_ref().is_some_and(|l| l.listed) {
            " Listing it on the Internet Lobby."
        } else {
            ""
        };
        self.message(format!(
            "Hosting {} on UDP port {}...{listed}",
            options.name, options.port
        ));
        Ok(())
    }
}

#[cfg(test)]
#[path = "hosting_tests.rs"]
mod tests;
