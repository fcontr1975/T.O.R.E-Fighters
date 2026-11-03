//! The mission recorder: every simulation tick of a flight, captured into a
//! `tore-replay` recording that a background thread encodes and writes.
//!
//! Per tick: [`Recorder::start_tick`] opens it, [`Recorder::begin`] runs
//! right after `Combat::advance_render` and turns the tick's render snapshot
//! and flight data into a frame, noting what changed (launches, hits, kills,
//! crashes, departures, AI activity and so on). Hooks later in the tick add
//! events with [`Recorder::note`] and the other noting methods, and
//! [`Recorder::end`] closes the frame. A closed frame waits until the next
//! tick opens, so anything noted between ticks (a pause, a bookmark, an
//! order) lands on the tick the player was looking at.
//!
//! Live flight never waits for the disk: frames travel to the writer on a
//! bounded queue, and if it is ever full the frame is dropped and the next
//! one carries a `system.gap` event. A headless probe recording, which has
//! no frame rate to protect, waits for the writer instead
//! ([`Recorder::wait_for_writer`]), so it never has gaps however loaded the
//! machine is. Nothing here feeds back into flight;
//! every read is of state the tick already computed. Opinionated addition
//! requested by John on 2026-09-26; see docs/REPLAYS.md.
//!
//! A recording is made for one seat ([`Recorder::for_seat`]): the plane that
//! seat flies is the recording's player, and the pilot's input, the command
//! notes, the cockpit sounds and the player-only reasons are its. Every other
//! aircraft, human-flown or not, is recorded as an ordinary aircraft. Single
//! player is seat 0 flying plane 0, which is the recorder's default.
//!
//! The reasons behind decisions live in two child modules: [`why`] turns the
//! AI's and the flight model's write-only records into reason events and
//! display trees, and [`journal`] turns the AI message journal and the
//! communication journal into `comms.*` events. A third, [`devices`], turns
//! combat's write-only notes of released chaff and flares and the player's
//! decoy rolls into entries a replay flies the devices from. A fourth,
//! [`datalink`], turns the data link's write-only journal into `datalink.*`
//! events. A fifth, [`surface`], records the surface world of a format 3
//! recording: the routed units' poses, launcher and magazine changes, names,
//! and the shots, resupply, radar and wreck events of surface units.
mod datalink;
mod devices;
mod display;
mod ground;
mod journal;
mod surface;
mod why;

use super::convert::{self, EffectWatch, FlightData, Presentation};
use super::trees;
use crate::{
    ai_wings::AiWings,
    combat::{self, CommandNote},
    flight, flight_ui,
    seats::SeatId,
    snapshot::RenderSnapshot,
    terrain,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use tore_formats::flight_model::departure::DepartureMode;
use tore_replay::{
    self as replay, Event, Frame,
    vocab::{field, kind, outcome},
};
use tore_sim::combat::{ledger, live};

/// Frames the writer may fall behind by before frames are dropped: two
/// seconds of flight.
const QUEUE_FRAMES: usize = 240;
/// Knots per foot per second.
const KT_PER_FPS: f64 = 1. / 1.687_81;

// A recording keeps every round the simulation can have in flight.
const _: () = assert!(replay::limits::MAX_PROJECTILES >= live::MAX_PROJECTILES);

enum Message {
    Aircraft(Box<replay::AircraftInfo>),
    Weapon(Box<replay::WeaponInfo>),
    Surface(Box<replay::SurfaceInfo>),
    Frame(Box<Frame>),
    Finish(Box<replay::Footer>),
}

/// The writer thread: it returns the finished recording's path.
type WriterThread = JoinHandle<Result<PathBuf, String>>;

/// Frames the recorder keeps emptied and ready, and the most the writer
/// hands back at once. A tick builds in one of them, so its lists keep the
/// room the last ticks grew them to.
const SPARE_FRAMES: usize = 8;

/// Starts the writer thread. It owns the file; when the game drops the
/// recorder without finishing, the writer keeps its `.partial` file. Each
/// frame it has written goes back emptied, through the receiver, for the
/// recorder to build a later tick in.
fn spawn(
    writer: replay::Writer,
) -> std::io::Result<(SyncSender<Message>, Receiver<Frame>, WriterThread)> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(QUEUE_FRAMES);
    let (give_back, returned) = std::sync::mpsc::sync_channel(SPARE_FRAMES);
    let thread = std::thread::Builder::new()
        .name("tore-recorder".into())
        .spawn(move || {
            let mut writer = writer;
            let mut refused = 0u64;
            for message in receiver {
                let result = match message {
                    Message::Aircraft(info) => writer.register_aircraft(&info),
                    Message::Weapon(info) => writer.register_weapon(&info),
                    Message::Surface(info) => writer.register_surface_unit(&info),
                    Message::Frame(frame) => {
                        let mut frame = *frame;
                        let result = writer.push(&frame);
                        frame.clear();
                        // Full, or the recorder gone: this one is dropped.
                        let _ = give_back.try_send(frame);
                        result
                    }
                    Message::Finish(footer) => {
                        if refused > 0 {
                            log::info!("Recording: {refused} items were refused while writing");
                        }
                        return writer.finish(&footer).map_err(|e| e.to_string());
                    }
                };
                if let Err(error) = result {
                    if refused == 0 {
                        log::info!("Recording: {error}");
                    }
                    refused += 1;
                }
            }
            Err("the recording was not finished; its .partial file keeps what was written".into())
        })?;
    Ok((sender, returned, thread))
}

/// Everything [`Recorder::begin`] reads for one tick.
pub struct Tick<'a> {
    /// The tick's picture, just taken by `Combat::advance_render`.
    pub snapshot: &'a RenderSnapshot,
    pub combat: &'a combat::Combat,
    /// The player's flight state after the tick, and before it.
    pub flight: &'a flight::State,
    pub previous: &'a flight::State,
    /// The player's controls for the tick.
    pub pilot: &'a flight::PilotInput,
    /// The other human-flown planes, recorded as ordinary aircraft.
    pub others: &'a [Crewed<'a>],
    pub wings: Option<&'a AiWings>,
    pub world: &'a terrain::Terrain,
    /// Combat's events for the tick.
    pub events: &'a [live::Event],
    /// Shot outcomes the ledger resolved during the tick.
    pub outcomes: &'a [ledger::Outcome],
    /// The AI message journal, drained for this tick (`None` without AI).
    pub journal: Option<&'a tore_sim::ai::thought::JournalBatch>,
}

/// A human-flown plane other than the recording's player: what the recorder
/// reads of it is what it reads of an AI aircraft, from its own cockpit.
pub struct Crewed<'a> {
    pub plane: u32,
    pub flight: &'a flight::State,
    /// Its seat's controls for the tick.
    pub pilot: &'a flight::PilotInput,
}

/// What the recorder remembers about one aircraft between ticks.
#[derive(Clone, Debug, Default)]
struct Watch {
    seen: bool,
    departure: Option<DepartureMode>,
    on_ground: bool,
    engine: bool,
    fuel_out: bool,
    crashed: bool,
    /// Its end (a crash or the wreck's end) was already reported.
    ended: bool,
    wreck: Option<tore_sim::wreck::Phase>,
    escape: Option<tore_sim::ejection::Phase>,
    pilot_dead: bool,
    hp: i32,
    sections: [i32; replay::SECTION_COUNT],
}

/// The rows of a slice found by id, as the map from ids to rows that every
/// tick used to build answered: of a repeated id the last row. The order
/// vector is the recorder's, handed in and taken back, so a tick allocates no
/// tree.
struct ById<'a, T> {
    rows: &'a [T],
    order: Vec<(u32, u32)>,
}
impl<'a, T> ById<'a, T> {
    fn new(rows: &'a [T], id: impl Fn(&T) -> u32, mut order: Vec<(u32, u32)>) -> Self {
        order.clear();
        order.extend(
            rows.iter()
                .enumerate()
                .map(|(index, row)| (id(row), index as u32)),
        );
        order.sort_unstable();
        let mut kept = 0;
        for at in 0..order.len() {
            if order.get(at + 1).is_some_and(|next| next.0 == order[at].0) {
                continue;
            }
            order[kept] = order[at];
            kept += 1;
        }
        order.truncate(kept);
        Self { rows, order }
    }
    fn get(&self, id: &u32) -> Option<&'a T> {
        let at = self.order.binary_search_by_key(id, |(id, _)| *id).ok()?;
        Some(&self.rows[self.order[at].1 as usize])
    }
    fn into_order(self) -> Vec<(u32, u32)> {
        self.order
    }
}

/// What the recorder remembers about one projectile between ticks.
#[derive(Clone, Copy, Debug, Default)]
struct Shot {
    owner: u32,
    weapon: u32,
    target: Option<u32>,
    status: Option<u8>,
    position: [f64; 3],
    /// Its lost track was already reported.
    lost: bool,
    /// When it lost track, for the reason its outcome gives.
    lost_at: Option<u64>,
    /// The station it left, and the clock and age its fuze counts from, as
    /// the simulation held them (a bomb that lands before its fuze arms is a
    /// dud, [`ground::is_dud`]).
    station: usize,
    launched_t: u16,
    age: u64,
    /// The round flies on a motion of its own (a guided weapon), so its fuze
    /// counts ticks of age.
    motion: bool,
}

/// What the recorder knows of a weapon type for its events and words.
#[derive(Clone, Debug)]
struct WeaponNote {
    name: String,
    class: replay::WeaponClass,
    /// The fuze's arming time, quarter seconds.
    arm_quarters: u16,
    /// A cluster bomb (its ground burst scatters submunitions).
    cluster: bool,
    /// A bomb, rocket or air-to-ground missile: a weapon whose kills the
    /// ground events name.
    air_to_ground: bool,
}

impl WeaponNote {
    fn of(weapon: &tore_formats::weapons::Weapon, class: replay::WeaponClass) -> Self {
        use tore_sim::combat::{guided, missiles};
        Self {
            name: weapon.hud_name.clone(),
            class,
            arm_quarters: weapon.damage.fuze_arm_t,
            cluster: weapon.effects.land_explosion == 18,
            air_to_ground: matches!(
                class,
                replay::WeaponClass::Bomb | replay::WeaponClass::Rocket
            ) || guided::kind(weapon).is_some()
                || missiles::SURFACE_ROLE.contains(&weapon.source.as_str()),
        }
    }
}

/// The seeker tone the player hears, without its loudness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tone {
    radar: bool,
    ground: bool,
    locked: bool,
}

impl Tone {
    fn name(self) -> &'static str {
        use tore_replay::vocab::tone;
        match (self.radar, self.locked, self.ground) {
            (true, true, _) => tone::RADAR_LOCK,
            (true, false, _) => tone::RADAR_SEARCH,
            (false, true, _) => tone::INFRARED_LOCK,
            (false, false, true) => tone::GROUND,
            (false, false, false) => tone::INFRARED_SEARCH,
        }
    }
}

/// The smallest change in seeker tone loudness that is recorded, so a tone
/// that swells with the seeker's quality makes a few entries, not one a
/// tick (agent decision, 2026-09-26).
const TONE_STEP: f64 = 0.05;

/// The `audio.tone` entry for the player's seeker tone, `now` against what
/// was `last` recorded, each with its loudness: when it starts, stops,
/// changes, or its loudness moves by [`TONE_STEP`] or more.
fn tone_event(player: u32, last: Option<(Tone, f64)>, now: Option<(Tone, f64)>) -> Option<Event> {
    let event = |tone: Tone, on: bool| {
        Event::new(kind::AUDIO_TONE)
            .with_subject(player)
            .with(field::TONE, tone.name())
            .with(field::ON, on)
    };
    match (last, now) {
        (Some((was, then)), Some((tone, strength)))
            if was == tone && (strength - then).abs() < TONE_STEP =>
        {
            None
        }
        (_, Some((tone, strength))) => Some(
            event(tone, true)
                .with(field::STRENGTH, trees::round(strength, 2))
                .with(field::SURFACE, tone.ground),
        ),
        (Some((was, _)), None) => Some(event(was, false)),
        (None, None) => None,
    }
}

/// The player's view target as recorded: the target the flight views
/// follow, and whether the views keep it by sight after the sensors
/// dropped it.
type ViewTarget = (Option<u32>, bool);

/// The `player.view_target` entry for `now` against what was `last`
/// recorded: at the first look, and whenever the target or its sight hold
/// changes.
fn view_target_event(player: u32, last: Option<ViewTarget>, now: ViewTarget) -> Option<Event> {
    if last == Some(now) {
        return None;
    }
    let (target, held) = now;
    let mut event = Event::new(kind::PLAYER_VIEW_TARGET)
        .with_subject(player)
        .with(field::HELD, held);
    if let Some(id) = target {
        event = event.with_object(id);
    }
    if let Some(from) = last.and_then(|(id, _)| id) {
        event = event.with(field::FROM, replay::Value::Id(from));
    }
    if held {
        event = event.with(
            field::REASON,
            "the sensors dropped it and it is within visual range",
        );
    }
    Some(event)
}

/// Pause, time compression and cheats, compared each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Session {
    frozen: bool,
    time_scale: f64,
    cheats: tore_sim::cheats::Cheats,
}

/// Records one flight. See the module documentation.
pub struct Recorder {
    path: PathBuf,
    sender: Option<SyncSender<Message>>,
    thread: Option<WriterThread>,
    /// Frames the writer has written and emptied, to build ticks in.
    returned: Option<Receiver<Frame>>,
    spare: Vec<Frame>,
    /// Messages the queue could not take, sent before the next frame.
    backlog: VecDeque<Message>,
    /// The frame being built between `begin` and `end`.
    frame: Option<Frame>,
    /// The last closed frame, still open to notes made between ticks.
    held: Option<Frame>,
    /// Notes made during a tick before its frame begins.
    early: Vec<Event>,
    registered: BTreeSet<u32>,
    weapons: BTreeMap<String, u32>,
    /// The source name of each registered weapon, by id.
    weapon_sources: Vec<String>,
    /// Scratch for the id orders of a tick's target and projectile lookups.
    id_orders: [Vec<(u32, u32)>; 2],
    /// What each registered weapon type is, by id.
    weapon_notes: BTreeMap<u32, WeaponNote>,
    effects: EffectWatch,
    /// Serial of the next crater or fire to record.
    marks: u64,
    /// The city fill's version and knocked-down buildings the recording
    /// last wrote (nr-fx3).
    city: Option<(u64, Vec<u32>)>,
    surface: BTreeMap<u32, i32>,
    watches: BTreeMap<u32, Watch>,
    shots: BTreeMap<u32, Shot>,
    /// Dropped frames not yet reported: first and last tick, events lost.
    gap: Option<(u64, u64, usize)>,
    /// Wait for a full queue instead of dropping frames (headless runs).
    wait: bool,
    /// The seeker tone and its loudness as last recorded.
    tone: Option<(Tone, f64)>,
    stall: Option<&'static str>,
    danger: bool,
    /// The player's view target as last recorded.
    view: Option<ViewTarget>,
    /// The plane the recording is for, its player, and the seat that flies it.
    player: u32,
    seat: SeatId,
    session: Option<Session>,
    bookmarks: u32,
    frames: u64,
    last_tick: Option<u64>,
    /// Copies of the comms and audio entries written, for live flight's
    /// Comms panel, until it takes them.
    comms: Vec<replay::TimedEvent>,
    /// Live flight's debug panels are showing: the display trees written
    /// are copied for them too.
    live: bool,
    /// Copies of the display trees written while `live`, until taken.
    trees: Vec<(u64, replay::TreeSample)>,
    /// Every registered identity, for names in reasons and trees.
    infos: BTreeMap<u32, replay::AircraftInfo>,
    /// What the reason events and trees remember between ticks.
    why: why::Why,
    /// The surface world of a format 3 recording.
    surface_watch: surface::Watch,
    /// Each aircraft's stores as last recorded (B3), so only changes are
    /// events.
    stores: tore_session::client::convert::stores::StoresWatch,
    /// The AI aircraft's chaff and flares left as the last tick was
    /// recorded, for the releases its end collects.
    dispensers: devices::Dispensers,
    /// The air-to-ground round: guided weapons, lasers, designations.
    ground: ground::Watch,
    /// The quarter-second clock of the step that has just run, for fuzes.
    state_clock: Option<u16>,
    /// The sensor console's orbit as last written: the tick and the point
    /// (nr-orbit), `None` while it is closed.
    orbit: Option<(u64, [f64; 3])>,
    /// Each aircraft's HUD and sight, written when they change (nr-replayhud).
    display: tore_session::client::convert::display::Seen,
}

/// Comms and audio entries kept for live flight between its looks.
const COMMS_TAP: usize = 1_024;
/// Display trees kept for live flight between its looks.
const TREES_TAP: usize = 4_096;

impl Recorder {
    /// Starts writing a recording to `path`, which must not exist yet.
    /// `roster` names every aircraft known at the start.
    pub fn start(
        path: PathBuf,
        header: &replay::Header,
        roster: &[replay::AircraftInfo],
    ) -> Result<Self, String> {
        let writer = replay::Writer::create(&path, header).map_err(|e| e.to_string())?;
        let partial = writer.partial_path().to_path_buf();
        let surface = writer.version() >= 3;
        let (sender, returned, thread) = spawn(writer).map_err(|e| e.to_string())?;
        log::info!("Recording to {}", partial.display());
        let mut recorder = Self::with(path, sender, Some(thread), roster);
        recorder.returned = Some(returned);
        recorder.surface_watch = surface::Watch::new(surface);
        Ok(recorder)
    }

    /// A recorder whose queue the caller reads, with no file, for tests.
    #[cfg(test)]
    fn detached(
        capacity: usize,
        roster: &[replay::AircraftInfo],
    ) -> (Self, std::sync::mpsc::Receiver<Message>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity);
        (Self::with(PathBuf::new(), sender, None, roster), receiver)
    }

    fn with(
        path: PathBuf,
        sender: SyncSender<Message>,
        thread: Option<WriterThread>,
        roster: &[replay::AircraftInfo],
    ) -> Self {
        let mut recorder = Self {
            path,
            sender: Some(sender),
            thread,
            returned: None,
            spare: Vec::new(),
            backlog: VecDeque::new(),
            frame: None,
            held: None,
            early: Vec::new(),
            registered: BTreeSet::new(),
            weapons: BTreeMap::new(),
            weapon_sources: Vec::new(),
            id_orders: Default::default(),
            weapon_notes: BTreeMap::new(),
            effects: EffectWatch::default(),
            marks: 0,
            city: None,
            surface: BTreeMap::new(),
            watches: BTreeMap::new(),
            shots: BTreeMap::new(),
            gap: None,
            wait: false,
            tone: None,
            stall: None,
            danger: false,
            view: None,
            player: 0,
            seat: SeatId(0),
            session: None,
            bookmarks: 0,
            frames: 0,
            last_tick: None,
            comms: Vec::new(),
            live: false,
            trees: Vec::new(),
            infos: BTreeMap::new(),
            why: why::Why::default(),
            surface_watch: surface::Watch::default(),
            stores: Default::default(),
            dispensers: devices::Dispensers::new(),
            ground: ground::Watch::default(),
            state_clock: None,
            orbit: None,
            display: tore_session::client::convert::display::Seen::default(),
        };
        for info in roster {
            recorder.register(info.clone());
        }
        recorder
    }

    /// Where the finished recording will be, so cleanup can spare it.
    #[allow(dead_code)] // Read by the Replays screen.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records for `seat`, which flies `plane`: that plane is the recording's
    /// player. The default is seat 0 flying plane 0, single player.
    pub fn for_seat(mut self, seat: SeatId, plane: u32) -> Self {
        self.seat = seat;
        self.player = plane;
        self
    }

    /// The plane the recording is for, its player.
    #[allow(dead_code)] // Read by tests and the probe.
    pub fn player(&self) -> u32 {
        self.player
    }

    fn register(&mut self, info: replay::AircraftInfo) {
        if self.registered.insert(info.id) {
            self.infos.insert(info.id, info.clone());
            self.send(Message::Aircraft(Box::new(info)));
        }
    }

    /// An aircraft's label for reasons: `You`, `Enemy 2-1`.
    fn who(&self, id: u32) -> String {
        who(&self.infos, id)
    }

    /// Makes the recorder wait for a full queue instead of dropping frames.
    /// For headless runs only: live flight must never wait for the disk.
    pub fn wait_for_writer(&mut self) {
        self.wait = true;
    }

    /// Queues a message for the writer and says whether it went. Normally it
    /// never waits: registrations that do not fit wait in the backlog and
    /// count as sent; a frame that does not fit is not sent. After
    /// [`Recorder::wait_for_writer`] every message waits for room instead.
    fn send(&mut self, message: Message) -> bool {
        let Some(sender) = &self.sender else {
            return false;
        };
        if self.wait {
            let mut pending = std::mem::take(&mut self.backlog);
            pending.push_back(message);
            for message in pending {
                if sender.send(message).is_err() {
                    self.sender = None;
                    return false;
                }
            }
            return true;
        }
        while !self.backlog.is_empty() {
            match sender.try_send(self.backlog.pop_front().expect("not empty")) {
                Ok(()) => {}
                Err(TrySendError::Full(message)) => {
                    self.backlog.push_front(message);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.sender = None;
                    return false;
                }
            }
        }
        let message = if self.backlog.is_empty() {
            match sender.try_send(message) {
                Ok(()) => return true,
                Err(TrySendError::Full(message)) => message,
                Err(TrySendError::Disconnected(_)) => {
                    self.sender = None;
                    return false;
                }
            }
        } else {
            message
        };
        match message {
            Message::Frame(_) => false,
            other => {
                self.backlog.push_back(other);
                true
            }
        }
    }

    /// Notes the ticks dropped so far at the start of `frame`.
    fn mark_gap(&self, frame: &mut Frame) {
        if let Some((from, to, lost)) = self.gap {
            frame.events.insert(
                0,
                Event::new(kind::SYSTEM_GAP)
                    .with(field::FROM, from as i64)
                    .with(field::TO, to as i64)
                    .with("events_lost", lost as i64)
                    .with_text("the recorder fell behind and dropped these ticks"),
            );
        }
    }

    /// Hands the held frame to the writer without waiting. If the queue is
    /// full the frame is dropped and joins the gap the next frame reports.
    fn release_held(&mut self) {
        let Some(mut frame) = self.held.take() else {
            return;
        };
        let (tick, events) = (frame.tick, frame.events.len());
        // Live flight's Comms panel lists what the recording holds, even
        // when the writer falls behind.
        self.comms.extend(
            frame
                .events
                .iter()
                .filter(|e| crate::replay::panels::Channel::of(&e.kind).is_some())
                .map(|e| replay::TimedEvent {
                    tick,
                    event: e.clone(),
                }),
        );
        if self.comms.len() > COMMS_TAP {
            self.comms.drain(..self.comms.len() - COMMS_TAP);
        }
        // Its display trees too while the panels show, so a live panel
        // shows the very tree the recording keeps.
        if self.live {
            self.trees
                .extend(frame.trees.iter().map(|tree| (tick, tree.clone())));
            if self.trees.len() > TREES_TAP {
                self.trees.drain(..self.trees.len() - TREES_TAP);
            }
        }
        self.mark_gap(&mut frame);
        fit(&mut frame);
        if self.send(Message::Frame(Box::new(frame))) {
            self.gap = None;
        } else if self.sender.is_some() {
            let (from, _, lost) = self.gap.unwrap_or((tick, tick, 0));
            self.gap = Some((from, tick, lost + events));
        }
    }

    /// The comms and audio entries written since the last call, oldest
    /// first, for live flight's Comms panel. Only the newest 1,024 wait.
    pub fn take_comms(&mut self) -> Vec<replay::TimedEvent> {
        std::mem::take(&mut self.comms)
    }

    /// Whether live flight's debug panels are showing, so the display trees
    /// written are kept for them.
    pub fn set_live(&mut self, live: bool) {
        self.live = live;
        if !live {
            self.trees.clear();
        }
    }

    /// The display trees written since the last call while live, oldest
    /// first, each with its tick.
    pub fn take_trees(&mut self) -> Vec<(u64, replay::TreeSample)> {
        std::mem::take(&mut self.trees)
    }

    /// Adds an event: to the tick being recorded, or between ticks to the
    /// last one, or before the first frame to the first.
    pub fn note(&mut self, event: Event) {
        if let Some(frame) = &mut self.frame {
            frame.events.push(event);
        } else if let Some(held) = &mut self.held {
            held.events.push(event);
        } else {
            self.early.push(event);
        }
    }

    /// Opens a simulation tick: the previous frame is written, and anything
    /// noted from now on belongs to this tick. Notes that `ui` and `combat`
    /// collected between ticks go on the previous frame first, so the live
    /// loop calls it once the tick's commands have been applied and their
    /// messages shown, when those notes exist.
    pub fn start_tick(
        &mut self,
        ui: Option<&mut flight_ui::FlightUi>,
        combat: &mut combat::Combat,
    ) {
        self.collect(ui, combat);
        self.release_held();
    }

    /// Cockpit messages, player commands, the player's view target,
    /// released chaff and flares and the player's decoy rolls collected
    /// since the last look.
    fn collect(&mut self, ui: Option<&mut flight_ui::FlightUi>, combat: &mut combat::Combat) {
        if let Some(ui) = ui {
            for (text, shown) in ui.take_notes() {
                let (result, reason) = match shown {
                    flight_ui::Shown::Now => (outcome::DELIVERED, ""),
                    flight_ui::Shown::Repeated => (
                        outcome::DELIVERED,
                        "the same message was on screen: it moved to the bottom with a fresh timer",
                    ),
                    flight_ui::Shown::PushedOff => (
                        outcome::REPLACED,
                        "seven newer lines pushed it off the screen",
                    ),
                };
                let mut event = Event::new(kind::COMMS_HUD)
                    .with(field::OUTCOME, result)
                    .with_text(text);
                if !reason.is_empty() {
                    event = event.with(field::REASON, reason);
                }
                self.note(event);
            }
        }
        let player = self.player;
        let designated = combat.state.view(player).and_then(|view| view.designated());
        for note in combat.take_notes() {
            let (name, text) = match note {
                CommandNote::Command(command) => (
                    crate::combat_tape::command_name(command),
                    format!("{command:?}"),
                ),
                CommandNote::Release => ("release".to_owned(), "trigger released".to_owned()),
            };
            let mut event = Event::new(kind::PLAYER_COMMAND)
                .with_subject(player)
                .with(field::COMMAND, name)
                .with_text(text);
            if let Some(target) = designated {
                event = event.with_object(target);
            }
            self.note(event);
        }
        // Looked at after each tick and before the next, so a change a
        // command makes between ticks lands on the tick on screen, like
        // the command.
        let own = combat.state.view(player);
        let view = own
            .as_ref()
            .and_then(|view| view.view_target())
            .map(|t| t.id);
        let held = view.is_some()
            && own
                .as_ref()
                .and_then(|view| view.display_target())
                .map(|t| t.id)
                != view;
        if let Some(event) = view_target_event(player, self.view, (view, held)) {
            self.note(event);
        }
        self.view = Some((view, held));
        self.countermeasures(combat);
    }

    /// Closes the tick's frame. It is written when the next tick opens, or
    /// when the recording finishes.
    pub fn end(&mut self, ui: Option<&mut flight_ui::FlightUi>, combat: &mut combat::Combat) {
        self.collect(ui, combat);
        if let Some(frame) = self.frame.take() {
            self.held = Some(frame);
        }
    }

    /// Pause, time compression and cheat changes, checked once per rendered
    /// frame so a pause is noted when it happens.
    pub fn session(&mut self, ui: &flight_ui::FlightUi) {
        let now = Session {
            frozen: ui.frozen(),
            time_scale: ui.time_scale,
            cheats: ui.cheats,
        };
        let Some(before) = self.session.replace(now) else {
            return;
        };
        if before.frozen != now.frozen {
            self.note(Event::new(if now.frozen {
                kind::SYSTEM_PAUSE
            } else {
                kind::SYSTEM_RESUME
            }));
        }
        if before.time_scale != now.time_scale {
            self.note(
                Event::new(kind::SYSTEM_TIME_SCALE)
                    .with(field::SCALE, now.time_scale)
                    .with_text(format!("time {}x", now.time_scale)),
            );
        }
        for (name, was, is) in cheat_changes(&before.cheats, &now.cheats) {
            if was != is {
                self.note(
                    Event::new(kind::SYSTEM_CHEAT)
                        .with(field::CHEAT, name)
                        .with(field::ON, is),
                );
            }
        }
        if before.cheats.enemy_ai != now.cheats.enemy_ai {
            self.note(
                Event::new(kind::SYSTEM_CHEAT)
                    .with(field::CHEAT, "enemy_ai")
                    .with(field::ON, now.cheats.enemy_ai.is_some())
                    .with_text(format!("{:?}", now.cheats.enemy_ai)),
            );
        }
    }

    /// Marks this moment; returns the bookmark's number, from 1.
    pub fn bookmark(&mut self) -> u32 {
        self.bookmarks += 1;
        let number = self.bookmarks;
        self.note(
            Event::new(kind::PLAYER_BOOKMARK)
                .with_subject(self.player)
                .with("index", i64::from(number))
                .with_text(format!("Bookmark {number}")),
        );
        number
    }

    /// An AI aircraft's pilot ejected; `friendly` wingmen play a cue.
    pub fn wing_ejection(&mut self, id: u32, message: &str, friendly: bool) {
        if friendly {
            self.note(
                Event::new(kind::AUDIO_EJECTION)
                    .with_subject(id)
                    .with(field::SOUND, "^PUNCH.5K")
                    .with_text(message),
            );
        }
    }

    /// Sounds the tick released: combat's emissions and the player's
    /// weapon release sounds, as the tick drained them.
    pub fn sounds(
        &mut self,
        emissions: &[tore_sim::acoustics::Emission],
        releases: &[(&str, &tore_formats::weapons::Weapon)],
    ) {
        for emission in emissions {
            let [x, y, z] = emission.position;
            let mut event = Event::new(kind::AUDIO_EFFECT)
                .with(field::SOUND, emission.kind.name())
                .with(field::X_FT, x)
                .with(field::Y_FT, y)
                .with(field::Z_FT, z);
            if emission.own {
                event = event.with(field::OWN, true);
            }
            self.note(event);
        }
        for (sound, weapon) in releases {
            let weapon = self.weapon_id(weapon);
            self.note(
                Event::new(kind::AUDIO_RELEASE)
                    .with_subject(self.player)
                    .with(field::SOUND, *sound)
                    .with(field::WEAPON, replay::Value::Id(weapon)),
            );
        }
    }

    /// The registered id for a weapon type, registering it on first use.
    fn weapon_id(&mut self, weapon: &tore_formats::weapons::Weapon) -> u32 {
        if let Some(id) = self.weapons.get(&weapon.source) {
            return *id;
        }
        let id = self.weapons.len() as u32;
        self.weapons.insert(weapon.source.clone(), id);
        self.weapon_sources.push(weapon.source.clone());
        let info = convert::weapon_info(id, weapon);
        self.weapon_notes
            .insert(id, WeaponNote::of(weapon, info.class));
        self.send(Message::Weapon(Box::new(info)));
        id
    }

    /// [`Self::weapon_id`] for a round whose id the last tick knew: the same
    /// id when the round's weapon still has that id's name, with no search.
    fn weapon_id_after(
        &mut self,
        weapon: &tore_formats::weapons::Weapon,
        before: Option<u32>,
    ) -> u32 {
        if let Some(id) = before
            && self
                .weapon_sources
                .get(id as usize)
                .is_some_and(|source| *source == weapon.source)
        {
            return id;
        }
        self.weapon_id(weapon)
    }

    /// An empty frame: one the writer has handed back when there is one.
    fn fresh_frame(&mut self) -> Frame {
        if let Some(returned) = &self.returned {
            while let Ok(frame) = returned.try_recv() {
                if self.spare.len() < SPARE_FRAMES {
                    self.spare.push(frame);
                }
            }
        }
        self.spare.pop().unwrap_or_default()
    }

    /// Records the tick. See the module documentation.
    pub fn begin(&mut self, tick: Tick<'_>) {
        // A frame left open, or one held from the last tick, goes first.
        if let Some(open) = self.frame.take() {
            self.release_held();
            self.held = Some(open);
        }
        self.release_held();
        let snapshot = tick.snapshot;
        let number = snapshot.tick;
        if self.last_tick.is_some_and(|last| number <= last) {
            // A tick can be recorded once only; a repeat means the picture
            // did not advance.
            return;
        }
        self.last_tick = Some(number);
        self.state_clock = Some(ground::impact_clock(&tick.combat.state));
        let mut frame = self.fresh_frame();
        frame.tick = number;
        frame.events = std::mem::take(&mut self.early);
        let targets = ById::new(
            &tick.combat.state.targets,
            |t| t.id,
            std::mem::take(&mut self.id_orders[0]),
        );
        let player_ground = ground_height(tick.world, tick.flight.position);
        let player = self.player;

        // Aircraft: the player first, then every other aircraft in snapshot
        // order, another human-flown plane among them. Ground objects keep
        // only their hit points.
        let aircraft = drawn_aircraft(snapshot, player);
        for pose in &aircraft {
            if !self.registered.contains(&pose.id) {
                self.register(default_info(pose, tick.wings));
            }
            let data = flight_data(player, pose, &tick, &targets, player_ground);
            frame.aircraft.push(convert::aircraft_state(pose, &data));
        }
        self.id_orders[0] = targets.into_order();
        // Surface rows whose hit points ran out this tick: wrecks.
        let mut lost: Vec<(u32, i32)> = Vec::new();
        for target in snapshot.targets.iter().filter(|p| p.aircraft.is_none()) {
            let before = self.surface.insert(target.id, target.damage.hp);
            if before != Some(target.damage.hp) {
                frame.surface_hp.push((target.id, target.damage.hp));
                if let Some(before) = before.filter(|hp| *hp > 0)
                    && target.damage.hp <= 0
                {
                    lost.push((target.id, before));
                }
            }
        }
        self.surface_tracks(&tick, &mut frame, &lost);
        if number.is_multiple_of(replay::TICKS_PER_SECOND) {
            frame.checksum = Some(replay::state_checksum(&frame.aircraft));
        }

        // Weapons in flight.
        let live_shots = ById::new(
            &tick.combat.state.projectiles,
            |p| p.id,
            std::mem::take(&mut self.id_orders[1]),
        );
        let mut laid: Vec<(u32, Shot)> = Vec::with_capacity(snapshot.projectiles.len());
        for pose in &snapshot.projectiles {
            let simulated = live_shots.get(&pose.id);
            let before = self.shots.get(&pose.id).copied();
            let weapon = simulated.map(|p| p.weapon(stores_of(&tick, player, p.owner)));
            let weapon_id = match weapon {
                Some(weapon) => self.weapon_id_after(weapon, before.map(|shot| shot.weapon)),
                None => self.weapons.get(&pose.weapon).copied().unwrap_or(u32::MAX),
            };
            let seeker = simulated
                .and_then(|p| p.guidance.as_ref())
                .map(convert::seeker);
            frame.projectiles.push(convert::projectile_state(
                pose,
                weapon_id,
                simulated.map_or(0, |p| p.age),
                seeker,
            ));
            laid.push((
                pose.id,
                Shot {
                    owner: pose.owner,
                    weapon: weapon_id,
                    target: pose.target,
                    status: seeker.map(|s| s.status),
                    position: pose.position,
                    lost: before.is_some_and(|shot| shot.lost),
                    lost_at: before.and_then(|shot| shot.lost_at),
                    station: simulated.map_or(0, |p| p.station),
                    launched_t: simulated.map_or(0, |p| p.launched_t),
                    age: simulated.map_or(0, |p| p.age),
                    motion: simulated.is_some_and(|p| p.motion.is_some()),
                },
            ));
        }
        // Built in one go from the rounds in the order they flew, which the
        // map sorts; a repeated id keeps its last row, as inserting did.
        let mut shots: BTreeMap<u32, Shot> = laid.into_iter().collect();
        frame.debris = convert::debris_states(&snapshot.debris);
        frame
            .escapees
            .extend(snapshot.pilots.iter().map(convert::escapee_state));
        frame.new_effects = self.effects.started(&snapshot.effects);
        for mark in &tick.combat.state.marks {
            if mark.serial >= self.marks {
                frame.new_effects.push(convert::mark_spawn(mark));
                self.marks = mark.serial + 1;
            }
        }
        new_puffs(tick.combat, &mut frame.new_puffs);
        self.city_changes(tick.world, &mut frame);

        // The stores each aircraft carries (B3): its layout the first time,
        // then each change, so the viewer draws them emptying.
        let mut events = self.stores.poses(aircraft.iter().copied());
        // Decoy rolls first: they explain this tick's spoofed outcomes.
        self.decoys(&tick, &frame, &mut events);
        self.dispensers = devices::dispensers(tick.wings);
        self.weapon_events(&tick, &frame, &mut shots, &live_shots, &mut events);
        self.id_orders[1] = live_shots.into_order();
        self.surface_watch.events(&tick, &lost, &mut events);
        self.aircraft_events(&tick, &frame, &mut events);
        self.cue_events(&tick, player_ground, &mut events);
        self.orbit_event(&tick, number, &mut events);
        events.extend(self.display_events(&tick, number));
        self.ground.events(
            &ground::Context {
                tick: &tick,
                frame: &frame,
                old: &self.shots,
                new: &shots,
                notes: &self.weapon_notes,
                infos: &self.infos,
            },
            &mut events,
        );
        self.shots = shots;
        // The reasons behind the tick and its display trees.
        self.explain(&tick, &mut frame, &mut events);
        frame.events.extend(events);
        self.frames += 1;
        self.frame = Some(frame);
    }

    /// `player.orbit`: the AC-130's sensor console's orbit starting or
    /// ending (opening or closing on a point, or a pin made or let go while
    /// it searches, slice nr-cona), and the point its orbit circles when it
    /// has moved more than 50 ft, at most once a second (slice nr-orbit).
    fn orbit_event(&mut self, tick: &Tick<'_>, number: u64, events: &mut Vec<Event>) {
        const MOVED_FT: f64 = 50.;
        let status = tick.flight.orbit_status();
        let due = match (self.orbit, status) {
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
            (Some((at, was)), Some(now)) => {
                number.saturating_sub(at) >= replay::TICKS_PER_SECOND
                    && (now.point[0] - was[0]).hypot(now.point[2] - was[2]) > MOVED_FT
            }
        };
        if !due {
            return;
        }
        let mut event = Event::new(kind::PLAYER_ORBIT)
            .with_subject(self.player)
            .with(field::ON, status.is_some());
        if let Some(now) = status {
            for (name, value) in field::POSITION.into_iter().zip(now.point) {
                event = event.with(name, value);
            }
            event = event
                .with(field::RADIUS_FT, now.radius_ft)
                .with(field::HEIGHT_FT, now.height_ft);
        }
        self.orbit = status.map(|now| (number, now.point));
        events.push(event);
    }

    /// The city fill buildings knocked down since the last tick (nr-fx3,
    /// John 2026-10-13: replays show the flattened blocks and their fires).
    fn city_changes(&mut self, world: &terrain::Terrain, frame: &mut Frame) {
        if let Some(fill) = &world.city_fill {
            city_change(fill, &mut self.city, frame);
        }
    }

    /// The surface world of the tick (format 3): names for every surface unit
    /// this tick mentions, then the poses, the launcher and magazine changes
    /// and the debris pieces of the frame.
    fn surface_tracks(&mut self, tick: &Tick<'_>, frame: &mut Frame, lost: &[(u32, i32)]) {
        if !self.surface_watch.on {
            return;
        }
        let ledger = &tick.combat.state.ledger;
        let mentioned = tick
            .snapshot
            .projectiles
            .iter()
            .map(|p| p.owner)
            .chain(tick.outcomes.iter().map(|o| o.key.owner))
            .chain(tick.combat.surface.trace.iter().filter_map(trace_unit))
            .chain(tick.combat.surface.trace.iter().filter_map(trace_target))
            .chain(lost.iter().map(|(id, _)| *id))
            .chain(
                lost.iter()
                    .filter_map(|(id, _)| ledger.credit(*id).map(|kill| kill.owner)),
            );
        for info in self.surface_watch.register(tick, mentioned) {
            // Reasons and trees name a unit by its label, like an aircraft.
            self.infos.insert(
                info.id,
                replay::AircraftInfo {
                    id: info.id,
                    name: info.name.clone(),
                    label: info.label.clone(),
                    side: info.side,
                    ..replay::AircraftInfo::default()
                },
            );
            self.send(Message::Surface(Box::new(info)));
        }
        frame.surface = self.surface_watch.poses(tick.snapshot);
        frame.surface_stock = self.surface_watch.stock(tick);
        frame.debris_pieces = self.surface_watch.pieces(tick.snapshot);
    }

    /// The reason an aircraft's loss gives when a surface unit shot it down:
    /// "it was shot down by an SA-6".
    fn shot_down_by(&self, tick: &Tick<'_>, victim: u32) -> Option<String> {
        let name = tore_world::ai_wings::outcome::shot_down_by(
            &tick.world.surface,
            &tick.combat.state.ledger,
            victim,
        )?;
        Some(format!(
            "it was shot down by {} {name}",
            surface::article(name)
        ))
    }

    fn weapon_events(
        &mut self,
        tick: &Tick<'_>,
        frame: &Frame,
        shots: &mut BTreeMap<u32, Shot>,
        live_shots: &ById<'_, live::Projectile>,
        events: &mut Vec<Event>,
    ) {
        let state_of = |id: u32| frame.aircraft.iter().find(|a| a.id == id);
        // What a round with no target of its own was fired at: the AI's
        // current target, or the designated one of a human-flown plane.
        let intended = |owner: u32| {
            if let Some(view) = tick.combat.state.view(owner) {
                view.designated()
            } else {
                tick.wings
                    .and_then(|w| w.mission().actor(owner))
                    .and_then(|a| a.controller().target())
            }
        };
        // Both maps run in id order, so the shot's row of the last tick is
        // found by walking along them together.
        let mut old = self.shots.iter().peekable();
        for (id, shot) in shots.iter_mut() {
            while old.next_if(|(before, _)| *before < id).is_some() {}
            let Some((_, previous)) = old.next_if(|(before, _)| *before == id) else {
                // A launch, with its geometry at release.
                let mut event = Event::new(kind::WEAPON_LAUNCH)
                    .with_subject(shot.owner)
                    .with(field::PROJECTILE, replay::Value::Id(*id))
                    .with(field::WEAPON, replay::Value::Id(shot.weapon));
                if let Some(weapon) = live_shots
                    .get(id)
                    .map(|p| p.weapon(stores_of(tick, self.player, p.owner)))
                {
                    event = event.with(
                        field::CLASS,
                        convert::weapon_info(shot.weapon, weapon).class.name(),
                    );
                }
                if let Some(energy) = live_shots
                    .get(id)
                    .and_then(|p| p.motion)
                    .and_then(|m| m.energy)
                {
                    let b = energy.body;
                    event = event
                        .with("missile-model", "energy-1")
                        .with(
                            "missile-data",
                            format!("{:016x}", tore_formats::missile_data::fingerprint()),
                        )
                        .with("motor-ignition", i64::from(b.ignition))
                        .with("motor-boost-end", i64::from(b.ignition + b.boost.ticks))
                        .with(
                            "motor-end",
                            i64::from(b.ignition + b.boost.ticks + b.sustain.ticks),
                        )
                        .with("motor-boost-powered", b.boost.thrust > 0.)
                        .with("motor-sustain-powered", b.sustain.thrust > 0.);
                }
                let mode = live_shots
                    .get(id)
                    .map_or("unguided", |p| match p.guidance.as_ref() {
                        Some(guidance)
                            if guidance.mode
                                == tore_sim::combat::missiles::LaunchMode::Boresight =>
                        {
                            "boresight"
                        }
                        Some(_) => "cued",
                        None if live::is_gun(p.weapon(stores_of(tick, self.player, p.owner))) => {
                            "gun"
                        }
                        None => "unguided",
                    });
                event = event.with(field::MODE, mode);
                let aim = shot.target.or_else(|| intended(shot.owner));
                if let Some(target) = aim {
                    event = event.with_object(target);
                }
                if let (Some(shooter), Some(target)) =
                    (state_of(shot.owner), aim.and_then(state_of))
                {
                    event = geometry(event, shooter, target);
                } else if let Some(shooter) = state_of(shot.owner) {
                    event = event
                        .with(field::SHOOTER_ALT_FT, shooter.position[1])
                        .with(field::SHOOTER_SPEED_KT, speed(shooter) * KT_PER_FPS);
                }
                // An air-to-ground weapon says it aims at the surface, and
                // how far the ground target was (a bomb or rocket aims at
                // nothing the picture holds: its release says which).
                if self
                    .weapon_notes
                    .get(&shot.weapon)
                    .is_some_and(|note| note.air_to_ground)
                {
                    event = event.with(field::SURFACE, true);
                    if event.get(field::RANGE_FT).is_none()
                        && let Some(row) = aim.and_then(|id| {
                            tick.combat.state.targets.iter().find(|row| {
                                row.id == id && !row.airborne && convert::is_surface_id(id)
                            })
                        })
                    {
                        event = event.with(field::RANGE_FT, distance(shot.position, row.position));
                    }
                }
                events.push(event);
                continue;
            };
            // The shot lost its target, once per shot: the target let go
            // while it flew on, or its seeker gave up.
            const LOST: u8 = tore_sim::combat::missiles::seeker::Status::Lost as u8;
            let let_go = previous.target.is_some() && shot.target.is_none();
            let seeker_lost = shot.status == Some(LOST) && previous.status != Some(LOST);
            if !shot.lost && (let_go || seeker_lost) {
                shot.lost = true;
                shot.lost_at = Some(frame.tick);
                let target = previous.target.or(shot.target);
                let mut event = Event::new(kind::WEAPON_TRACK_LOST)
                    .with_subject(shot.owner)
                    .with(field::PROJECTILE, replay::Value::Id(*id));
                if let Some(target) = target {
                    event = event.with_object(target);
                    if let Some(state) = state_of(target) {
                        event =
                            event.with(field::RANGE_FT, distance(shot.position, state.position));
                    }
                }
                if seeker_lost {
                    event = event.with("seeker", "lost");
                }
                let decoyed = self.why.decoyed(*id);
                let reason = if let Some(roll) = decoyed {
                    format!(
                        "decoyed by {} from {}",
                        why::decoy_label(roll),
                        self.who(roll.releaser)
                    )
                } else if seeker_lost {
                    "the seeker lost the target".to_owned()
                } else if target
                    .and_then(state_of)
                    .is_none_or(|state| !state.flags.alive)
                {
                    "its target was destroyed or is gone".to_owned()
                } else {
                    "it could not hold the target: seeker limits, terrain or guidance time"
                        .to_owned()
                };
                events.push(event.with(field::REASON, reason));
            }
        }
        // Seeker milestones combat reported this tick.
        for event in tick.events {
            let (kind, id) = match event {
                live::Event::SeekerActivated(id) => (kind::WEAPON_SEEKER_ACTIVE, *id),
                live::Event::Pitbull(id) => (kind::WEAPON_PITBULL, *id),
                _ => continue,
            };
            let Some(shot) = shots.get(&id) else {
                continue;
            };
            let mut out = Event::new(kind)
                .with_subject(shot.owner)
                .with(field::PROJECTILE, replay::Value::Id(id))
                .with(field::WEAPON, replay::Value::Id(shot.weapon));
            if let Some(target) = shot.target {
                out = out.with_object(target);
                if let Some(state) = state_of(target) {
                    out = out.with(field::RANGE_FT, distance(shot.position, state.position));
                }
            }
            events.push(out);
        }
        // How each shot ended.
        for resolved in tick.outcomes {
            events.push(self.outcome_event(resolved));
        }
        // Rounds that hit the ground: each ground impact effect this tick,
        // matched to the nearest weapon that vanished.
        let mut vanished: Vec<(u32, Shot)> = if frame
            .new_effects
            .iter()
            .any(|effect| is_ground_burst(effect.kind))
        {
            self.shots
                .iter()
                .filter(|(id, _)| !shots.contains_key(id))
                .map(|(id, shot)| (*id, *shot))
                .collect()
        } else {
            Vec::new()
        };
        for effect in frame.new_effects.iter().filter(|e| is_ground_burst(e.kind)) {
            let nearest = vanished
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    distance(a.1.1.position, effect.position)
                        .total_cmp(&distance(b.1.1.position, effect.position))
                })
                .map(|(i, _)| i);
            let [x, y, z] = effect.position;
            let mut event = Event::new(kind::COMBAT_GROUND_IMPACT)
                .with(field::X_FT, x)
                .with(field::Y_FT, y)
                .with(field::Z_FT, z);
            if let Some(i) = nearest {
                let (id, shot) = vanished.remove(i);
                event = event
                    .with_subject(shot.owner)
                    .with(field::PROJECTILE, replay::Value::Id(id))
                    .with(field::WEAPON, replay::Value::Id(shot.weapon));
            }
            events.push(event);
        }
    }

    /// The `weapon.outcome` event for one shot the ledger resolved. A late
    /// hit that replaces an earlier spoof says so, and why.
    fn outcome_event(&self, resolved: &ledger::Outcome) -> Event {
        let mut event = Event::new(kind::WEAPON_OUTCOME)
            .with_subject(resolved.key.owner)
            .with(field::PROJECTILE, replay::Value::Id(resolved.projectile))
            .with(field::RESULT, result_name(resolved.resolution));
        if let Some(target) = resolved.key.aim {
            event = event.with_object(target);
        }
        if let ledger::Resolution::Hit(damage) = resolved.resolution {
            event = event.with(field::DAMAGE, i64::from(damage));
        }
        let shot = self.shots.get(&resolved.projectile);
        if let Some(weapon) = shot.map(|shot| shot.weapon) {
            event = event.with(field::WEAPON, replay::Value::Id(weapon));
        }
        if let Some((miss, _)) = self.why.closest(resolved.projectile)
            && resolved.resolution == ledger::Resolution::Missed
        {
            event = event.with(field::MISS_FT, trees::round(miss, 1));
        }
        let reason = match resolved.resolution {
            ledger::Resolution::Spoofed => self.why.decoyed(resolved.projectile).map(|roll| {
                format!(
                    "decoyed by {} from {} ({})",
                    why::decoy_label(roll),
                    self.who(roll.releaser),
                    roll.draw.map_or_else(String::new, |d| trees::draw_text(&d))
                )
            }),
            ledger::Resolution::Jammed => {
                Some("the target's jammer or countermeasures deceived it".to_owned())
            }
            ledger::Resolution::Missed => shot
                .and_then(|shot| shot.lost_at)
                .map(|at| format!("it lost track of its target at {}", trees::clock(at))),
            // The late strike names the chaff or flare that fooled the missile,
            // from the roll kept since its own tick, and when the track was
            // lost.
            ledger::Resolution::Hit(_) => resolved.replaces.map(|_| {
                let roll = self
                    .why
                    .decoyed(resolved.projectile)
                    .or_else(|| self.why.decoyed_earlier(resolved.projectile));
                let decoy = roll.map_or_else(
                    || "it was spoofed".to_owned(),
                    |roll| {
                        format!(
                            "it was decoyed by {} from {}",
                            why::decoy_label(roll),
                            self.who(roll.releaser)
                        )
                    },
                );
                let when = shot
                    .and_then(|shot| shot.lost_at)
                    .map(|at| format!(" at {}", trees::clock(at)))
                    .unwrap_or_default();
                format!("{decoy}{when} but flew on and struck anyway")
            }),
        };
        // An air-to-ground round says how it ended when nothing above did:
        // a bomb that landed before its fuze armed, or a guided weapon and
        // what it was following.
        let reason = reason.or_else(|| self.ground_reason(resolved, shot));
        if let Some(replaced) = resolved.replaces {
            event = event.with(field::REPLACES, result_name(replaced));
        }
        if let Some(reason) = reason {
            event = event.with(field::REASON, reason);
        }
        event
    }

    /// The words for a bomb or guided weapon's outcome, if it has any: a
    /// dud says so, a guided weapon names what it followed.
    fn ground_reason(&self, resolved: &ledger::Outcome, shot: Option<&Shot>) -> Option<String> {
        let guided = self
            .ground
            .how_guided(resolved.projectile, |id| self.who(id));
        let dud = shot.zip(self.state_clock).is_some_and(|(shot, clock)| {
            resolved.resolution == ledger::Resolution::Missed
                && self
                    .weapon_notes
                    .get(&shot.weapon)
                    .is_some_and(|note| ground::is_dud(shot, note, clock))
        });
        match (dud, guided) {
            (true, Some(how)) => Some(format!("it landed before its fuze armed; {}", how.outcome)),
            (true, None) => Some("it landed before its fuze armed".to_owned()),
            (false, Some(how)) => Some(how.outcome),
            (false, None) => None,
        }
    }

    fn aircraft_events(&mut self, tick: &Tick<'_>, frame: &Frame, events: &mut Vec<Event>) {
        let ledger = &tick.combat.state.ledger;
        let hit_by = |victim: u32| -> Option<&ledger::Outcome> {
            let hits = tick
                .outcomes
                .iter()
                .filter(|o| matches!(o.resolution, ledger::Resolution::Hit(_)));
            if victim == self.player {
                hits.clone()
                    .find(|o| o.key.aim == Some(victim))
                    .or_else(|| hits.clone().next())
            } else {
                let owner = ledger.attacker(victim).map(|kill| kill.owner);
                hits.clone()
                    .find(|o| Some(o.key.owner) == owner && o.key.aim == Some(victim))
                    .or_else(|| hits.clone().find(|o| Some(o.key.owner) == owner))
            }
        };
        let subsystems: Vec<&str> = tick
            .events
            .iter()
            .filter_map(|e| match e {
                live::Event::SubsystemDamaged { aircraft, index } if *aircraft == self.player => {
                    Some(tore_sim::aircraft_systems::label(*index))
                }
                _ => None,
            })
            .collect();
        for state in &frame.aircraft {
            let id = state.id;
            let actor = tick.wings.and_then(|wings| wings.mission().actor(id));
            let flight = flight_of(self.player, tick, id);
            let mut watch = self.watches.remove(&id).unwrap_or_default();
            let first = !watch.seen;
            watch.seen = true;
            let speed_kt = speed(state) * KT_PER_FPS;
            let hp_before = if first { state.hp } else { watch.hp };

            // Damage landed.
            if !first && state.hp < watch.hp {
                let hit = hit_by(id);
                let attacker = if id == self.player {
                    hit.map(|o| o.key.owner)
                } else {
                    ledger
                        .attacker(id)
                        .map(|kill| kill.owner)
                        .or(hit.map(|o| o.key.owner))
                };
                let mut event = Event::new(kind::COMBAT_HIT)
                    .with_object(id)
                    .with(field::DAMAGE, i64::from(watch.hp - state.hp))
                    .with(field::HP_AFTER, i64::from(state.hp));
                if let Some(attacker) = attacker {
                    event = event.with_subject(attacker);
                }
                if let Some(hit) = hit {
                    event = event.with(field::PROJECTILE, replay::Value::Id(hit.projectile));
                    if let Some(weapon) = self.shots.get(&hit.projectile).map(|s| s.weapon) {
                        event = event.with(field::WEAPON, replay::Value::Id(weapon));
                    }
                }
                let section = (0..replay::SECTION_COUNT)
                    .filter(|i| state.sections[*i] > watch.sections[*i])
                    .max_by_key(|i| state.sections[*i] - watch.sections[*i]);
                if let Some(section) = section {
                    event = event.with(field::SECTION, section as i64);
                }
                if id == self.player && !subsystems.is_empty() {
                    event = event.with("subsystems", subsystems.join(", "));
                }
                events.push(event);
            }
            // Destroyed in combat.
            let destroyed = tick.events.iter().any(|e| match e {
                live::Event::Destroyed(victim) => *victim == id,
                live::Event::OwnshipDestroyed { aircraft } => *aircraft == id,
                _ => false,
            });
            if destroyed {
                let killer = if id == self.player {
                    hit_by(id).map(|o| o.key.owner)
                } else {
                    ledger.credit(id).map(|kill| kill.owner)
                };
                let mut event = Event::new(kind::COMBAT_DESTROYED).with_subject(id);
                if let Some(killer) = killer {
                    event = event.with_object(killer);
                }
                // Lost to overspeed or the map edge, not to a weapon.
                if let Some(cause) = flight.and_then(|f| f.systems.structure.cause) {
                    event = event.with(field::REASON, cause.label());
                } else if let Some(reason) = self.shot_down_by(tick, id) {
                    event = event.with(field::REASON, reason);
                }
                if let Some(hit) = hit_by(id) {
                    event = event.with(field::PROJECTILE, replay::Value::Id(hit.projectile));
                    if let Some(weapon) = self.shots.get(&hit.projectile).map(|s| s.weapon) {
                        event = event.with(field::WEAPON, replay::Value::Id(weapon));
                    }
                }
                events.push(event);
            }
            watch.hp = state.hp;
            watch.sections = state.sections;

            if let Some(f) = flight {
                // Departure mode, stalls and spins.
                let mode = f.maneuver.departure;
                if !first && mode != watch.departure {
                    let name = |m: Option<DepartureMode>| {
                        m.map_or("none".to_owned(), |m| format!("{m:?}").to_lowercase())
                    };
                    events.push(
                        Event::new(kind::FLIGHT_DEPARTURE)
                            .with_subject(id)
                            .with(field::FROM, name(watch.departure))
                            .with(field::TO, name(mode)),
                    );
                    for (kind, entered) in [
                        (kind::FLIGHT_STALL, DepartureMode::Stalled),
                        (kind::FLIGHT_SPIN, DepartureMode::Spinning),
                    ] {
                        let was = watch.departure == Some(entered);
                        let is = mode == Some(entered);
                        if was != is {
                            let mut event = Event::new(kind)
                                .with_subject(id)
                                .with(field::ON, is)
                                .with(field::SPEED_KT, speed_kt);
                            if kind == kind::FLIGHT_SPIN && is {
                                let yaw = f.maneuver.body_rates_rad_per_second[2];
                                event = event.with(
                                    field::DIRECTION,
                                    if yaw >= 0. { "right" } else { "left" },
                                );
                            }
                            events.push(event);
                        }
                    }
                }
                watch.departure = mode;
                let alive = !f.crashed && f.escape.is_none() && !f.systems.pilot.dead;
                // Engine and fuel.
                let engine = f.engine && f.fuel > 0.;
                if !first && watch.engine && !engine && alive && state.hp > 0 {
                    let commanded = id == self.player
                        && tick.pilot.commands.iter().any(|c| {
                            matches!(
                                c,
                                flight::PilotCommand::Toggle(flight::Switch::Engine)
                                    | flight::PilotCommand::Set(flight::Switch::Engine, false)
                            )
                        });
                    let mut event = Event::new(kind::AIRCRAFT_FLAMEOUT).with_subject(id);
                    if f.fuel <= 0. {
                        event = event.with(field::REASON, "its fuel ran out");
                    } else if commanded {
                        event = event.with(field::REASON, "the pilot switched it off");
                    }
                    events.push(event);
                }
                watch.engine = engine;
                let fuel_out = f.fuel <= 0.;
                if !first && fuel_out && !watch.fuel_out {
                    events.push(Event::new(kind::AIRCRAFT_FUEL_OUT).with_subject(id));
                }
                watch.fuel_out = fuel_out;
                // Wheels on the ground.
                let on_ground = state.flags.on_ground;
                if !first && on_ground != watch.on_ground && alive {
                    events.push(
                        Event::new(if on_ground {
                            kind::AIRCRAFT_LANDED
                        } else {
                            kind::AIRCRAFT_TOOK_OFF
                        })
                        .with_subject(id)
                        .with(field::SINK_FPS, -state.velocity[1]),
                    );
                }
                watch.on_ground = on_ground;
                // Ejection and the pilot.
                let escape = f.escape.as_ref().map(|e| e.phase);
                if !first && watch.escape.is_none() && escape.is_some() {
                    let mut event = Event::new(kind::AIRCRAFT_EJECTED).with_subject(id);
                    let hazard = actor
                        .and_then(|a| a.trace().ejection)
                        .and_then(|e| e.assessment);
                    if let Some(hazard) = hazard {
                        event = event.with(field::REASON, trees::hazard_text(&hazard));
                    } else if id == self.player
                        && tick.pilot.commands.contains(&flight::PilotCommand::Eject)
                    {
                        event = event.with(field::REASON, "you pulled the ejection handle");
                    }
                    events.push(event);
                }
                watch.escape = escape;
                let dead = f.systems.pilot.dead;
                if !first && dead && !watch.pilot_dead {
                    let mut event = Event::new(kind::AIRCRAFT_PILOT_KILLED).with_subject(id);
                    if let Some(killer) = ledger.credit(id).map(|k| k.owner) {
                        event = event.with_object(killer);
                    }
                    events.push(event);
                }
                watch.pilot_dead = dead;
                // The end of the aircraft: flying into the ground or a
                // structure while still flyable, or the wreck of a destroyed
                // one coming down or exploding.
                if !first
                    && f.crashed
                    && !watch.crashed
                    && hp_before > 0
                    && !destroyed
                    && !watch.ended
                {
                    watch.ended = true;
                    events.push(
                        Event::new(kind::AIRCRAFT_CRASHED)
                            .with_subject(id)
                            .with(field::SPEED_KT, speed_kt)
                            .with(
                                field::REASON,
                                f.systems
                                    .structure
                                    .cause
                                    .map_or("it hit the ground or a structure", |c| c.label()),
                            ),
                    );
                }
                watch.crashed = f.crashed;
            }
            let wreck = convert::wreck_phase(state.wreck_phase);
            if !first && wreck != watch.wreck && !watch.ended {
                let reason = match wreck {
                    Some(tore_sim::wreck::Phase::Grounded) => Some("the wreck hit the ground"),
                    Some(tore_sim::wreck::Phase::Exploded) => Some("the wreck exploded in the air"),
                    _ => None,
                };
                if let Some(reason) = reason {
                    watch.ended = true;
                    events.push(
                        Event::new(kind::AIRCRAFT_CRASHED)
                            .with_subject(id)
                            .with(field::SPEED_KT, speed_kt)
                            .with(field::REASON, reason),
                    );
                }
            }
            watch.wreck = wreck;

            self.watches.insert(id, watch);
        }
    }

    /// Cockpit cues the player hears: seeker tone, stall warning, device
    /// sounds and the ejection seat's warnings and effects.
    fn cue_events(&mut self, tick: &Tick<'_>, player_ground: f64, events: &mut Vec<Event>) {
        let flight = tick.flight;
        let player = self.player;
        let tone = tick
            .combat
            .state
            .view(player)
            .and_then(|view| view.seeker_tone(combat::launcher(flight)))
            .map(|t| {
                let tone = Tone {
                    radar: t.radar,
                    ground: t.ground,
                    locked: t.locked,
                };
                // The loudness the mixer takes.
                (tone, t.strength.clamp(0., 1.))
            });
        if let Some(event) = tone_event(player, self.tone, tone) {
            events.push(event);
            self.tone = tone;
        }
        let stall = crate::audio::warning_cue(flight, player_ground);
        if stall != self.stall {
            let mut event = Event::new(kind::AUDIO_STALL_WARNING)
                .with_subject(player)
                .with(field::ON, stall.is_some());
            if let Some(sound) = stall.or(self.stall) {
                event = event.with(field::SOUND, sound);
            }
            events.push(event);
            self.stall = stall;
        }
        let names = ["gear", "flaps", "hook", "brake"];
        for (device, sound) in names
            .into_iter()
            .zip(crate::audio::actuator_cues(tick.previous, flight))
        {
            if let Some(sound) = sound {
                events.push(
                    Event::new(kind::AUDIO_DEVICE)
                        .with_subject(player)
                        .with(field::DEVICE, device)
                        .with(field::SOUND, sound),
                );
            }
        }
        let danger = tore_sim::ejection::assess(flight, |x, z| {
            f64::from(tick.world.height(x as f32, z as f32))
        })
        .is_some();
        let ejection = |sound: &str, text: &str| {
            Event::new(kind::AUDIO_EJECTION)
                .with_subject(player)
                .with(field::SOUND, sound)
                .with_text(text)
        };
        if danger && !self.danger && flight.escape.is_none() && !flight.systems.pilot.dead {
            events.push(ejection("^EJECTX3.5K", "eject warning"));
        }
        self.danger = danger;
        if tick.previous.escape.is_none() && flight.escape.is_some() {
            events.push(ejection("^EJECTNG.5K", "ejecting"));
            events.push(ejection("&EJECT.5K", "ejection seat"));
        }
        let inflating = |f: &flight::State| {
            f.escape
                .as_ref()
                .is_some_and(|p| p.phase == tore_sim::ejection::Phase::Inflating)
        };
        if inflating(flight) && !inflating(tick.previous) {
            events.push(ejection("&CHUTE.5K", "parachute opening"));
        }
    }

    /// Writes the footer and seek index, renames the file to its final name
    /// and returns that path. Waits for the writer to drain its queue.
    pub fn finish(mut self, footer: &replay::Footer) -> Option<PathBuf> {
        if let Some(frame) = self.frame.take() {
            self.held = Some(frame);
        }
        let last = self.held.take().map(|mut frame| {
            self.mark_gap(&mut frame);
            fit(&mut frame);
            frame
        });
        let sender = self.sender.take()?;
        // Everything left waits its turn now; the flight is over.
        for message in std::mem::take(&mut self.backlog) {
            let _ = sender.send(message);
        }
        if let Some(frame) = last {
            let _ = sender.send(Message::Frame(Box::new(frame)));
        }
        let _ = sender.send(Message::Finish(Box::new(footer.clone())));
        drop(sender);
        let result = self.thread.take()?.join();
        match result {
            Ok(Ok(path)) => {
                log::info!(
                    "Recording saved: {} ({} frames)",
                    path.display(),
                    self.frames
                );
                Some(path)
            }
            Ok(Err(error)) => {
                log::info!("Recording not finished: {error}");
                None
            }
            Err(_) => {
                log::info!("Recording writer stopped unexpectedly");
                None
            }
        }
    }
}

impl Drop for Recorder {
    /// A recorder dropped without `finish` (a panic, for one) stops the
    /// writer, which keeps its `.partial` file.
    fn drop(&mut self) {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Keeps a frame within the format's limits, so the writer never refuses a
/// whole tick: over-long lists keep their first items, text is cut to the
/// longest string the format stores, and a `system.note` says what was left
/// out. Live flight stays far inside the limits; this is insurance.
use tore_session::recording::bounds::fit;

/// The surface unit a controller trace line is about.
fn trace_unit(trace: &tore_world::surface::fire::Trace) -> Option<u32> {
    use tore_world::surface::fire::Trace;
    match trace {
        Trace::Phase { unit, .. }
        | Trace::Shot { unit, .. }
        | Trace::Swap { unit, .. }
        | Trace::Rearm { unit }
        | Trace::Refill { unit, .. }
        | Trace::Radar { unit, .. }
        | Trace::Shutdown { unit, .. } => Some(unit.0),
        Trace::Battery { .. } | Trace::FlakTeam { .. } => None,
    }
}

/// The ground unit or ship a surface controller engages or fires at, when
/// its target is one (surface units fight each other since 2026-10-11): the
/// registry names it like the unit that fires.
fn trace_target(trace: &tore_world::surface::fire::Trace) -> Option<u32> {
    use tore_world::surface::fire::{Trace, is_surface_id};
    match trace {
        Trace::Phase {
            target: Some(target),
            ..
        }
        | Trace::Shot { target, .. } => Some(*target).filter(|id| is_surface_id(*id)),
        _ => None,
    }
}

/// Each on-off cheat's name and state before and after.
fn cheat_changes(
    before: &tore_sim::cheats::Cheats,
    after: &tore_sim::cheats::Cheats,
) -> [(&'static str, bool, bool); 17] {
    type Switch = fn(&tore_sim::cheats::Cheats) -> bool;
    let list: [(&'static str, Switch); 17] = [
        ("invulnerable", |c| c.invulnerable()),
        ("realistic_damage", |c| c.system_damage()),
        ("unlimited_ammo", |c| c.unlimited_ammo),
        ("unlimited_fuel", |c| c.unlimited_fuel),
        ("no_spins", |c| c.no_spins),
        ("no_turbulence", |c| c.no_turbulence),
        ("extra_g", |c| c.extra_g),
        ("ignore_weapon_weights", |c| c.ignore_weapon_weights),
        ("no_sun_whiteout", |c| c.no_sun_whiteout),
        ("no_g_effects", |c| c.no_g_effects),
        ("no_screen_shake", |c| c.no_screen_shake),
        ("no_crashes", |c| c.no_crashes),
        ("easy_aiming", |c| c.easy_aiming),
        ("ignore_midair_collisions", |c| c.ignore_midair_collisions),
        ("easy_targeting", |c| c.easy_targeting),
        ("guns_only", |c| c.guns_only),
        ("easy_physics", |c| c.easy_physics),
    ];
    list.map(|(name, switch)| (name, switch(before), switch(after)))
}

/// The cheats switched on, for a recording's header.
/// What the city fill's knocked-down buildings did since `last` (its
/// version and flat buildings the recording last wrote), put on `frame` by
/// where each stands: only what changed, read when the fill's version moved.
/// Fewer flat than before means the city stood again (a restart): the frame
/// says so and lists every building flat now.
pub(crate) use tore_session::recording::city_change;

pub fn cheats_on(cheats: &tore_sim::cheats::Cheats) -> Vec<String> {
    let mut on: Vec<String> = cheat_changes(cheats, cheats)
        .into_iter()
        .filter(|(_, was, _)| *was)
        .map(|(name, _, _)| name.to_owned())
        .collect();
    if let Some(level) = cheats.enemy_ai {
        on.push(format!("enemy_ai={level:?}"));
    }
    on
}

fn ground_height(world: &terrain::Terrain, position: [f64; 3]) -> f64 {
    world.surface(position[0], position[2]).height
}

/// Whether a recorded effect is a weapon striking the ground: the plain
/// ground effect of recordings before explosion types, or a blast on the
/// ground (what every ground burst is now, and what `combat.ground_impact`
/// is written for once each).
fn is_ground_burst(kind: replay::EffectKind) -> bool {
    matches!(
        kind,
        replay::EffectKind::Ground
            | replay::EffectKind::Blast {
                on: replay::Strike::Ground,
                ..
            }
    )
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// A ledger resolution as a recorded `result`.
fn result_name(resolution: ledger::Resolution) -> &'static str {
    match resolution {
        ledger::Resolution::Hit(_) => outcome::HIT,
        ledger::Resolution::Missed => outcome::MISSED,
        ledger::Resolution::Spoofed => outcome::SPOOFED,
        ledger::Resolution::Jammed => outcome::JAMMED,
    }
}

/// An aircraft's registered label, or `aircraft 7` for one never registered.
fn who(infos: &BTreeMap<u32, replay::AircraftInfo>, id: u32) -> String {
    match infos.get(&id) {
        Some(info) if !info.label.is_empty() => info.label.clone(),
        Some(info) if !info.name.is_empty() => format!("{} {id}", info.name),
        _ => format!("aircraft {id}"),
    }
}

/// Airspeed as recorded, or the ground speed when none was.
fn speed(state: &replay::AircraftState) -> f64 {
    if state.airspeed > 0. {
        state.airspeed
    } else {
        state.ground_speed()
    }
}

/// Launch geometry between a shooter and its target.
fn geometry(
    event: Event,
    shooter: &replay::AircraftState,
    target: &replay::AircraftState,
) -> Event {
    let line: [f64; 3] = std::array::from_fn(|i| target.position[i] - shooter.position[i]);
    let range = distance(target.position, shooter.position);
    let angle = |a: [f64; 3], b: [f64; 3]| {
        let dot: f64 = (0..3).map(|i| a[i] * b[i]).sum();
        (dot / range.max(1e-9)).clamp(-1., 1.).acos().to_degrees()
    };
    // Aspect: 0 degrees is dead astern of the target, 180 head on.
    let aspect = angle(target.forward(), line);
    let off_boresight = angle(shooter.forward(), line);
    let relative: [f64; 3] = std::array::from_fn(|i| target.velocity[i] - shooter.velocity[i]);
    let closure = -(0..3).map(|i| relative[i] * line[i]).sum::<f64>() / range.max(1e-9);
    event
        .with(field::RANGE_FT, range)
        .with(field::ASPECT_DEG, aspect)
        .with(field::OFF_BORESIGHT_DEG, off_boresight)
        .with(field::CLOSURE_KT, closure * KT_PER_FPS)
        .with(field::SHOOTER_ALT_FT, shooter.position[1])
        .with(field::TARGET_ALT_FT, target.position[1])
        .with(field::SHOOTER_SPEED_KT, speed(shooter) * KT_PER_FPS)
        .with(field::TARGET_SPEED_KT, speed(target) * KT_PER_FPS)
}

/// The aircraft a recording draws, the player first: `player`'s pose, then
/// every other aircraft of the snapshot in its order. The snapshot's own player
/// pose is one of them when it is not `player`.
pub(super) fn drawn_aircraft(
    snapshot: &RenderSnapshot,
    player: u32,
) -> Vec<&crate::snapshot::AircraftPose> {
    let own = if snapshot.player.id == player {
        Some(&snapshot.player)
    } else {
        snapshot.target(player)
    };
    // A player the snapshot does not draw falls back to its own player pose.
    let first = own.unwrap_or(&snapshot.player);
    std::iter::once(first)
        .chain(
            std::iter::once(&snapshot.player)
                .chain(&snapshot.targets)
                .filter(|p| p.id != first.id && p.aircraft.is_some()),
        )
        .collect()
}

/// The flight state of aircraft `id`: the player's, another human-flown
/// plane's or an AI aircraft's.
pub(super) fn flight_of<'a>(player: u32, tick: &Tick<'a>, id: u32) -> Option<&'a flight::State> {
    if id == player {
        Some(tick.flight)
    } else if let Some(other) = tick.others.iter().find(|o| o.plane == id) {
        Some(other.flight)
    } else {
        tick.wings
            .and_then(|wings| wings.mission().actor(id))
            .map(|actor| actor.flight())
    }
}

/// The stores that decide what a projectile fired by `owner` is: its own
/// ownship's, or the player's for any other owner (an AI aircraft's round
/// carries its weapon record already in the shape the recorder reads).
fn stores_of<'a>(tick: &Tick<'a>, player: u32, owner: u32) -> &'a live::Configuration {
    let state = &tick.combat.state;
    state
        .ownship(owner)
        .or_else(|| state.ownship(player))
        .unwrap_or_else(|| state.own())
        .configuration()
}

/// Flight data for one drawn aircraft: the player's own state, an AI
/// aircraft's, or what a straight-flight fixture's combat target holds.
fn flight_data(
    player: u32,
    pose: &crate::snapshot::AircraftPose,
    tick: &Tick<'_>,
    targets: &ById<'_, live::Target>,
    player_ground: f64,
) -> FlightData {
    let controls = |f: &flight::State, input: &flight::PilotInput| {
        [input.pitch, input.roll, input.yaw, f.throttle]
    };
    if pose.id == player {
        let f = tick.flight;
        return FlightData {
            airspeed: f.speed,
            g: f.g,
            fuel_lb: f.fuel + f.systems.external_lbs(),
            controls: controls(f, tick.pilot),
            on_ground: f.supported_at(player_ground),
            alive: !f.crashed
                && tick
                    .combat
                    .state
                    .ownship(player)
                    .is_some_and(|own| own.hp > 0)
                && !f.systems.pilot.dead
                && f.escape.is_none(),
            ejected: f.escape.is_some(),
            wreck_gone: f.wreck_gone(),
        };
    }
    let target = targets.get(&pose.id);
    // Another human-flown plane: its own cockpit, as an AI aircraft's actor
    // would give it.
    if let Some(other) = tick.others.iter().find(|o| o.plane == pose.id) {
        let f = other.flight;
        return FlightData {
            airspeed: f.speed,
            g: f.g,
            fuel_lb: f.fuel + f.systems.external_lbs(),
            controls: controls(f, other.pilot),
            on_ground: f.supported_at(ground_height(tick.world, f.position)),
            alive: !f.crashed && pose.damage.hp > 0 && !f.systems.pilot.dead && f.escape.is_none(),
            ejected: f.escape.is_some(),
            wreck_gone: f.wreck_gone() || (!pose.airborne && pose.damage.hp <= 0),
        };
    }
    if let Some(actor) = tick.wings.and_then(|w| w.mission().actor(pose.id)) {
        let f = actor.flight();
        return FlightData {
            airspeed: f.speed,
            g: f.g,
            fuel_lb: f.fuel + f.systems.external_lbs(),
            controls: controls(f, actor.last_input()),
            on_ground: target.is_some_and(|t| t.on_ground),
            alive: actor.alive() && f.escape.is_none(),
            ejected: f.escape.is_some(),
            wreck_gone: f.wreck_gone() || target.is_some_and(|t| !t.airborne && t.hp <= 0),
        };
    }
    let [x, y, z] = pose.velocity;
    FlightData {
        airspeed: (x * x + y * y + z * z).sqrt(),
        g: 1.,
        fuel_lb: 0.,
        controls: [0.; 4],
        on_ground: target.is_some_and(|t| t.on_ground),
        alive: pose.damage.hp > 0,
        ejected: false,
        wreck_gone: !pose.airborne && pose.damage.hp <= 0,
    }
}

/// Smoke and contrail puffs released this tick: the newest puffs, which
/// have not aged yet.
fn new_puffs(combat: &combat::Combat, out: &mut Vec<replay::PuffSpawn>) {
    use tore_sim::combat::smoke::Kind;
    out.clear();
    for (layer, smoke) in [
        (replay::LAYER_SMOKE, &combat.state.smoke),
        (replay::LAYER_CONTRAILS, &combat.contrails),
    ] {
        let start = out.len();
        for puff in smoke.puffs.iter().rev().take_while(|p| p.age == 0) {
            out.push(replay::PuffSpawn {
                layer,
                kind: match puff.kind {
                    Kind::Missile => replay::PuffKind::Missile,
                    Kind::Aircraft => replay::PuffKind::Aircraft,
                    Kind::Contrail => replay::PuffKind::Contrail,
                    Kind::Burning => replay::PuffKind::Burning,
                },
                position: puff.position,
            });
        }
        // Release order, oldest first.
        out[start..].reverse();
    }
    out.truncate(tore_replay::limits::MAX_PUFFS_PER_TICK);
}

/// An identity for an aircraft that appeared after the recording started.
fn default_info(
    pose: &crate::snapshot::AircraftPose,
    wings: Option<&AiWings>,
) -> replay::AircraftInfo {
    let slot = wings.and_then(|w| w.slot(pose.id));
    replay::AircraftInfo {
        id: pose.id,
        pt: pose
            .aircraft
            .map(|a| convert::identity_key(a).to_owned())
            .unwrap_or_default(),
        name: pose
            .aircraft
            .map(|a| a.label().to_owned())
            .unwrap_or_default(),
        label: slot.map_or_else(|| format!("Aircraft {}", pose.id), |s| s.label()),
        side: slot.map_or(replay::Side::Unknown, |s| side(s.side)),
        wing: slot.map_or(0, |s| u16::from(s.wing_number)),
        member: slot.map_or(0, |s| u16::from(s.member_number)),
        skill: String::new(),
        human: false,
    }
}

fn side(side: tore_sim::ai::launch::Side) -> replay::Side {
    match side {
        tore_sim::ai::launch::Side::Friendly => replay::Side::Friendly,
        tore_sim::ai::launch::Side::Enemy => replay::Side::Enemy,
    }
}

/// An aircraft type's name in a recording. The imported PT name, except that
/// the F-22N and the F/A-XX borrow the F-22's PT name, so they take their own
/// exact label and are never shown as another aircraft.
fn type_name(
    id: tore_formats::aircraft::AircraftId,
    models: &[std::sync::Arc<crate::aircraft_type::AircraftType>],
    loaded: Option<&str>,
) -> String {
    use tore_formats::aircraft::AircraftId;
    if matches!(id, AircraftId::F22n | AircraftId::Faxx) {
        return id.label().to_owned();
    }
    loaded
        .map(str::to_owned)
        .or_else(|| {
            models
                .iter()
                .find(|m| m.profile.id == id)
                .map(|m| m.profile.name.clone())
        })
        .unwrap_or_else(|| id.label().to_owned())
}

/// A human pilot as the recording's roster registers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Human {
    /// The plane's id.
    pub id: u32,
    /// `You` for the recording's player.
    pub label: String,
    /// The aircraft's display name.
    pub name: String,
    pub side: replay::Side,
    /// Wing and place in it, from 1; 0 when the flight has no wings.
    pub wing: u16,
    pub member: u16,
}

impl Human {
    /// Single player: plane 0, `You`, the first friendly aircraft of the first
    /// friendly wing when the flight is a mission.
    pub fn single_player(name: &str, mission: bool) -> Self {
        Self {
            id: 0,
            label: "You".into(),
            name: name.into(),
            side: replay::Side::Friendly,
            wing: u16::from(mission),
            member: u16::from(mission),
        }
    }

    fn info(&self, pt: String) -> replay::AircraftInfo {
        replay::AircraftInfo {
            id: self.id,
            pt,
            name: self.name.clone(),
            label: self.label.clone(),
            side: self.side,
            wing: self.wing,
            member: self.member,
            skill: "Human".into(),
            human: true,
        }
    }
}

/// Every aircraft at the start of a flight: the recording's player, then each
/// other aircraft the snapshot draws, named as the setup screen names them.
/// `others` are the other human pilots.
pub fn roster(
    snapshot: &RenderSnapshot,
    player: &Human,
    others: &[Human],
    wings: Option<&AiWings>,
    models: &[std::sync::Arc<crate::aircraft_type::AircraftType>],
) -> Vec<replay::AircraftInfo> {
    let name_of = |id: tore_formats::aircraft::AircraftId| type_name(id, models, None);
    let poses = drawn_aircraft(snapshot, player.id);
    let identity = |pose: &crate::snapshot::AircraftPose| {
        pose.aircraft
            .map(|a| convert::identity_key(a).to_owned())
            .unwrap_or_default()
    };
    // The recorded pilot's aircraft by its exact name: the F-22N and the
    // F/A-XX never show as the F-22 whose PT name they borrow.
    let recorded = Human {
        name: poses[0].aircraft.map_or_else(
            || player.name.clone(),
            |id| type_name(id, models, Some(&player.name)),
        ),
        ..player.clone()
    };
    let mut out = vec![recorded.info(identity(poses[0]))];
    for pose in &poses[1..] {
        let mut info = default_info(pose, wings);
        if let Some(id) = pose.aircraft {
            info.name = name_of(id);
        }
        if let Some(human) = others.iter().find(|h| h.id == pose.id) {
            info = Human {
                name: info.name.clone(),
                ..human.clone()
            }
            .info(info.pt.clone());
        } else if let Some(actor) = wings.and_then(|w| w.mission().actor(pose.id)) {
            info.skill = format!("{:?}", actor.experience().level);
        }
        out.push(info);
    }
    out
}

/// A recording's header. `extra` holds the flight's settings, in order.
pub fn header(
    mission: replay::MissionKind,
    world: &terrain::Terrain,
    presentation: &Presentation,
    mut extra: Vec<(String, String)>,
    recorded_at: std::time::SystemTime,
) -> replay::Header {
    extra.extend(presentation.extras());
    extra.push(("platform".into(), crate::version::target().into()));
    // Samples count the external tanks' fuel too; older recordings do not.
    extra.push((
        replay::model::FUEL_KEY.into(),
        replay::model::FUEL_WITH_EXTERNAL.into(),
    ));
    replay::Header {
        format_version: replay::FORMAT_VERSION,
        game_version: crate::version::version().into(),
        game_commit: crate::version::commit().into(),
        recorded_at: super::library::utc_text(recorded_at),
        mission,
        world: crate::replay::identity::of(world),
        extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat_view::render_hash_tests as fixture;
    use std::sync::mpsc::Receiver;

    #[test]
    fn a_ground_unit_a_surface_unit_engages_or_fires_at_is_named() {
        use tore_sim::ai::surface::Phase;
        use tore_world::surface::{SURFACE_UNIT_BASE, UnitId, fire::Trace};
        let unit = UnitId(SURFACE_UNIT_BASE);
        let ground = SURFACE_UNIT_BASE + 5;
        let phase = |target| Trace::Phase {
            unit,
            weapon: 0,
            phase: Phase::Track,
            target,
        };
        let shot = |target| Trace::Shot {
            unit,
            mount: 0,
            record: "T72.JT".into(),
            target,
            rounds: 1,
            refused: 0,
            opening: false,
            flak: false,
        };
        assert_eq!(trace_target(&phase(Some(ground))), Some(ground));
        assert_eq!(trace_target(&shot(ground)), Some(ground));
        // An aircraft target, or none, joins the registry as an aircraft.
        assert_eq!(trace_target(&phase(Some(3))), None);
        assert_eq!(trace_target(&phase(None)), None);
        assert_eq!(trace_target(&shot(3)), None);
    }

    #[test]
    fn a_decoyed_missile_that_strikes_anyway_is_recorded_as_a_hit_replacing_the_spoof() {
        let roster = [replay::AircraftInfo {
            id: 3,
            label: "Enemy 1-1".into(),
            ..Default::default()
        }];
        let (mut recorder, _receiver) = Recorder::detached(64, &roster);
        let key = ledger::Key {
            owner: 3,
            aim: Some(0),
            kind: ledger::ShotKind::AirToAir,
        };
        let spoofed = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Spoofed,
            replaces: None,
        });
        assert_eq!(spoofed.string(field::RESULT), Some(outcome::SPOOFED));
        assert_eq!(spoofed.get(field::REPLACES), None);
        let hit = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Hit(116),
            replaces: Some(ledger::Resolution::Spoofed),
        });
        assert_eq!(hit.kind, kind::WEAPON_OUTCOME);
        assert_eq!(hit.subject, Some(3));
        assert_eq!(hit.object, Some(0));
        assert_eq!(hit.id(field::PROJECTILE), Some(1 << 24));
        assert_eq!(hit.string(field::RESULT), Some(outcome::HIT));
        assert_eq!(hit.num(field::DAMAGE), Some(116.));
        assert_eq!(hit.string(field::REPLACES), Some(outcome::SPOOFED));
        assert_eq!(
            hit.string(field::REASON),
            Some("it was spoofed but flew on and struck anyway")
        );
        // A shot the recorder followed names when its track was lost.
        recorder.shots.insert(
            1 << 24,
            Shot {
                owner: 3,
                weapon: 2,
                target: None,
                status: None,
                position: [0.; 3],
                lost: true,
                lost_at: Some(1_621),
                ..Shot::default()
            },
        );
        let named = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Hit(116),
            replaces: Some(ledger::Resolution::Spoofed),
        });
        assert_eq!(named.id(field::WEAPON), Some(2));
        assert_eq!(
            named.string(field::REASON),
            Some("it was spoofed at 0:13.5 but flew on and struck anyway")
        );
        // A roll made on an earlier tick names the chaff or flare that fooled
        // the missile: its kind, its number and who released it.
        let roll = |device| crate::ai_wings::DecoyRoll {
            projectile: 1 << 24,
            releaser: 3,
            class: tore_sim::ai::threat::SeekerClass::Radar,
            device,
            susceptibility: 70,
            effectiveness: 60,
            draw: None,
            decoyed: true,
        };
        recorder.keep_roll(roll(7));
        recorder.why.next_tick();
        let fooled = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Hit(116),
            replaces: Some(ledger::Resolution::Spoofed),
        });
        assert_eq!(
            fooled.string(field::REASON),
            Some(
                "it was decoyed by chaff #7 from Enemy 1-1 at 0:13.5 but flew on and struck anyway"
            )
        );
        // The `weapon.decoyed` entry carries the device's number too, and
        // omits it for a roll that has none.
        assert_eq!(
            why::decoyed_event(&roll(7), Some(3)).num(field::NUMBER),
            Some(7.)
        );
        assert_eq!(
            why::decoyed_event(&roll(0), Some(3)).get(field::NUMBER),
            None
        );
        // A spoofed outcome quotes the draw of its own tick's roll only.
        let spoof = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Spoofed,
            replaces: None,
        });
        assert_eq!(
            spoof.string(field::REASON),
            None,
            "this tick's roll is gone"
        );
        // A roll recorded before devices were numbered names the kind alone.
        recorder.keep_roll(roll(0));
        let old = recorder.outcome_event(&ledger::Outcome {
            projectile: 1 << 24,
            key,
            resolution: ledger::Resolution::Hit(116),
            replaces: Some(ledger::Resolution::Spoofed),
        });
        assert_eq!(
            old.string(field::REASON),
            Some("it was decoyed by chaff from Enemy 1-1 at 0:13.5 but flew on and struck anyway")
        );
        // An ordinary hit names nothing it replaces and gives no reason.
        let plain = recorder.outcome_event(&ledger::Outcome {
            projectile: 2,
            key,
            resolution: ledger::Resolution::Hit(40),
            replaces: None,
        });
        assert_eq!(plain.get(field::REPLACES), None);
        assert_eq!(plain.get(field::REASON), None);
    }

    #[test]
    fn the_f22n_and_faxx_are_never_named_as_the_f22() {
        use tore_formats::aircraft::AircraftId;
        assert_eq!(type_name(AircraftId::Faxx, &[], Some("F-22")), "F/A-XX");
        assert_eq!(
            type_name(AircraftId::F22n, &[], Some("F-22")),
            "F-22N Raptor"
        );
        assert_eq!(type_name(AircraftId::F22, &[], Some("F-22")), "F-22");
        assert_eq!(type_name(AircraftId::F18, &[], None), "F/A-18D Hornet");
    }

    /// Frames waiting in the queue, oldest first; registrations are skipped.
    fn frames(receiver: &Receiver<Message>) -> Vec<Frame> {
        receiver
            .try_iter()
            .filter_map(|message| match message {
                Message::Frame(frame) => Some(*frame),
                _ => None,
            })
            .collect()
    }

    /// Records `snapshot` as tick `number`, the way the tick loop does.
    fn tick(
        recorder: &mut Recorder,
        combat: &mut combat::Combat,
        snapshot: &RenderSnapshot,
        number: u64,
        ui: &mut flight_ui::FlightUi,
    ) {
        let flight = fixture::player();
        let world = tore_world::test_support::terrain();
        let mut snapshot = snapshot.clone();
        snapshot.tick = number;
        recorder.start_tick(Some(ui), combat);
        recorder.begin(Tick {
            snapshot: &snapshot,
            combat,
            flight: &flight,
            previous: &flight,
            pilot: &flight::PilotInput::default(),
            others: &[],
            wings: None,
            world: &world,
            events: &[],
            outcomes: &[],
            journal: None,
        });
        recorder.end(Some(ui), combat);
    }

    #[test]
    fn an_oversized_frame_is_trimmed_to_fit_rather_than_refused() {
        use tore_replay::limits::{MAX_EVENTS_PER_TICK, MAX_PROJECTILES, MAX_STRING_BYTES};
        let mut frame = Frame {
            tick: 5,
            projectiles: (0..MAX_PROJECTILES as u32 + 76)
                .map(|id| replay::ProjectileState {
                    id,
                    direction: [0., 0., 1.],
                    ..Default::default()
                })
                .collect(),
            events: (0..1100)
                .map(|_| Event::new(kind::COMMS_HUD).with_text("é".repeat(700)))
                .collect(),
            ..Frame::default()
        };
        fit(&mut frame);
        assert_eq!(frame.projectiles.len(), MAX_PROJECTILES);
        assert_eq!(frame.events.len(), MAX_EVENTS_PER_TICK);
        assert!(
            frame
                .events
                .iter()
                .all(|e| e.text.len() <= MAX_STRING_BYTES)
        );
        let note = frame.events.last().unwrap();
        assert_eq!(note.kind, kind::SYSTEM_NOTE);
        assert!(note.text.contains("76 projectiles") && note.text.contains("77 events"));
        // The writer takes it.
        let dir = crate::replay::tests::TempDir::new("fit");
        let mut writer = replay::Writer::create(
            dir.path().join("fit.tore-replay"),
            &replay::Header::default(),
        )
        .unwrap();
        writer.push(&frame).unwrap();
    }

    #[test]
    fn written_comms_and_audio_entries_are_handed_to_live_flight() {
        let (mut recorder, _receiver) = Recorder::detached(64, &[]);
        recorder.note(Event::new(kind::COMMS_RADIO).with_text("Fox two"));
        recorder.note(Event::new(kind::WEAPON_LAUNCH));
        recorder.note(Event::new(kind::AUDIO_TONE));
        recorder.release_held();
        assert!(recorder.take_comms().is_empty());
        recorder.held = Some(Frame {
            tick: 7,
            events: std::mem::take(&mut recorder.early),
            ..Default::default()
        });
        recorder.release_held();
        let taken = recorder.take_comms();
        let kinds: Vec<_> = taken
            .iter()
            .map(|e| (e.tick, e.event.kind.as_str()))
            .collect();
        assert_eq!(kinds, [(7, kind::COMMS_RADIO), (7, kind::AUDIO_TONE)]);
        assert!(recorder.take_comms().is_empty());
        // Only the newest wait when nobody looks.
        for tick in 0..(COMMS_TAP as u64 + 5) {
            recorder.held = Some(Frame {
                tick,
                events: vec![Event::new(kind::COMMS_HUD)],
                ..Default::default()
            });
            recorder.release_held();
        }
        let taken = recorder.take_comms();
        assert_eq!(taken.len(), COMMS_TAP);
        assert_eq!(taken[0].tick, 5);
    }

    #[test]
    fn display_trees_are_handed_to_live_flight_only_while_it_shows_them() {
        let (mut recorder, _receiver) = Recorder::detached(64, &[]);
        let tree = |subject| replay::TreeSample {
            subject,
            channel: tore_replay::vocab::channel::AI_THOUGHT.into(),
            nodes: vec![replay::Node::new(0, "Activity", "ATTACKING")],
        };
        let frame = |tick, subject| Frame {
            tick,
            trees: vec![tree(subject)],
            ..Default::default()
        };
        recorder.held = Some(frame(1, 3));
        recorder.release_held();
        assert!(recorder.take_trees().is_empty());
        recorder.set_live(true);
        recorder.held = Some(frame(2, 4));
        recorder.release_held();
        assert_eq!(recorder.take_trees(), [(2, tree(4))]);
        recorder.held = Some(frame(3, 5));
        recorder.release_held();
        recorder.set_live(false);
        assert!(recorder.take_trees().is_empty());
    }

    #[test]
    fn the_seeker_tone_is_recorded_with_its_loudness_and_surface() {
        use tore_replay::vocab::tone as named;
        let lock = Tone {
            radar: false,
            ground: true,
            locked: true,
        };
        let search = Tone {
            locked: false,
            ..lock
        };
        let started = tone_event(0, None, Some((lock, 0.912))).unwrap();
        assert_eq!(started.string(field::TONE), Some(named::INFRARED_LOCK));
        assert_eq!(started.flag(field::ON), Some(true));
        assert_eq!(started.num(field::STRENGTH), Some(0.91));
        assert_eq!(started.flag(field::SURFACE), Some(true));
        // Small swells are not recorded; one of a step or more is.
        assert!(tone_event(0, Some((lock, 0.912)), Some((lock, 0.95))).is_none());
        let louder = tone_event(0, Some((lock, 0.912)), Some((lock, 0.97))).unwrap();
        assert_eq!(louder.num(field::STRENGTH), Some(0.97));
        // A new tone at the same loudness, and the tone stopping.
        let changed = tone_event(0, Some((lock, 0.5)), Some((search, 0.5))).unwrap();
        assert_eq!(changed.string(field::TONE), Some(named::GROUND));
        let stopped = tone_event(0, Some((search, 0.5)), None).unwrap();
        assert_eq!(stopped.flag(field::ON), Some(false));
        assert_eq!(stopped.string(field::TONE), Some(named::GROUND));
        assert!(tone_event(0, None, None).is_none());
    }

    #[test]
    fn a_full_queue_drops_frames_and_the_next_frame_reports_the_gap() {
        let (mut recorder, receiver) = Recorder::detached(3, &[]);
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        for number in 0..8 {
            tick(&mut recorder, &mut combat, &snapshot, number, &mut ui);
        }
        let first = frames(&receiver);
        assert!(!first.is_empty() && first.len() < 7, "{}", first.len());
        // The disk catches up: the next frame says what was lost.
        tick(&mut recorder, &mut combat, &snapshot, 8, &mut ui);
        let next = frames(&receiver);
        assert_eq!(next.len(), 1);
        let gap = &next[0].events[0];
        assert_eq!(gap.kind, kind::SYSTEM_GAP);
        let (from, to) = (
            gap.get(field::FROM)
                .and_then(replay::Value::as_i64)
                .unwrap(),
            gap.get(field::TO).and_then(replay::Value::as_i64).unwrap(),
        );
        let last_sent = first.last().unwrap().tick as i64;
        assert_eq!((from, to), (last_sent + 1, 6));
        assert_eq!(next[0].tick, 7);
        // Nothing waits forever: the recorder never blocked.
    }

    #[test]
    fn waiting_for_the_writer_never_drops_a_frame() {
        let (mut recorder, receiver) = Recorder::detached(3, &[]);
        recorder.wait_for_writer();
        // A slow writer on its own thread, as in a busy headless run.
        let writer = std::thread::spawn(move || {
            let mut ticks = Vec::new();
            for message in receiver {
                if let Message::Frame(frame) = message {
                    assert!(
                        frame.events.iter().all(|e| e.kind != kind::SYSTEM_GAP),
                        "a waiting recorder reported a gap"
                    );
                    ticks.push(frame.tick);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            ticks
        });
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        for number in 0..12 {
            tick(&mut recorder, &mut combat, &snapshot, number, &mut ui);
        }
        // The last frame is still held for notes; every earlier one arrived.
        drop(recorder);
        let ticks = writer.join().unwrap();
        assert_eq!(ticks, (0..11).collect::<Vec<u64>>());
    }

    /// A cleared frame is the empty one, with the room it had.
    #[test]
    fn a_cleared_frame_is_an_empty_one_that_keeps_its_room() {
        let mut frame = crate::replay::fixture::frame(300);
        frame.checksum = Some(7);
        frame.city_stood = true;
        frame.city_fell.push([1, 2]);
        frame.surface_hp.push((3, 4));
        assert!(!frame.aircraft.is_empty());
        let room = frame.aircraft.capacity();
        frame.clear();
        assert_eq!(frame, Frame::default());
        assert_eq!(frame.aircraft.capacity(), room);
    }

    /// Records `ticks` ticks of a scripted scene into a real file, building
    /// ticks in the frames the writer hands back or not, and reads the
    /// recording back: the frames, and how many ticks reused one.
    fn recorded(
        dir: &crate::replay::tests::TempDir,
        name: &str,
        pooled: bool,
    ) -> (Vec<Frame>, u32) {
        let roster = [replay::AircraftInfo {
            id: 0,
            label: "You".into(),
            human: true,
            ..Default::default()
        }];
        let mut recorder = Recorder::start(
            dir.path().join(format!("{name}.tore-replay")),
            &replay::Header::default(),
            &roster,
        )
        .unwrap();
        recorder.wait_for_writer();
        if !pooled {
            recorder.returned = None;
        }
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        let mut reused = 0;
        for number in 0..400 {
            reused += u32::from(!recorder.spare.is_empty());
            tick(&mut recorder, &mut combat, &snapshot, number, &mut ui);
            // Let the writer thread hand frames back, as it does between
            // real ticks.
            if number % 50 == 49 {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        let path = recorder.finish(&replay::Footer::default()).unwrap();
        let recording = replay::Recording::open(path).unwrap();
        let frames = recording.frames(0, 399).map(Result::unwrap).collect();
        (frames, reused)
    }

    /// The writer hands written frames back and the recorder builds later
    /// ticks in them: the recording is the one it makes without.
    #[test]
    fn building_ticks_in_the_frames_the_writer_hands_back_records_the_same_frames() {
        let dir = crate::replay::tests::TempDir::new("recorder-pool");
        let (pooled, reused) = recorded(&dir, "pooled", true);
        let (plain, none) = recorded(&dir, "plain", false);
        assert_eq!(none, 0);
        assert!(reused > 100, "{reused} ticks found a frame handed back");
        assert_eq!(pooled.len(), 400);
        assert_eq!(pooled, plain);
    }

    #[test]
    fn the_view_target_is_noted_on_change_only() {
        let entry = |last, now| view_target_event(0, last, now);
        // The first look records the starting state, even without a target.
        let start = entry(None, (None, false)).unwrap();
        assert_eq!(start.kind, kind::PLAYER_VIEW_TARGET);
        assert_eq!((start.subject, start.object), (Some(0), None));
        assert_eq!(start.flag(field::HELD), Some(false));
        assert!(entry(Some((None, false)), (None, false)).is_none());
        let picked = entry(Some((None, false)), (Some(4), false)).unwrap();
        assert_eq!(picked.object, Some(4));
        assert!(picked.get(field::FROM).is_none());
        assert!(entry(Some((Some(4), false)), (Some(4), false)).is_none());
        // The sensors drop it and the views hold it by sight: same target,
        // a new entry that says so and why.
        let held = entry(Some((Some(4), false)), (Some(4), true)).unwrap();
        assert_eq!((held.object, held.id(field::FROM)), (Some(4), Some(4)));
        assert_eq!(held.flag(field::HELD), Some(true));
        assert!(held.string(field::REASON).is_some());
        assert!(entry(Some((Some(4), true)), (Some(4), true)).is_none());
        // Out of sight: down to none, from the held one.
        let dropped = entry(Some((Some(4), true)), (None, false)).unwrap();
        assert_eq!((dropped.object, dropped.id(field::FROM)), (None, Some(4)));
        assert_eq!(dropped.flag(field::HELD), Some(false));
    }

    /// Combat with a synthetic radar reaching 90 nmi and the 10 nmi visual
    /// channel, so the player can designate a contact and keep it by sight.
    fn sighted(config: &live::Configuration) -> live::State {
        use tore_sim::sensors;
        let volume = |nmi: f64| sensors::Volume {
            azimuth_rad: 1.,
            elevation_rad: 1.,
            minimum_ft: 0.,
            maximum_ft: nmi * sensors::FEET_PER_NAUTICAL_MILE,
            minimum_relative_ft: f64::NEG_INFINITY,
            maximum_relative_ft: f64::INFINITY,
        };
        let mut config = config.clone();
        config.sensors.radar = Some(sensors::RadarProfile {
            record: "SYNTHETIC.SEE".into(),
            search: volume(90.),
            track: volume(50.),
            look_down: 0.,
            preset: sensors::Preset::Advanced,
            notch: sensors::Preset::Advanced.notch(),
            resistance: sensors::Preset::Advanced.resistance(),
            band: 0,
            source_flags: [0; 2],
            source_doppler: [0; 3],
        });
        config.sensors.visual = Some(sensors::profile::VisualProfile {
            record: "SYNTHETIC.VIS".into(),
            search: volume(10.),
            track: volume(5.),
        });
        live::State::new(config, true).unwrap()
    }

    #[test]
    fn a_recorded_view_target_holds_by_sight_drops_and_reads_back() {
        let dir = crate::replay::tests::TempDir::new("recorder-view-target");
        let roster = [replay::AircraftInfo {
            id: 0,
            label: "You".into(),
            human: true,
            ..Default::default()
        }];
        let mut recorder = Recorder::start(
            dir.path().join("view.tore-replay"),
            &replay::Header::default(),
            &roster,
        )
        .unwrap();
        recorder.wait_for_writer();
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        combat.state = sighted(combat.state.own().configuration());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        let launcher = combat::launcher(&player);
        let blind = live::Launcher {
            radar: false,
            radar_power: false,
            ..launcher
        };
        combat.state.range_target(0, launcher);
        let mut number = 0;
        let mut run = |recorder: &mut Recorder, combat: &mut combat::Combat, with, ticks| {
            for _ in 0..ticks {
                combat.state.step(
                    &[live::OwnshipInput {
                        aircraft: 0,
                        held: false,
                        launcher: with,
                    }],
                    |_, _| 0.,
                );
                tick(recorder, combat, &snapshot, number, &mut ui);
                number += 1;
            }
            number - 1
        };
        let on_screen = run(&mut recorder, &mut combat, launcher, 120);
        // Between ticks the player designates the contact.
        combat.command(live::Command::Designate, launcher);
        let id = combat
            .state
            .own_view()
            .designated()
            .expect("a radar contact");
        run(&mut recorder, &mut combat, launcher, 120);
        // Radar off: the sensors drop it, the views hold it by sight.
        run(&mut recorder, &mut combat, blind, 120);
        assert!(combat.state.own_view().display_target().is_none());
        assert_eq!(
            combat.state.own_view().view_target().map(|t| t.id),
            Some(id)
        );
        recorder.bookmark();
        // Past visual range it is gone.
        let far = [launcher.position[0], launcher.position[1], -70_000.];
        combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .position = far;
        run(&mut recorder, &mut combat, blind, 30);
        assert!(combat.state.own_view().view_target().is_none());
        let path = recorder
            .finish(&replay::Footer {
                end_tick: number,
                ..Default::default()
            })
            .unwrap();
        let recording = replay::Recording::open(path).unwrap();
        let views: Vec<(u64, Option<u32>, Option<bool>)> = recording
            .events()
            .iter()
            .filter(|e| e.event.kind == kind::PLAYER_VIEW_TARGET)
            .map(|e| (e.tick, e.event.object, e.event.flag(field::HELD)))
            .collect();
        // One entry per change, not one a tick: the start, the designation
        // on the tick on screen, the hold and the drop.
        assert_eq!(views.len(), 4, "{views:?}");
        assert_eq!(views[0], (0, None, Some(false)));
        assert_eq!(views[1], (on_screen, Some(id), Some(false)));
        assert_eq!((views[2].1, views[2].2), (Some(id), Some(true)));
        assert_eq!((views[3].1, views[3].2), (None, Some(false)));
        assert!(views[2].0 > views[1].0 && views[3].0 > views[2].0);
        // The replay viewer follows them for the player.
        let targets = super::super::viewer::targets(recording.events(), 0);
        let player: Vec<(u64, Option<u32>)> = views.iter().map(|v| (v.0, v.1)).collect();
        assert_eq!(targets[&0], player);
        // The summary's bookmark window says so in words.
        let mut summary = Vec::new();
        replay::export::write_summary(
            &recording,
            &replay::export::SummaryOptions::default(),
            &mut summary,
        )
        .unwrap();
        let summary = String::from_utf8(summary).unwrap();
        assert!(summary.contains("You view target none -> "), "{summary}");
        assert!(summary.contains(" held by sight because the sensors dropped it"));
    }

    #[test]
    fn notes_between_ticks_land_on_the_tick_on_screen() {
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        recorder.session(&ui);
        tick(&mut recorder, &mut combat, &snapshot, 0, &mut ui);
        // Between ticks: a bookmark, a pause, a cockpit message and a
        // command, all while tick 0 is on screen.
        assert_eq!(recorder.bookmark(), 1);
        ui.paused = true;
        recorder.session(&ui);
        ui.message("Radio silence");
        ui.message("Radio silence");
        combat.command(live::Command::ClearDesignation, combat::launcher(&player));
        tick(&mut recorder, &mut combat, &snapshot, 1, &mut ui);
        tick(&mut recorder, &mut combat, &snapshot, 2, &mut ui);
        let frames = frames(&receiver);
        assert_eq!(frames.iter().map(|f| f.tick).collect::<Vec<_>>(), [0, 1]);
        // Cockpit sounds depend on the fixture's aircraft; leave them out.
        let kinds: Vec<&str> = frames[0]
            .events
            .iter()
            .map(|e| e.kind.as_str())
            .filter(|kind| !kind.starts_with("audio."))
            .collect();
        // The first frame opens with the starting view target, then the HUD
        // sample (nr-replayhud). The notes taken between ticks follow.
        assert_eq!(
            kinds,
            [
                kind::PLAYER_VIEW_TARGET,
                kind::AIRCRAFT_HUD,
                kind::PLAYER_BOOKMARK,
                kind::SYSTEM_PAUSE,
                kind::COMMS_HUD,
                kind::COMMS_HUD,
                kind::PLAYER_COMMAND,
            ]
        );
        // The repeat moves the line on screen to the bottom with a new timer.
        let hud: Vec<(Option<&str>, bool)> = frames[0]
            .events
            .iter()
            .filter(|e| e.kind == kind::COMMS_HUD)
            .map(|e| (e.string(field::OUTCOME), e.get(field::REASON).is_some()))
            .collect();
        assert_eq!(
            hud,
            [
                (Some(outcome::DELIVERED), false),
                (Some(outcome::DELIVERED), true)
            ]
        );
        let bookmark = frames[0]
            .events
            .iter()
            .find(|e| e.kind == kind::PLAYER_BOOKMARK)
            .unwrap();
        assert_eq!(bookmark.text, "Bookmark 1");
        assert!(
            frames[1]
                .events
                .iter()
                .all(|e| e.kind.starts_with("audio."))
        );
        // Tick 0 carries the once-a-second checksum; tick 1 does not.
        assert!(frames[0].checksum.is_some() && frames[1].checksum.is_none());
    }

    /// Opens tick `number` the way the live loop's presentation does, with
    /// `between` standing for the world's command phase and the cues it made:
    /// they run first, and `start_tick` follows them.
    fn tick_with_commands(
        recorder: &mut Recorder,
        combat: &mut combat::Combat,
        snapshot: &RenderSnapshot,
        number: u64,
        ui: &mut flight_ui::FlightUi,
        between: impl FnOnce(&mut combat::Combat, &mut flight_ui::FlightUi),
    ) {
        let flight = fixture::player();
        let world = tore_world::test_support::terrain();
        let mut snapshot = snapshot.clone();
        snapshot.tick = number;
        between(combat, ui);
        recorder.start_tick(Some(ui), combat);
        ui.message(format!("cue after the commands, tick {number}"));
        recorder.begin(Tick {
            snapshot: &snapshot,
            combat,
            flight: &flight,
            previous: &flight,
            pilot: &flight::PilotInput::default(),
            others: &[],
            wings: None,
            world: &world,
            events: &[],
            outcomes: &[],
            journal: None,
        });
        recorder.end(Some(ui), combat);
    }

    #[test]
    fn commands_the_step_applies_are_noted_on_the_frame_before_their_tick() {
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        tick_with_commands(&mut recorder, &mut combat, &snapshot, 0, &mut ui, |_, _| {});
        // Tick 1's command phase: a command and the message it gives the
        // pilot. Both belong to tick 0's frame, where a command given between
        // the frames was always noted; a cue that comes later in tick 1 is
        // tick 1's.
        tick_with_commands(
            &mut recorder,
            &mut combat,
            &snapshot,
            1,
            &mut ui,
            |combat, ui| {
                combat.command(live::Command::ClearDesignation, combat::launcher(&player));
                ui.message("Seeker mode changed");
            },
        );
        tick_with_commands(&mut recorder, &mut combat, &snapshot, 2, &mut ui, |_, _| {});
        let frames = frames(&receiver);
        assert_eq!(frames.iter().map(|f| f.tick).collect::<Vec<_>>(), [0, 1]);
        let of = |frame: &Frame| -> Vec<(String, String)> {
            frame
                .events
                .iter()
                .filter(|e| e.kind == kind::PLAYER_COMMAND || e.kind == kind::COMMS_HUD)
                .map(|e| (e.kind.clone(), e.text.clone()))
                .collect()
        };
        assert_eq!(
            of(&frames[0]),
            [
                (
                    kind::COMMS_HUD.to_owned(),
                    "cue after the commands, tick 0".to_owned()
                ),
                (kind::COMMS_HUD.to_owned(), "Seeker mode changed".to_owned()),
                (
                    kind::PLAYER_COMMAND.to_owned(),
                    "ClearDesignation".to_owned()
                ),
            ]
        );
        assert_eq!(
            of(&frames[1]),
            [(
                kind::COMMS_HUD.to_owned(),
                "cue after the commands, tick 1".to_owned()
            )]
        );
    }

    #[test]
    fn released_chaff_and_flares_are_recorded_exactly_on_their_ticks() {
        use tore_sim::combat::countermeasures::Release;
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(Vec::new(), Vec::new());
        let player = fixture::player();
        combat.restart_render(combat.own_id(), &player, None);
        let snapshot = combat.render_snapshot().clone();
        let mut ui = flight_ui::FlightUi::default();
        let own = combat.state.own_mut();
        (own.chaff, own.flares) = (2, 2);
        tick(&mut recorder, &mut combat, &snapshot, 0, &mut ui);
        // Between ticks the player releases chaff: its entries land on the
        // tick on screen, the sound with the player's own flag.
        let launcher = combat::launcher(&player);
        combat.command(live::Command::ReleaseChaff, launcher);
        let emissions = combat.state.take_sound_events();
        recorder.sounds(&emissions, &[]);
        tick(&mut recorder, &mut combat, &snapshot, 1, &mut ui);
        // An AI aircraft's flare leaves during tick 2, and a range reset
        // clears the devices during tick 3.
        let flare = Release {
            position: [100.5, 2_000.25, -300.125],
            velocity: [1. / 3., -2., 700.],
            basis: tore_sim::attitude::Basis::new(0.3, -0.1, 0.7),
        };
        for (number, change) in [(2u64, true), (3, false)] {
            recorder.start_tick(Some(&mut ui), &mut combat);
            if change {
                combat
                    .state
                    .device_released(flare, live::EffectKind::Flare, 7);
            } else {
                combat.state.range_target(0, launcher);
            }
            let mut snapshot = snapshot.clone();
            snapshot.tick = number;
            let (world, flight) = (tore_world::test_support::terrain(), fixture::player());
            recorder.begin(Tick {
                snapshot: &snapshot,
                combat: &combat,
                flight: &flight,
                previous: &flight,
                pilot: &flight::PilotInput::default(),
                others: &[],
                wings: None,
                world: &world,
                events: &[],
                outcomes: &[],
                journal: None,
            });
            recorder.end(Some(&mut ui), &mut combat);
        }
        tick(&mut recorder, &mut combat, &snapshot, 4, &mut ui);
        let frames = frames(&receiver);
        let entries = |tick: u64, kind: &str| -> Vec<Event> {
            frames
                .iter()
                .filter(|f| f.tick == tick)
                .flat_map(|f| f.events.iter())
                .filter(|e| e.kind == kind)
                .cloned()
                .collect()
        };
        let chaff = entries(0, kind::COMBAT_COUNTERMEASURE);
        assert_eq!(chaff.len(), 1);
        assert_eq!(
            convert::device_release(&chaff[0], 0),
            Some(convert::DeviceRelease {
                owner: 0,
                kind: live::EffectKind::Chaff,
                release: Release {
                    position: launcher.position,
                    velocity: launcher.velocity,
                    basis: launcher.basis,
                },
                number: 1,
                // The fixture's combat never stepped.
                tick: combat.state.tick(),
            })
        );
        assert_eq!(chaff[0].num(field::LEFT), Some(1.));
        let sounds = entries(0, kind::AUDIO_EFFECT);
        assert_eq!(sounds.len(), 1);
        assert_eq!(
            (sounds[0].string(field::SOUND), sounds[0].flag(field::OWN)),
            (Some("chaff"), Some(true))
        );
        let command = entries(0, kind::PLAYER_COMMAND);
        assert_eq!(command[0].string(field::COMMAND), Some("chaff"));
        // The AI's flare: every number back bit for bit; its dispensers are
        // unknown without an AI bridge, so no count is kept.
        let ai = entries(2, kind::COMBAT_COUNTERMEASURE);
        let back = convert::device_release(&ai[0], 2).unwrap();
        assert_eq!(
            (back.owner, back.kind, back.number),
            (7, live::EffectKind::Flare, 2)
        );
        for (a, b) in [
            (back.release.position, flare.position),
            (back.release.velocity, flare.velocity),
            (back.release.basis.right, flare.basis.right),
            (back.release.basis.up, flare.basis.up),
            (back.release.basis.forward, flare.basis.forward),
        ] {
            assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
        }
        assert!(ai[0].get(field::LEFT).is_none());
        assert_eq!(entries(3, kind::COMBAT_COUNTERMEASURES_CLEARED).len(), 1);
    }

    #[test]
    fn launches_effects_and_aircraft_are_recorded_from_the_picture() {
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(
            fixture::models(),
            (0..7).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let player = fixture::player();
        let scene = fixture::scene(combat.state.own().configuration());
        let [previous, current] = fixture::snapshots(&mut combat, &scene, true, &player);
        let mut ui = flight_ui::FlightUi::default();
        tick(&mut recorder, &mut combat, &previous, 10, &mut ui);
        tick(&mut recorder, &mut combat, &current, 11, &mut ui);
        tick(&mut recorder, &mut combat, &current, 12, &mut ui);
        let mut registered = BTreeSet::new();
        let mut weapons = BTreeSet::new();
        let mut frames = Vec::new();
        for message in receiver.try_iter() {
            match message {
                Message::Aircraft(info) => assert!(registered.insert(info.id)),
                Message::Weapon(info) => assert!(weapons.insert(info.source.clone())),
                Message::Surface(_) => unreachable!("no surface unit in this world"),
                Message::Frame(frame) => frames.push(*frame),
                Message::Finish(_) => unreachable!(),
            }
        }
        // The player and every aircraft of either tick, but not the two
        // ground objects.
        let aircraft = |snapshot: &RenderSnapshot| -> BTreeSet<u32> {
            std::iter::once(0)
                .chain(
                    snapshot
                        .targets
                        .iter()
                        .filter(|t| t.aircraft.is_some())
                        .map(|t| t.id),
                )
                .collect()
        };
        assert_eq!(registered, &aircraft(&previous) | &aircraft(&current));
        assert_eq!(frames[1].aircraft.len(), aircraft(&current).len());
        assert_eq!(frames[1].surface_hp.len(), 0, "unchanged since tick 10");
        assert_eq!(frames[0].surface_hp.len(), 2);
        // Every weapon in flight launched once, each type registered once.
        let launches = |frame: &Frame| {
            frame
                .events
                .iter()
                .filter(|e| e.kind == kind::WEAPON_LAUNCH)
                .count()
        };
        assert_eq!(launches(&frames[0]), 0);
        assert_eq!(launches(&frames[1]), current.projectiles.len());
        let sources: BTreeSet<String> = current
            .projectiles
            .iter()
            .map(|p| p.weapon.clone())
            .collect();
        assert_eq!(weapons, sources);
        // Effects start once, lasting the ticks they have left.
        assert_eq!(frames[1].new_effects.len(), current.effects.len());
        assert_eq!(
            frames[1].new_effects[1].duration_ticks,
            u32::from(current.effects[1].ticks)
        );
    }

    #[test]
    fn a_ground_burst_writes_one_ground_impact_naming_the_weapon_that_vanished() {
        use tore_world::snapshot::EffectPose;
        // A bare ground effect (a recording from before explosion types) and
        // a blast on the ground (every ground burst since) each write one.
        for blast in [None, Some(18)] {
            let (mut recorder, receiver) = Recorder::detached(64, &[]);
            let mut combat = fixture::combat(
                fixture::models(),
                (0..7).map(|i| (i % 3, [0.; 3])).collect(),
            );
            let player = fixture::player();
            let scene = fixture::scene(combat.state.own().configuration());
            let [_, mut flying] = fixture::snapshots(&mut combat, &scene, true, &player);
            flying.effects.clear();
            let shot = flying.projectiles[0].clone();
            let mut ui = flight_ui::FlightUi::default();
            tick(&mut recorder, &mut combat, &flying, 10, &mut ui);
            // The weapon is gone and its burst stands where it was.
            let mut landed = flying.clone();
            landed.projectiles.retain(|p| p.id != shot.id);
            landed.effects = vec![EffectPose {
                kind: tore_sim::combat::live::EffectKind::Ground,
                position: shot.position,
                ticks: 20,
                blast,
                scale: tore_sim::combat::blast_scale::FULL,
            }];
            tick(&mut recorder, &mut combat, &landed, 11, &mut ui);
            // The same burst a tick on, one tick shorter: not a new one.
            landed.effects[0].ticks -= 1;
            tick(&mut recorder, &mut combat, &landed, 12, &mut ui);
            let frames: Vec<Frame> = receiver
                .try_iter()
                .filter_map(|m| match m {
                    Message::Frame(f) => Some(*f),
                    _ => None,
                })
                .collect();
            let impacts: Vec<(usize, &Event)> = frames
                .iter()
                .enumerate()
                .flat_map(|(i, f)| f.events.iter().map(move |e| (i, e)))
                .filter(|(_, e)| e.kind == kind::COMBAT_GROUND_IMPACT)
                .collect();
            assert_eq!(
                impacts.len(),
                1,
                "blast {blast:?}: once, on the tick it lands"
            );
            let (frame, event) = impacts[0];
            assert_eq!(frame, 1);
            assert_eq!(event.subject, Some(shot.owner));
            assert_eq!(event.id(field::PROJECTILE), Some(shot.id));
            assert!(event.id(field::WEAPON).is_some());
            assert_eq!(event.num(field::X_FT), Some(shot.position[0]));
            assert_eq!(event.num(field::Z_FT), Some(shot.position[2]));
        }
    }

    #[test]
    fn only_a_strike_on_the_ground_counts_as_a_ground_burst() {
        use tore_sim::combat::live::EffectKind as Live;
        let kind = |live, blast| convert::effect_kind(live, blast);
        assert!(is_ground_burst(kind(Live::Ground, None)));
        assert!(is_ground_burst(kind(Live::Ground, Some(18))));
        assert!(!is_ground_burst(kind(Live::Hit, Some(18))));
        assert!(!is_ground_burst(kind(Live::Destroyed, Some(18))));
        assert!(!is_ground_burst(kind(Live::Flak, Some(27))));
        assert!(!is_ground_burst(kind(Live::Launch, None)));
    }

    #[test]
    fn laser_and_ground_designation_changes_are_recorded_once() {
        use tore_sim::combat::laser::{LaserState, Mask};
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(
            fixture::models(),
            (0..7).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let player = fixture::player();
        let scene = fixture::scene(combat.state.own().configuration());
        let snapshot = fixture::snapshots(&mut combat, &scene, true, &player)[1].clone();
        let mut ui = flight_ui::FlightUi::default();
        // A ground object of the mission's surface world.
        let ground = 0x5000_0001;
        combat
            .state
            .targets
            .iter_mut()
            .find(|t| !t.airborne)
            .expect("the fixture has ground objects")
            .id = ground;
        tick(&mut recorder, &mut combat, &snapshot, 0, &mut ui);
        // Designate a ground object, then let the laser go ready, lase,
        // mask and stop; ticks that change nothing add nothing.
        combat.state.own_mut().sensors.hold(ground);
        let steps: [(LaserState, Option<Mask>); 6] = [
            (LaserState::Off, None),
            (LaserState::Ready, None),
            (LaserState::Lasing, None),
            (LaserState::Lasing, None),
            (LaserState::Masked, Some(Mask::Range)),
            (LaserState::Off, None),
        ];
        for (n, (state, mask)) in steps.into_iter().enumerate() {
            let own = combat.state.own_mut();
            own.laser.state = state;
            own.laser.mask = mask;
            own.laser.point = (state == LaserState::Lasing).then_some([10., 20., 30.]);
            tick(&mut recorder, &mut combat, &snapshot, 1 + n as u64, &mut ui);
        }
        // The last frame stays open to notes until the next tick.
        tick(&mut recorder, &mut combat, &snapshot, 7, &mut ui);
        let frames = frames(&receiver);
        let events = |kind: &str| -> Vec<(u64, Event)> {
            frames
                .iter()
                .flat_map(|f| f.events.iter().map(move |e| (f.tick, e.clone())))
                .filter(|(_, e)| e.kind == kind)
                .collect()
        };
        let lasers = events(kind::PLAYER_LASER);
        let states: Vec<(u64, &str)> = lasers
            .iter()
            .map(|(tick, e)| (*tick, e.string(field::STATE).unwrap()))
            .collect();
        assert_eq!(
            states,
            [(2, "ready"), (3, "lasing"), (5, "masked"), (6, "off")]
        );
        let lasing = &lasers[1].1;
        assert_eq!(lasing.flag(field::ON), Some(true));
        assert_eq!(lasing.num(field::Y_FT), Some(20.));
        assert_eq!(lasers[2].1.string(field::MASK), Some("range"));
        let designations = events(kind::PLAYER_DESIGNATE);
        assert_eq!(designations.len(), 1, "the object is designated once");
        assert_eq!(designations[0].1.object, Some(ground));
        assert_eq!(designations[0].1.string(field::CHANNEL), Some("visual"));
    }

    #[test]
    fn a_moving_laser_spot_is_written_at_most_ten_times_a_second() {
        use tore_sim::combat::laser::LaserState;
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(
            fixture::models(),
            (0..7).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let player = fixture::player();
        let scene = fixture::scene(combat.state.own().configuration());
        let snapshot = fixture::snapshots(&mut combat, &scene, true, &player)[1].clone();
        let mut ui = flight_ui::FlightUi::default();
        // The spot runs 100 feet a tick for 40 ticks, then holds still for 30.
        for n in 0..70u64 {
            let own = combat.state.own_mut();
            own.laser.state = LaserState::Lasing;
            own.laser.point = Some([100. * n.min(40) as f64, 0., 50.]);
            tick(&mut recorder, &mut combat, &snapshot, n, &mut ui);
        }
        // Switching off ends the entries, and the next lasing starts afresh.
        combat.state.own_mut().laser.state = LaserState::Off;
        tick(&mut recorder, &mut combat, &snapshot, 70, &mut ui);
        combat.state.own_mut().laser.state = LaserState::Lasing;
        combat.state.own_mut().laser.point = Some([9., 9., 9.]);
        tick(&mut recorder, &mut combat, &snapshot, 71, &mut ui);
        tick(&mut recorder, &mut combat, &snapshot, 72, &mut ui);
        let frames = frames(&receiver);
        let spots: Vec<(u64, f64)> = frames
            .iter()
            .flat_map(|f| f.events.iter().map(move |e| (f.tick, e)))
            .filter(|(_, e)| e.kind == kind::PLAYER_LASER_SPOT)
            .map(|(tick, e)| (tick, e.num(field::X_FT).unwrap()))
            .collect();
        // The state event writes the first spot (not an entry of this kind);
        // then one every 12 ticks while it moves (the last catches it where
        // it stopped), none once it holds still, and none again for the new
        // lasing, whose state event wrote it.
        assert_eq!(
            spots,
            [(12, 1_200.), (24, 2_400.), (36, 3_600.), (48, 4_000.)]
        );
    }

    #[test]
    fn a_laser_and_designation_kept_while_the_ai_flies_are_still_recorded() {
        use tore_sim::combat::laser::LaserState;
        let (mut recorder, receiver) = Recorder::detached(64, &[]);
        let mut combat = fixture::combat(
            fixture::models(),
            (0..7).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let player = fixture::player();
        let scene = fixture::scene(combat.state.own().configuration());
        let snapshot = fixture::snapshots(&mut combat, &scene, true, &player)[1].clone();
        let mut ui = flight_ui::FlightUi::default();
        let ground = 0x5000_0001;
        combat
            .state
            .targets
            .iter_mut()
            .find(|t| !t.airborne)
            .expect("the fixture has ground objects")
            .id = ground;
        // A second human, lasing a spot and designating a ground object.
        const BUDDY: u32 = 7;
        let config = combat.state.own().configuration().clone();
        let mut buddy = tore_sim::combat::live::Ownship::new(
            BUDDY,
            tore_sim::combat::live::DEFAULT_OWNSHIP_SIDE,
            config,
            false,
        )
        .unwrap();
        buddy.sensors.hold(ground);
        buddy.laser.state = LaserState::Lasing;
        buddy.laser.point = Some([0., 0., 50.]);
        combat.state.add_ownship(buddy).unwrap();
        tick(&mut recorder, &mut combat, &snapshot, 0, &mut ui);
        tick(&mut recorder, &mut combat, &snapshot, 1, &mut ui);
        // The human drops: the AI flies on with the cockpit kept. Its spot
        // runs 100 feet a tick, then the laser goes off, then the human
        // clears the designation.
        let gone = combat.state.remove_ownship(BUDDY).unwrap();
        combat.state.keep(BUDDY, gone.kept());
        for n in 0..40u64 {
            let mut kept = combat.state.take_kept(BUDDY).unwrap();
            kept.laser.point = Some([100. * n as f64, 0., 50.]);
            combat.state.keep(BUDDY, kept);
            tick(&mut recorder, &mut combat, &snapshot, 2 + n, &mut ui);
        }
        let mut kept = combat.state.take_kept(BUDDY).unwrap();
        kept.laser.state = LaserState::Off;
        kept.laser.point = None;
        kept.picks.selected = None;
        combat.state.keep(BUDDY, kept);
        tick(&mut recorder, &mut combat, &snapshot, 42, &mut ui);
        tick(&mut recorder, &mut combat, &snapshot, 43, &mut ui);
        // A second dropped human lases, then the aircraft is destroyed with
        // the laser on: the recording ends the laser and the designation.
        let config = combat.state.own().configuration().clone();
        let mut lost = tore_sim::combat::live::Ownship::new(
            9,
            tore_sim::combat::live::DEFAULT_OWNSHIP_SIDE,
            config,
            false,
        )
        .unwrap();
        lost.sensors.hold(ground);
        lost.laser.state = LaserState::Lasing;
        lost.laser.point = Some([5., 0., 5.]);
        combat.state.add_ownship(lost).unwrap();
        tick(&mut recorder, &mut combat, &snapshot, 44, &mut ui);
        let lost = combat.state.remove_ownship(9).unwrap();
        combat.state.keep(9, lost.kept());
        tick(&mut recorder, &mut combat, &snapshot, 45, &mut ui);
        combat.state.take_kept(9);
        tick(&mut recorder, &mut combat, &snapshot, 46, &mut ui);
        tick(&mut recorder, &mut combat, &snapshot, 47, &mut ui);
        let frames = frames(&receiver);
        let mine = |kind: &str| -> Vec<(u64, Event)> {
            frames
                .iter()
                .flat_map(|f| f.events.iter().map(move |e| (f.tick, e.clone())))
                .filter(|(_, e)| e.kind == kind && e.subject == Some(BUDDY))
                .collect()
        };
        let spots = mine(kind::PLAYER_LASER_SPOT);
        assert!(
            spots.len() >= 3,
            "the spot is followed in the gap: {spots:?}"
        );
        assert!(spots.iter().all(|(_, e)| e.flag(field::KEPT) == Some(true)));
        let lasers = mine(kind::PLAYER_LASER);
        let states: Vec<(u64, &str, Option<bool>)> = lasers
            .iter()
            .map(|(t, e)| (*t, e.string(field::STATE).unwrap(), e.flag(field::KEPT)))
            .collect();
        // The laser comes on while a human flies, and goes off in the gap.
        assert_eq!(states, [(0, "lasing", None), (42, "off", Some(true))]);
        let designations = mine(kind::PLAYER_DESIGNATE);
        let channels: Vec<(u64, &str, Option<bool>)> = designations
            .iter()
            .map(|(t, e)| (*t, e.string(field::CHANNEL).unwrap(), e.flag(field::KEPT)))
            .collect();
        assert_eq!(channels, [(0, "visual", None), (42, "cleared", Some(true))]);
        // The destroyed aircraft's laser and designation end with it, once.
        let ended = |kind: &str| -> Vec<(u64, &str)> {
            frames
                .iter()
                .flat_map(|f| f.events.iter().map(move |e| (f.tick, e)))
                .filter(|(_, e)| e.kind == kind && e.subject == Some(9))
                .map(|(t, e)| {
                    let word = e.string(field::STATE).or(e.string(field::CHANNEL));
                    (t, word.unwrap())
                })
                .collect()
        };
        assert_eq!(ended(kind::PLAYER_LASER), [(44, "lasing"), (46, "off")]);
        assert_eq!(
            ended(kind::PLAYER_DESIGNATE),
            [(44, "visual"), (46, "cleared")]
        );
    }

    /// Records the fixture's two snapshots for the seat that flies `plane` and
    /// returns the frames and the roster the recording registered.
    fn record_for(
        seat: SeatId,
        plane: u32,
        snapshots: &[RenderSnapshot; 2],
    ) -> (Vec<Frame>, Vec<replay::AircraftInfo>) {
        let human = |id: u32, label: &str| Human {
            id,
            label: label.into(),
            name: String::new(),
            side: replay::Side::Friendly,
            wing: 1,
            member: 1 + id as u16,
        };
        let others: Vec<Human> = [0, plane]
            .into_iter()
            .filter(|id| *id != plane)
            .map(|id| human(id, "Seat 1"))
            .collect();
        let roster = roster(&snapshots[1], &human(plane, "You"), &others, None, &[]);
        let (recorder, receiver) = Recorder::detached(64, &roster);
        let mut recorder = recorder.for_seat(seat, plane);
        let mut combat = fixture::combat(
            fixture::models(),
            (0..3).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let mut ui = flight_ui::FlightUi::default();
        tick(&mut recorder, &mut combat, &snapshots[0], 10, &mut ui);
        recorder.bookmark();
        tick(&mut recorder, &mut combat, &snapshots[1], 11, &mut ui);
        tick(&mut recorder, &mut combat, &snapshots[1], 12, &mut ui);
        assert_eq!(recorder.player(), plane);
        let mut infos = Vec::new();
        let mut frames = Vec::new();
        for message in receiver.try_iter() {
            match message {
                Message::Aircraft(info) => infos.push(*info),
                Message::Frame(frame) => frames.push(*frame),
                _ => {}
            }
        }
        (frames, infos)
    }

    #[test]
    fn a_recording_for_seat_1_names_seat_1s_plane_as_the_player() {
        let mut combat = fixture::combat(
            fixture::models(),
            (0..7).map(|i| (i % 3, [0.; 3])).collect(),
        );
        let player = fixture::player();
        let scene = fixture::scene(combat.state.own().configuration());
        let snapshots = fixture::snapshots(&mut combat, &scene, true, &player);
        // Seat 1 flies the first AI aircraft of the picture.
        let plane = snapshots[1]
            .targets
            .iter()
            .find(|t| t.aircraft.is_some())
            .unwrap()
            .id;
        assert_ne!(plane, 0);
        let (frames, infos) = record_for(SeatId(1), plane, &snapshots);
        // The roster registers it first, as `You`, a human; plane 0 is an
        // ordinary human pilot of the mission.
        assert_eq!(infos[0].id, plane);
        assert_eq!((infos[0].label.as_str(), infos[0].human), ("You", true));
        let zero = infos.iter().find(|i| i.id == 0).unwrap();
        assert_eq!((zero.label.as_str(), zero.human), ("Seat 1", true));
        // Every frame lists the player first and plane 0 among the others,
        // each aircraft once.
        for frame in &frames {
            assert_eq!(frame.aircraft[0].id, plane);
            let mut ids: Vec<u32> = frame.aircraft.iter().map(|a| a.id).collect();
            assert!(ids.contains(&0));
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), frame.aircraft.len());
        }
        // What only the player has is the seat's plane's.
        let events: Vec<&Event> = frames.iter().flat_map(|f| &f.events).collect();
        let personal: Vec<&&Event> = events
            .iter()
            .filter(|e| e.kind.starts_with("player."))
            .collect();
        assert!(
            personal.iter().any(|e| e.kind == kind::PLAYER_BOOKMARK)
                && personal.iter().any(|e| e.kind == kind::PLAYER_VIEW_TARGET)
        );
        assert!(personal.iter().all(|e| e.subject == Some(plane)));
        // Single player's recording of the same picture names plane 0.
        let (frames, infos) = record_for(SeatId(0), 0, &snapshots);
        assert_eq!(infos[0].id, 0);
        assert_eq!(frames[0].aircraft[0].id, 0);
        assert!(
            frames
                .iter()
                .flat_map(|f| &f.events)
                .filter(|e| e.kind.starts_with("player."))
                .all(|e| e.subject == Some(0))
        );
    }
}
