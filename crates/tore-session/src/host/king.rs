//! The King's lobby (stage F phase 2, slice F2-1; docs/ARCHITECTURE.md, "The
//! King, the crown and the house" and "Slots, sides and joining"): the crown
//! and the house, the King's settings and their phase rules, slot locks, and
//! the King's rules on taking a plane (join in progress, lock sides, the
//! locks).
//!
//! The house is the connection whose game runs the host
//! ([`HostConfig::house`](super::HostConfig::house)); its leaving ends the
//! game. The King is a lobby role: the house when it joins (a game a player
//! hosts), or the first player to join a dedicated server with `king
//! first-player`, and when the King leaves, the longest-connected player.
//!
//! Every rule below that the design leaves open is an agent decision, named
//! as such in docs/ARCHITECTURE.md's "Built (F2-1)" notes.

use super::config::{PUBLIC_IS_BROADCAST, RATE_IS_THE_FILES};
use super::{ConnectionId, CrownRule, Host, HostError, Life, LobbyEvent, Stage, StartMode};
use crate::settings::{self, Change, LoadoutRule, Mode, Store, number};
use crate::wire::messages::{Lock, Message, PasswordChange, SettingsChange};
use std::collections::BTreeMap;
use std::time::Duration;
use tore_sim::ai::launch::Side;
use tore_world::mission::MissionSpec;
use tore_world::seats::PlaneId;

/// The refusal of a King's change made outside the lobby.
pub const IN_THE_LOBBY: &str = "Change it in the lobby, between missions.";
/// The refusal of any seating after the mission's first tick with join in
/// progress off.
pub const NO_NEW_PILOTS: &str = "This game takes no new pilots once the mission flies.";
/// The refusal of a plane on the other side with lock sides on.
pub const SIDES_LOCKED: &str = "Sides are locked until the mission ends.";
/// The refusal of a King's change on a server whose file locks its mission.
pub const MISSION_LOCKED: &str = "This server's mission and settings are its operator's.";
/// The longest game name, in bytes (the configuration's own limit).
const NAME_LIMIT: usize = 64;

/// What the King's lobby keeps on the host beside the settings store: the
/// session's, which stage K moves with the host (docs/ARCHITECTURE.md,
/// "State for exact checkpoints").
#[derive(Clone, Debug, Default)]
pub(super) struct Court {
    /// Each locked slot's lock, by plane; a plane not here is open. Cleared
    /// with every new mission.
    locks: BTreeMap<u32, Lock>,
    /// The side of the first plane each player flew this mission, by
    /// callsign, for lock sides (agent decision: the callsign, since a
    /// player who leaves and joins again has a new connection).
    sides: BTreeMap<String, Side>,
    /// A crowned dedicated server's own mission, which it goes back to once
    /// it has been empty for its empty timeout.
    file_spec: Option<MissionSpec>,
    /// When the last player left a crowned dedicated server.
    vacant_since: Option<Duration>,
}

impl Court {
    /// A token restores the same side restriction under its new display name.
    pub(super) fn rejoin_callsign(&mut self, old: &str, new: &str) {
        if let Some(side) = self.side_of(old) {
            self.sides.insert(new.to_owned(), side);
        }
    }

    /// The court of a host built from `config` for its mission `spec`.
    pub(super) fn new(config: &super::HostConfig, spec: &MissionSpec) -> Self {
        Self {
            file_spec: (config.crown == CrownRule::FirstPlayer && config.house.is_none())
                .then(|| spec.clone()),
            ..Self::default()
        }
    }

    /// The lock on `plane`'s slot.
    pub(super) fn lock(&self, plane: u32) -> Lock {
        self.locks.get(&plane).cloned().unwrap_or_default()
    }

    /// The side the first plane `callsign` flew this mission fixed, if it
    /// flew one (Autobalance counts it on that side while it is back in the
    /// lobby).
    pub(super) fn side_of(&self, callsign: &str) -> Option<Side> {
        self.sides.get(callsign).copied()
    }
}

/// `spec` with the settings the mission itself carries (friendly fire and
/// the loadout rule), so every player's copy builds the same rules.
pub(super) fn with_settings(mut spec: MissionSpec, store: &Store) -> MissionSpec {
    spec.friendly_fire = store.friendly_fire();
    spec.cheat_loadouts = store.loadouts() == LoadoutRule::Any;
    spec
}

impl Host {
    /// The King's settings in force: the configuration's to start with,
    /// then the King's changes. A hosting game's thread reads the
    /// visibility here to list the game while it is public.
    pub fn settings(&self) -> &Store {
        &self.settings
    }

    /// The lobby id of the King, if one is connected.
    pub fn king_id(&self) -> Option<u8> {
        self.peers
            .values()
            .find(|peer| peer.king && !matches!(peer.stage, Stage::Closing { .. }))
            .map(|peer| peer.lobby.id)
    }

    /// The start rule in force: the configuration's, except that a dedicated
    /// server with a King connected waits for the King's start.
    pub(super) fn start_rule(&self) -> StartMode {
        if self.config.start != StartMode::King
            && self.config.crown == CrownRule::FirstPlayer
            && self.king_id().is_some()
        {
            StartMode::King
        } else if self.config.start == StartMode::Now
            && (!self.settings.ai() || self.settings.min_players() > 0)
        {
            // Waiting on ready players (AI off, or a minimum): the first
            // ready player's start attempt, and each after it, decides.
            StartMode::FirstPlayer
        } else {
            self.config.start
        }
    }

    // ----- The crown ----------------------------------------------------

    /// Whether a player joining now, the house when `house`, wears the
    /// crown: the house always; otherwise the first player of a game whose
    /// crown goes to the first player and that has no house (a game with a
    /// house waits for it).
    pub(super) fn crowned_at_join(&self, house: bool) -> bool {
        house
            || (self.config.crown == CrownRule::FirstPlayer
                && self.config.house.is_none()
                && self.king_id().is_none())
    }

    /// The King gives the crown to the player with lobby id `player`
    /// (message 23). The caller has checked that `connection` is the King.
    pub(super) fn pass_crown(
        &mut self,
        connection: ConnectionId,
        player: u8,
    ) -> Result<(), String> {
        let to = self
            .peers
            .iter()
            .find(|(_, peer)| {
                peer.lobby.id == player && !matches!(peer.stage, Stage::Closing { .. })
            })
            .map(|(id, _)| *id)
            .ok_or("There is no such player.")?;
        if to == connection {
            return Err("You wear the crown already.".into());
        }
        let mut callsigns = (String::new(), String::new());
        if let Some(peer) = self.peers.get_mut(&connection) {
            peer.king = false;
            callsigns.0 = peer.callsign.clone();
        }
        if let Some(peer) = self.peers.get_mut(&to) {
            peer.king = true;
            callsigns.1 = peer.callsign.clone();
        }
        let (from, to) = callsigns;
        self.lobby_log(from, LobbyEvent::CrownPassed(to));
        self.lobby_dirty = true;
        Ok(())
    }

    /// The King left (not the house, whose leaving ends the game): the
    /// crown passes to the longest-connected player, if any is left.
    pub(super) fn crown_departed(&mut self) {
        if matches!(self.life, Life::Stopped | Life::Ended { next_at: None, .. })
            || self.king_id().is_some()
        {
            return;
        }
        let Some(heir) = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .min_by_key(|(_, peer)| peer.lobby.order)
            .map(|(id, _)| *id)
        else {
            return;
        };
        if let Some(peer) = self.peers.get_mut(&heir) {
            peer.king = true;
            let callsign = peer.callsign.clone();
            self.lobby_log(callsign, LobbyEvent::Crowned);
        }
        self.lobby_dirty = true;
    }

    /// Why the King may not kick lobby id `player`: the house runs the game
    /// (agent decision: its leaving would end the game for everyone, which
    /// only the house decides).
    pub(super) fn kick_refusal(&self, player: u8) -> Option<String> {
        self.peers
            .values()
            .any(|peer| peer.lobby.id == player && peer.house)
            .then(|| "The house runs the game; it cannot be kicked.".to_owned())
    }

    /// Why the King may not change the mission: a server whose file locks
    /// it.
    pub(super) fn mission_refusal(&self) -> Option<String> {
        self.config
            .mission_locked
            .then(|| MISSION_LOCKED.to_owned())
    }

    // ----- Settings -----------------------------------------------------

    /// The King changes the lobby's settings, all or none (message 24). The
    /// caller has checked that `connection` is the King.
    pub(super) fn change_settings(
        &mut self,
        connection: ConnectionId,
        change: &SettingsChange,
    ) -> Result<(), String> {
        if let Some(why) = self.mission_refusal() {
            return Err(why);
        }
        self.check_change(change)?;
        let before = self.settings.clone();
        self.settings.apply(&change.values)?;
        if change
            .values
            .iter()
            .any(|(n, _)| matches!(*n, number::MIN_PLAYERS | number::MAX_PLAYERS | number::MODE))
        {
            let minimum = self.settings.min_players().min(self.capacity()) as u32;
            self.settings.apply(&[(number::MIN_PLAYERS, minimum)])?;
        }
        if let Some(name) = &change.name {
            self.settings.set_name(name);
        }
        match &change.password {
            Some(PasswordChange::Set(password)) => self.settings.set_password(Some(password)),
            Some(PasswordChange::Clear) => self.settings.set_password(None),
            None => {}
        }
        let mut changed: Vec<(u8, u32)> = self
            .settings
            .lobby_list()
            .into_iter()
            .zip(before.lobby_list())
            .filter(|(now, was)| now != was)
            .map(|(now, _)| now)
            .collect();
        let mut words = Vec::new();
        if self.settings.name() != before.name() {
            words.push(format!("name \"{}\"", self.settings.name()));
        }
        if self.settings.password() != before.password() {
            words.push(
                match self.settings.password() {
                    Some(_) => "password set",
                    None => "password cleared",
                }
                .to_owned(),
            );
            // The password's setting is said in those words.
            changed.retain(|(n, _)| *n != number::PASSWORD);
        }
        if change
            .cheats
            .is_some_and(|cheats| cheats != self.spec.cheats)
        {
            words.push("realism changed".into());
        }
        if changed.is_empty() && words.is_empty() {
            return Ok(());
        }
        let said = settings::words(&changed);
        if !said.is_empty() {
            words.insert(0, said);
        }
        if let Some(peer) = self.peers.get(&connection) {
            let callsign = peer.callsign.clone();
            self.lobby_log(callsign, LobbyEvent::SettingsChanged(words.join(", ")));
        }
        let moved = |n: u8| changed.iter().any(|(c, _)| *c == n);
        if moved(number::MODE) {
            self.fit_slots();
        }
        // The upload figures measured what the old rate needs.
        if moved(number::SNAPSHOT_RATE) {
            self.rate_changed();
        }
        // A change that alters what players chose clears every ready mark,
        // as a mission change does. Turning Autobalance on is the exception
        // (slice A1, plan 6.3): only the players its re-deal moves lose
        // theirs.
        let balance_on = self.settings.balanced() && !before.balanced();
        if change
            .cheats
            .is_some_and(|cheats| cheats != self.spec.cheats)
            || moved(number::MODE)
            || moved(number::LOADOUTS)
            || (moved(number::LOCK_SIDES) && !balance_on)
        {
            self.clear_ready();
        }
        // Friendly fire and the loadout rule travel in the mission, so a
        // change of either goes to every player's copy.
        let mut spec = with_settings(self.spec.clone(), &self.settings);
        if let Some(cheats) = change.cheats {
            spec.cheats = cheats;
        }
        if spec != self.spec
            && let Err(why) = self.respec(spec)
        {
            // The same mission with two settings changed builds as before;
            // a failure leaves the settings as they were.
            self.settings = before;
            return Err(why);
        }
        // Autobalance turned on: the host deals the sides (slice A1).
        if balance_on {
            self.balance_redeal();
        }
        self.lobby_dirty = true;
        Ok(())
    }

    /// The lobby's mission with friendly fire or the loadout rule changed:
    /// built again and sent to every player under the same number (agent
    /// decision: its planes and slots are the same, so requests made for it
    /// still stand). A kept loadout the new rule refuses goes back to the
    /// standard load, and its player is told, as a mission change does.
    fn respec(&mut self, spec: MissionSpec) -> Result<(), String> {
        let (world, manifest) = self.build(&spec).map_err(|error| match error {
            HostError::Mission(text) => text,
            other => other.to_string(),
        })?;
        self.spec = spec;
        self.spec_text = self.spec.to_text();
        self.world = world;
        self.manifest = manifest;
        self.tracker = super::Tracker::new(&self.world);
        self.revival = super::revive::Revivals::default();
        self.gives.clear();
        let slots = self.slots();
        let mut dropped = Vec::new();
        for (id, peer) in &self.peers {
            let (Some(plane), Some(load)) = (peer.lobby.slot, &peer.lobby.loadout) else {
                continue;
            };
            let aircraft = slots.iter().find(|s| s.id == plane.0).map(|s| s.aircraft);
            let kind = aircraft.and_then(|aircraft| {
                self.world
                    .combat
                    .dummy_types()
                    .iter()
                    .find(|kind| kind.profile.id == aircraft)
            });
            let refused = match kind {
                Some(kind) => load
                    .check_in(&kind.profile, &*self.resources, &self.spec)
                    .err()
                    .map(|error| error.to_string()),
                None => Some("the mission holds no such aircraft".into()),
            };
            if let Some(reason) = refused {
                dropped.push((*id, plane.0, reason));
            }
        }
        for (id, plane, reason) in dropped {
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.lobby.loadout = None;
            }
            self.send(
                id,
                &Message::Notice(format!(
                    "Your loadout for plane {plane} does not fit the King's new settings ({reason}); you have the standard load."
                )),
            );
        }
        let mission = self.mission_message();
        let roster = Message::Roster(self.roster());
        let to: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in to {
            self.send(id, &mission);
            self.send(id, &roster);
        }
        self.lobby_dirty = true;
        Ok(())
    }

    /// The host's own checks of a King's change, before anything applies:
    /// the phase, the registry, the player limit's floor and ceiling, the
    /// listing, PvP's settings in co-op, the name and the password.
    fn check_change(&self, change: &SettingsChange) -> Result<(), String> {
        let lobby = matches!(self.life, Life::Lobby);
        if change.cheats.is_some() && !lobby {
            return Err(IN_THE_LOBBY.into());
        }
        let mode = change
            .values
            .iter()
            .rev()
            .find(|(n, _)| *n == number::MODE)
            .and_then(|&(_, v)| Mode::from_value(v))
            .unwrap_or(self.settings.mode());
        for &(n, value) in &change.values {
            if let Some(why) = settings::refusal(n, value) {
                return Err(why);
            }
            let Some(setting) = settings::setting(n) else {
                continue;
            };
            if setting.change == Change::InLobby && !lobby {
                return Err(IN_THE_LOBBY.into());
            }
            if setting.pvp_only && mode != Mode::Pvp {
                return Err(format!("{} applies only in PvP.", setting.name));
            }
            match n {
                number::MAX_PLAYERS => {
                    let connected = self
                        .peers
                        .values()
                        .filter(|peer| !matches!(peer.stage, Stage::Closing { .. }))
                        .count();
                    if (value as usize) < connected {
                        return Err(format!(
                            "{connected} players are connected: the limit cannot be lower."
                        ));
                    }
                    if value as usize > self.config.max_players {
                        return Err(format!(
                            "This game takes at most {} players.",
                            self.config.max_players
                        ));
                    }
                }
                number::HOST => self.host_pin_refusal(value)?,
                number::VISIBILITY
                    if value == settings::Visibility::Public.value() && !self.config.listable =>
                {
                    return Err(PUBLIC_IS_BROADCAST.into());
                }
                number::SNAPSHOT_RATE if self.config.house.is_none() => {
                    return Err(RATE_IS_THE_FILES.into());
                }
                _ => {}
            }
        }
        if let Some(name) = &change.name
            && (name.is_empty() || name.len() > NAME_LIMIT || name.chars().any(char::is_control))
        {
            return Err(format!(
                "The game's name is 1 to {NAME_LIMIT} bytes of printable text."
            ));
        }
        if let Some(PasswordChange::Set(password)) = &change.password
            && (password.is_empty() || password.chars().any(char::is_control))
        {
            return Err("The password is 1 to 255 bytes of printable text.".into());
        }
        Ok(())
    }

    /// Clears every ready mark of a player in the lobby.
    fn clear_ready(&mut self) {
        for peer in self.peers.values_mut() {
            if !Self::in_flight(peer) {
                peer.lobby.ready = false;
            }
        }
        self.lobby_dirty = true;
    }

    /// After the mode changed the slots: a held slot that is no longer one
    /// is freed and its player told, and the locks of planes that are no
    /// longer slots go.
    fn fit_slots(&mut self) {
        let slots: Vec<u32> = self.slots().iter().map(|slot| slot.id).collect();
        self.court.locks.retain(|plane, _| slots.contains(plane));
        let lost: Vec<(ConnectionId, u32)> = self
            .peers
            .iter()
            .filter_map(|(id, peer)| {
                let plane = peer.lobby.slot?.0;
                (!slots.contains(&plane)).then_some((*id, plane))
            })
            .collect();
        for (id, plane) in lost {
            self.free_slot(
                id,
                format!("Plane {plane} is no longer a slot in this game's mode; take another."),
            );
        }
    }

    /// Frees the slot `connection` holds and tells the player `why`. A
    /// player flying it flies on; only its slot in the lobby goes.
    fn free_slot(&mut self, connection: ConnectionId, why: String) {
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        if Self::in_flight(peer) {
            peer.lobby.slot = None;
            peer.lobby.loadout = None;
        } else {
            peer.lobby.release();
        }
        let callsign = peer.callsign.clone();
        self.send(connection, &Message::Notice(why));
        self.lobby_log(callsign, LobbyEvent::Slot(None));
        self.lobby_dirty = true;
    }

    // ----- Slot locks ---------------------------------------------------

    /// The King opens, closes or reserves `plane`'s slot (message 27), in
    /// any phase (agent decision: a lock binds the next take; a player
    /// flying the plane flies on). The caller has checked that `connection`
    /// is the King and the mission's number.
    pub(super) fn lock_slot(
        &mut self,
        connection: ConnectionId,
        plane: u32,
        lock: &Lock,
    ) -> Result<(), String> {
        if !self.slots().iter().any(|slot| slot.id == plane) {
            return Err(format!("Plane {plane} is not a slot players may take."));
        }
        if let Lock::Reserved(callsign) = lock
            && (callsign.is_empty() || callsign.chars().any(char::is_control))
        {
            return Err("Name the player the slot is kept for.".into());
        }
        if self.court.lock(plane) == *lock {
            return Ok(());
        }
        match lock {
            Lock::Open => self.court.locks.remove(&plane),
            _ => self.court.locks.insert(plane, lock.clone()),
        };
        let king = self
            .peers
            .get(&connection)
            .map(|p| p.callsign.clone())
            .unwrap_or_default();
        self.lobby_log(
            king,
            LobbyEvent::SlotLocked {
                plane,
                lock: lock.clone(),
            },
        );
        // A lock that removes a player's slot frees it.
        let holder = self
            .peers
            .iter()
            .find(|(_, peer)| peer.lobby.slot == Some(PlaneId(plane)))
            .map(|(id, peer)| (*id, peer.callsign.clone()));
        if let Some((id, callsign)) = holder {
            let why = match lock {
                Lock::Closed => Some(format!(
                    "The King closed plane {plane}'s slot: the AI flies it."
                )),
                Lock::Reserved(kept) if *kept != callsign => {
                    Some(format!("The King keeps plane {plane}'s slot for {kept}."))
                }
                _ => None,
            };
            if let Some(why) = why {
                self.free_slot(id, why);
            }
        }
        self.clear_ready();
        Ok(())
    }

    /// Why the slot locks refuse `connection` holding or taking `plane`.
    pub(super) fn lock_refusal(&self, connection: ConnectionId, plane: u32) -> Option<String> {
        match self.court.lock(plane) {
            Lock::Open => None,
            Lock::Closed => Some(format!("Plane {plane} is closed: the AI flies it.")),
            Lock::Reserved(kept) => {
                let callsign = self.peers.get(&connection).map(|p| p.callsign.as_str());
                (callsign != Some(kept.as_str()))
                    .then(|| format!("Plane {plane} is kept for {kept}."))
            }
        }
    }

    /// Whether `plane`'s slot is closed: the handshake's capacity leaves it
    /// out.
    pub(super) fn closed_slot(&self, plane: u32) -> bool {
        self.court.lock(plane) == Lock::Closed
    }

    // ----- Taking a plane -----------------------------------------------

    /// Why the King's rules refuse `connection` taking `plane` now, if they
    /// do: the slot's lock, join in progress, lock sides, Autobalance. A
    /// revival (slice F2-V) is not a new pilot: it asks
    /// [`Host::sides_refusal`] alone.
    pub(super) fn king_take_refusal(
        &self,
        connection: ConnectionId,
        plane: PlaneId,
    ) -> Option<String> {
        // A slot's lock is its lineage's, by the root (the lobby pass's
        // follow-up F1).
        if let Some(why) = self.lock_refusal(connection, self.root_of(plane).0) {
            return Some(why);
        }
        self.new_pilot_refusal()
            .or_else(|| self.sides_refusal(connection, plane))
            .or_else(|| {
                // Autobalance: the side the host gave the player (slice A1).
                let side = self.world.roster.plane(plane)?.slot.wing.side;
                self.balance_side_refusal(connection, side)
            })
    }

    /// Why join in progress off refuses a new pilot now: the mission flies
    /// past its first tick.
    pub(super) fn new_pilot_refusal(&self) -> Option<String> {
        (!self.settings.join_in_progress()
            && matches!(self.life, Life::Flying)
            && self.world.tick() > 0)
            .then(|| NO_NEW_PILOTS.to_owned())
    }

    /// Why lock sides refuses `connection` a plane: it flew one on the other
    /// side earlier in this mission.
    pub(super) fn sides_refusal(&self, connection: ConnectionId, plane: PlaneId) -> Option<String> {
        if !self.settings.lock_sides() || !matches!(self.life, Life::Flying) {
            return None;
        }
        let peer = self.peers.get(&connection)?;
        let side = *self.court.sides.get(&peer.callsign)?;
        let wanted = self.world.roster.plane(plane)?.slot.wing.side;
        (wanted != side).then(|| SIDES_LOCKED.to_owned())
    }

    /// `connection` took `plane`: the first plane it flies this mission
    /// fixes its side for lock sides.
    pub(super) fn king_seated(&mut self, connection: ConnectionId, plane: PlaneId) {
        let (Some(peer), Some(entry)) =
            (self.peers.get(&connection), self.world.roster.plane(plane))
        else {
            return;
        };
        self.court
            .sides
            .entry(peer.callsign.clone())
            .or_insert(entry.slot.wing.side);
    }

    /// A mission starts flying: nobody has a side yet.
    pub(super) fn king_mission_start(&mut self) {
        self.court.sides.clear();
    }

    /// The mission changed from one whose slots were `old`: a lock stays on
    /// a plane that is still a slot of the same aircraft, as a kept loadout
    /// does, and the others go.
    pub(super) fn king_mission_changed(&mut self, old: &[tore_world::mission::OpenPlane]) {
        let slots = self.slots();
        self.court.locks.retain(|plane, _| {
            let aircraft = |list: &[tore_world::mission::OpenPlane]| {
                list.iter().find(|p| p.id == *plane).map(|p| p.aircraft)
            };
            aircraft(&slots).is_some() && aircraft(&slots) == aircraft(old)
        });
    }

    // ----- A crowned dedicated server -----------------------------------

    /// A crowned dedicated server empty for its empty timeout, in the lobby,
    /// goes back to its file's mission and settings, so a public server does
    /// not keep a stranger's choices.
    pub(super) fn king_update(&mut self, now: Duration) {
        if self.court.file_spec.is_none() {
            return;
        }
        if self
            .peers
            .values()
            .any(|peer| !matches!(peer.stage, Stage::Closing { .. }))
        {
            self.court.vacant_since = None;
            return;
        }
        let since = *self.court.vacant_since.get_or_insert(now);
        if now.saturating_sub(since) < self.config.empty_timeout
            || !matches!(self.life, Life::Lobby)
        {
            return;
        }
        let settings = Store::from_config(&self.config);
        let Some(file) = self.court.file_spec.clone() else {
            return;
        };
        let spec = with_settings(file, &settings);
        // The file's mission again, with the seed it flew on when it is the
        // same mission (a ground target drawn at the start keeps its layout).
        let mut spec = match self.spec.surface_seed {
            seed if seed != 0 && spec.surface_seed == 0 => {
                let mut same = spec.clone();
                same.surface_seed = seed;
                if same == self.spec {
                    same
                } else {
                    super::with_surface_seed(spec, self.session_id, self.number.wrapping_add(1))
                }
            }
            _ => super::with_surface_seed(spec, self.session_id, self.number.wrapping_add(1)),
        };
        if spec == self.spec && settings == self.settings && self.court.locks.is_empty() {
            return;
        }
        match self.build_seeded(&mut spec) {
            Ok((world, manifest)) => {
                self.settings = settings;
                self.spec = spec;
                self.spec_text = self.spec.to_text();
                self.world = world;
                self.manifest = manifest;
                self.tracker = super::Tracker::new(&self.world);
                self.revival = super::revive::Revivals::default();
                self.gives.clear();
                self.number = self.number.wrapping_add(1);
                self.court.locks.clear();
                self.lobby_log("The server".into(), LobbyEvent::BackToFile);
            }
            Err(error) => {
                let tick = self.world.tick();
                self.log(super::HostLog::Fault {
                    tick,
                    text: match error {
                        HostError::Mission(text) => text,
                        other => other.to_string(),
                    },
                });
            }
        }
    }
}

// Stage K: the court part of the session's state (slice K1).
#[path = "king_state.rs"]
pub(super) mod state;
