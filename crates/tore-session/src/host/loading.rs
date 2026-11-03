//! A host's mission builds with its players kept (protocol 28, slice NET2).
//!
//! A host builds a mission's world on its own loop: when the King changes
//! the mission or a setting that rebuilds it, when a flight with the
//! players' loadouts starts, and for the next mission after an end. The
//! loop sends nothing meanwhile. Before protocol 28 a build over 5 seconds
//! dropped every player for silence, and one over 1.5 seconds set the
//! standbys racing to take the game over.
//!
//! Now the build stays on the loop (John, 2026-10-13: keepalives during the
//! build, not a worker thread) and a [`tore_net::Loading`] thread speaks for
//! the host while it runs: each player's loading Keepalive, a quarter second
//! into the build and then twice a second, for up to 3 minutes. A player
//! hearing it keeps the connection and counts the host lost only after
//! [`crate::standby::takeover::LOADING_SILENCE`] instead of 1.5 seconds.
//!
//! The program that runs the host hands it a way to clone its socket
//! ([`Host::set_loading_socket`]): the dedicated server, a game that hosts
//! (its socket and the in-process link's send-only copy) and `tore-bot`.
//! A relayed player is reached through the relay channels the program last
//! gave ([`Host::set_relay_channels`]). A host given none (the tests' hosts)
//! builds as before.

use super::{Host, HostError, HostLog, build_world};
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tore_net::master::RelayFramings;
use tore_net::{Datagrams, Loading, LoadingConfig};
use tore_world::mission::MissionSpec;
use tore_world::resources::Manifest;

/// Makes the socket a host's loading thread sends on: a clone of the host's
/// own, made at the start of each build.
pub type LoadingSocket = Box<dyn FnMut() -> io::Result<Box<dyn Datagrams + Send>> + Send>;

/// What the host keeps for its loading thread.
#[derive(Default)]
pub(super) struct Loader {
    socket: Option<LoadingSocket>,
    /// `(channel, key, master)` of each relay channel open at the
    /// program's last word.
    relay_channels: Vec<(u32, u32, SocketAddr)>,
    /// A test aid: each build after the first lasts at least this long.
    slow_build: Option<Duration>,
    config: LoadingConfig,
}

/// A boxed socket as [`Datagrams`], for the loading thread.
struct Boxed(Box<dyn Datagrams + Send>);

impl Datagrams for Boxed {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        self.0.send_datagram(to, datagram)
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        self.0.recv_datagram(buf)
    }
}

impl Host {
    /// How this host's loading thread clones the host's socket (protocol
    /// 28): set once by the program that runs it. A host without one sends
    /// nothing while it builds.
    pub fn set_loading_socket(&mut self, socket: LoadingSocket) {
        self.loader.socket = Some(socket);
    }

    /// The relay channels open now, as the program's listing gives them
    /// (`HostListing::relay_channels`): a build's loading keepalives reach
    /// a relayed player through them.
    pub fn set_relay_channels(&mut self, channels: Vec<(u32, u32, SocketAddr)>) {
        if self.loader.relay_channels != channels {
            self.loader.relay_channels = channels;
        }
    }

    /// A test aid (`tore-server --slow-build`, `tore-bot --host-slow-build`):
    /// every mission build after the first lasts at least `delay`, as a
    /// slow machine's would.
    pub fn set_slow_build(&mut self, delay: Option<Duration>) {
        self.loader.slow_build = delay;
    }

    /// The loading thread's timing (tests).
    #[cfg(test)]
    pub(crate) fn set_loading_config(&mut self, config: LoadingConfig) {
        self.loader.config = config;
    }

    /// Select a viable layout at a mission boundary, before publishing its seed.
    pub(super) fn build_seeded(
        &mut self,
        spec: &mut MissionSpec,
    ) -> Result<(super::journal::Driver, Manifest), HostError> {
        // The fixed distance table, as the creator applies it: a King's,
        // a rotation's or a mission file's distance the theater, target and
        // start do not offer moves to the nearest offered one, logged.
        if let Some(moved) = tore_world::mission_layout::distance_table::adjust(spec) {
            self.log_distance(spec, moved);
        }
        let initial = spec.surface_seed;
        let (built, attempts) = tore_world::mission_layout::seed_retry::build(spec, |candidate| {
            self.build(candidate).map_err(|error| match error {
                HostError::Mission(text) => text,
                other => other.to_string(),
            })
        })
        .map_err(HostError::Mission)?;
        self.log_surface_seed(spec, initial, attempts);
        Ok(built)
    }

    /// Logs a distance the table moved in `spec`: `moved` is the old and
    /// new nm.
    pub(super) fn log_distance(&mut self, spec: &MissionSpec, moved: (u32, u32)) {
        let text = tore_world::mission_layout::distance_table::adjust_notice(spec, moved);
        self.log(HostLog::Note {
            tick: self.world.tick(),
            text,
        });
    }

    pub(super) fn log_surface_seed(&mut self, spec: &MissionSpec, initial: u32, attempts: usize) {
        if spec.lays_out_surface() {
            self.log(HostLog::Note {
                tick: self.world.tick(),
                text: tore_world::mission_layout::seed_retry::notice(
                    initial,
                    spec.surface_seed,
                    attempts,
                ),
            });
        }
    }

    /// Builds `spec` on this loop with the loading thread speaking for the
    /// host meanwhile, and logs how long it took when the thread spoke.
    pub(super) fn build(
        &mut self,
        spec: &MissionSpec,
    ) -> Result<(super::journal::Driver, Manifest), HostError> {
        let started = Instant::now();
        let targets = self.server.loading_datagrams();
        let players = targets.len();
        let loading = match (&mut self.loader.socket, players) {
            (Some(make), 1..) => match make() {
                Ok(socket) => {
                    let socket =
                        RelayFramings::new(Boxed(socket), self.loader.relay_channels.clone());
                    Loading::start(socket, targets, self.loader.config).ok()
                }
                Err(_) => None,
            },
            _ => None,
        };
        let built = build_world(spec, &self.resources, self.config.contrail_puffs);
        if let Some(delay) = self.loader.slow_build {
            std::thread::sleep(delay.saturating_sub(started.elapsed()));
        }
        let rounds = loading.as_ref().map_or(0, Loading::sent);
        drop(loading);
        if rounds > 0 {
            let tick = self.world.tick();
            self.log(HostLog::Loaded {
                tick,
                players,
                took: started.elapsed(),
                rounds,
            });
        }
        built
    }
}
