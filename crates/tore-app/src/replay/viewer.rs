//! The mission replay viewer: plays a recording back from any viewpoint.
//! Opinionated addition requested by John on 2026-09-26; see
//! docs/REPLAYS.md. The picture comes from the same drawing helpers and the
//! same renderer calls as live flight, fed from the recording; every
//! interface choice here is an agent design (2026-09-26).
//!
//! The viewer owns its own world and aircraft models. Entering it points
//! the renderer at them; leaving hands the renderer back to the game's.
pub(crate) mod banner_check;
mod director;
mod views;
use crate::aircraft::Airframe;
use crate::flight;
use crate::flight_canvas::FlightCanvas;
use crate::flight_views::{self, Body, Reference, Rig, Scene};
use crate::pylons;
use crate::render_snapshot::{self, CombatArt};
use crate::renderer::Renderer;
use crate::replay::clock::{self, Clock, Direction};
use crate::replay::context_menu::{
    self, Action, Item, Menu, Outcome, Pickable, RightClick, Target,
};
use crate::replay::convert;
use crate::replay::devices::DeviceTrack;
use crate::replay::drone::{Drone, Mode};
use crate::replay::ground_marks;
use crate::replay::overlay::{self, Control, Marker, MarkerKind, Model, Placement};
use crate::replay::panels::{self, Data, Kind, Panels, RecordedTrees};
use crate::replay::pause;
use crate::replay::playback::Playback;
use crate::replay::sound::{self, ReplaySound};
use crate::replay::tape_views;
use crate::replay::tracks::{Scanner, Tracks};
use crate::replay::trails;
use crate::replay::weather::WeatherTrack;
use crate::snapshot::{self, RenderSnapshot};
use crate::{AppResult, attitude::Basis};
use crate::{camera::Camera, scenery::Scenery, terrain::Terrain};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tore_formats::aircraft::AircraftId;
use tore_replay::{
    AircraftInfo, Event, Recording, Side, TimedEvent, TreeSample, WeaponClass, vocab,
};

#[derive(Clone, Copy, Default)]
struct WeaponApproach {
    scanned: u64,
    target: Option<u32>,
    stopped: Option<(u64, flight_views::WeaponStop)>,
}

/// Weather snapshots built ahead of the playhead each frame, in ticks:
/// a ten-minute recording is ready in about a second.
const WEATHER_BUDGET: u64 = 1_200;
/// How long a subtitle stays up, in ticks: about four seconds.
const SUBTITLE_TICKS: u64 = 480;
/// Messages at the top of the view last this long.
const TOAST: Duration = Duration::from_secs(3);
/// Labels further away than this are left out, feet (100 nautical miles).
const LABEL_REACH: f64 = 607_600.;
/// The view the viewer opens in: F10, outside the aircraft.
const EXTERNAL: u8 = 1;
/// The object view, from any object to any other: numbered after the
/// flight views, and only in a replay.
pub const OBJECT: u8 = 12;
/// Feet in a nautical mile, for the object view's range readout.
const NMI: f64 = 6_076.12;
/// An aircraft whose model origin is this close to the camera is the one
/// the camera sits in, as in the back view from its pilot's eye: no label.
const INSIDE: f64 = 30.;

/// What the viewer shows besides the 3D view. Every part can be switched
/// on its own; `hidden` hides all of them at once, trails excepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub hidden: bool,
    pub labels: bool,
    pub timer: bool,
    /// The Comms panel is open.
    pub comms: bool,
    pub subtitles: bool,
    pub trails: bool,
    /// Index into [`trails::LENGTHS`].
    pub trail_length: usize,
    /// Laser spots, designations and guided weapons drawn in the 3D view
    /// (B, FX-R); on at first, and only for a recording that has any.
    pub marks: bool,
    pub banner: bool,
}

impl Ui {
    /// The interface parts named in a comma-separated list (labels, timer,
    /// trails, marks, banner, comms, subtitles), all others off; `none` for none.
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut ui = Self {
            labels: false,
            timer: false,
            subtitles: false,
            marks: false,
            banner: false,
            ..Self::default()
        };
        for part in list.split(',').map(str::trim) {
            match part {
                "labels" => ui.labels = true,
                "timer" => ui.timer = true,
                "trails" => ui.trails = true,
                "marks" => ui.marks = true,
                "banner" => ui.banner = true,
                "comms" => ui.comms = true,
                "subtitles" => ui.subtitles = true,
                "none" | "" => {}
                other => {
                    return Err(format!(
                        "unknown replay interface part {other:?}: use labels, timer, trails, marks, banner, comms or subtitles"
                    ));
                }
            }
        }
        Ok(ui)
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            hidden: false,
            labels: true,
            timer: true,
            comms: false,
            subtitles: true,
            trails: false,
            trail_length: trails::DEFAULT_LENGTH,
            marks: true,
            banner: true,
        }
    }
}

/// A debug panel or the right-click menu to open when the viewer starts,
/// from `--replay-panels`, for captures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// AI thinking of the selected aircraft.
    Thought,
    /// Telemetry of the selected aircraft.
    Telemetry,
    /// Guidance of the selected aircraft's newest missile in flight.
    Guidance,
    Comms,
    /// The right-click menu on the selected aircraft.
    Menu,
}

impl Request {
    /// The panels named in a comma-separated list: thought, telemetry,
    /// guidance, comms and menu.
    pub fn parse(list: &str) -> Result<Vec<Self>, String> {
        list.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| match part {
                "thought" => Ok(Self::Thought),
                "telemetry" => Ok(Self::Telemetry),
                "guidance" => Ok(Self::Guidance),
                "comms" => Ok(Self::Comms),
                "menu" => Ok(Self::Menu),
                other => Err(format!(
                    "unknown replay panel {other:?}: use thought, telemetry, guidance, comms or menu"
                )),
            })
            .collect()
    }
}

/// How the viewer starts, from the command line or the Replays screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// Start paused at this tick.
    pub tick: Option<u64>,
    /// Flight view, 0 to 11 as `--flight-view` numbers them.
    pub view: Option<u8>,
    /// Aircraft to select.
    pub aircraft: Option<u32>,
    /// Start in the follow drone.
    pub drone: bool,
    /// Start playing at this speed, backwards when negative.
    pub speed: Option<f64>,
    pub ui: Ui,
    /// Debug panels to open on the first frame.
    pub panels: Vec<Request>,
    /// Start with the Escape menu open at this page, for captures.
    pub menu: Option<pause::Start>,
    /// Start in the object view, looking at this.
    pub look_at: Option<Target>,
}

/// An object named on the command line: `aircraft:ID`, `ground:ID` or
/// `weapon:ID`.
pub fn parse_object(text: &str) -> Result<Target, String> {
    let bad = || format!("{text:?} is not an object: use aircraft:ID, ground:ID or weapon:ID");
    let (kind, id) = text.split_once(':').ok_or_else(bad)?;
    let id: u32 = id.trim().parse().map_err(|_| bad())?;
    match kind.trim() {
        "aircraft" => Ok(Target::Aircraft(id)),
        "ground" => Ok(Target::Ground(id)),
        "weapon" => Ok(Target::Missile(id)),
        _ => Err(bad()),
    }
}

/// How long one frame's parts took, in milliseconds, for the
/// `TORE_PERF_FRAMES` measurement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    /// The frame reached the screen.
    pub presented: bool,
    /// Rebuilding the moment and preparing the 3D view.
    pub scene: f64,
    /// Labels and the interface.
    pub interface: f64,
    /// Handing the frame to the GPU.
    pub present: f64,
}

/// What the app should do after a key or click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    None,
    /// The Escape menu moved: a click sound.
    Click,
    /// Back to the menu.
    Leave,
    /// Quit the game, from the Escape menu.
    Exit,
    /// Open the Graphics options screen over the paused replay.
    Graphics,
    /// Open the Sound/Music Prefs screen over the paused replay.
    Sound,
    /// Open the controls screen over the paused replay.
    Controls,
    /// Save the frame as a PNG.
    Screenshot,
    /// The away player takes its own aircraft back (slice F2-O4).
    TakeBack,
    /// The away player flies again in a new aircraft, by the revival rules.
    Spawn,
    /// The away player leaves the game.
    LeaveGame,
}

/// Names of the flight views by `--flight-view` number, with their keys.
fn view_name(view: u8) -> &'static str {
    match view {
        0 => "F1 Front",
        1 => "F10 External",
        2 => "Oblique",
        3 => "F2 Back",
        4 => "F3 Up",
        flight_views::TRACK => "F4 Track",
        flight_views::THREAT => "F5 Threat",
        flight_views::WING => "F6 Wing",
        flight_views::TARGET => "F7 Target",
        flight_views::TARGET_PLAYER => "F8 Target view",
        flight_views::FLY_BY => "F9 Fly-by",
        flight_views::MISSILE => "F12 Missile",
        OBJECT => "O Object view",
        tape_views::HUD_CAMERA => "V HUD camera",
        tape_views::SENSOR => "Shift+V Sensor view",
        _ => "View",
    }
}

/// The views the camera button steps through, in F-key order.
const VIEW_ORDER: [u8; 11] = [
    0,
    3,
    4,
    flight_views::TRACK,
    flight_views::THREAT,
    flight_views::WING,
    flight_views::TARGET,
    flight_views::TARGET_PLAYER,
    flight_views::FLY_BY,
    1,
    flight_views::MISSILE,
];

/// The terrain height under an east and north position, feet.
fn ground(world: &Terrain) -> impl Fn(f64, f64) -> f64 + '_ {
    move |x, z| f64::from(world.height(x as f32, z as f32))
}

/// A copy of a camera: the terrain camera is not `Clone`.
fn copy(camera: &Camera) -> Camera {
    let mut out = Camera::new();
    out.weather_slot = camera.weather_slot;
    out.hidden_target = camera.hidden_target;
    out.hidden_projectile = camera.hidden_projectile;
    out.position = camera.position;
    out.yaw = camera.yaw;
    out.pitch = camera.pitch;
    out.roll = camera.roll;
    out.view_fraction = camera.view_fraction;
    out.zoom = camera.zoom;
    out.near_clip = camera.near_clip;
    out.keys = camera.keys.clone();
    out.thermal = camera.thermal;
    out.sensor = camera.sensor;
    out
}

/// The body roll rate between two recorded attitudes a tick apart,
/// radians per second: the part of the turn about the nose. The recording
/// keeps attitudes, not rates, and wing vapor shortens with roll rate.
pub fn roll_rate(before: [f64; 3], after: [f64; 3]) -> f64 {
    let a = Basis::new(before[0], before[1], before[2]);
    let b = Basis::new(after[0], after[1], after[2]);
    // For a small turn, each axis moves by the turn crossed with it, so
    // the sum of each axis crossed with its successor is twice the turn.
    let turn: [f64; 3] = std::array::from_fn(|i| {
        [(a.right, b.right), (a.up, b.up), (a.forward, b.forward)]
            .iter()
            .map(|(p, q)| tore_sim::attitude::cross(*p, *q)[i])
            .sum::<f64>()
            * clock::TICKS_PER_SECOND
            / 2.
    });
    // Banking right turns the aircraft negatively about its nose.
    -tore_sim::attitude::dot(turn, b.forward)
}

/// A name label over an aircraft: its text, the top left of the text on
/// the view, its size there and its colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub id: u32,
    pub text: String,
    pub at: [f64; 2],
    pub size: [f64; 2],
    pub color: [u8; 3],
}

impl Label {
    /// Where it is drawn, as a right-click picks it: left, top, width and
    /// height in view pixels.
    pub fn rect(&self) -> [f64; 4] {
        [self.at[0], self.at[1], self.size[0], self.size[1]]
    }
}

/// Keep a live identity, advance past a missing one, and wrap in stable id order.
fn active_successor(current: Option<u32>, ids: &[u32], advance: bool) -> Option<u32> {
    if !advance && current.is_some_and(|id| ids.contains(&id)) {
        return current;
    }
    current
        .and_then(|id| ids.iter().copied().find(|next| *next > id))
        .or_else(|| ids.first().copied())
}

/// Name labels over `poses` for a view of `size` through `camera`: each
/// aircraft's label in its side's colour, `selected` in brackets. Aircraft
/// over 100 nautical miles away, wrecks on the ground and the aircraft the
/// camera sits in have none. `who` gives an aircraft's label and side.
pub fn name_labels<'a>(
    poses: impl Iterator<Item = &'a crate::snapshot::AircraftPose>,
    camera: &Camera,
    size: [u32; 2],
    font: &tore_formats::font::Font,
    selected: Option<u32>,
    who: &dyn Fn(u32) -> (String, Side),
) -> Vec<Label> {
    let scale = Placement::new(size).scale();
    let eye = camera.position;
    let mut out = Vec::new();
    for pose in poses {
        if Some(pose.id) == camera.hidden_target || (!pose.airborne && pose.crashed) {
            continue;
        }
        let distance = (0..3)
            .map(|i| (pose.position[i] - eye[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        if !(INSIDE..=LABEL_REACH).contains(&distance) {
            continue;
        }
        let Some([x, y]) = camera.project(size, pose.position) else {
            continue;
        };
        let (mut text, side) = who(pose.id);
        text = crate::replay::panels::ascii(&text);
        if Some(pose.id) == selected {
            text = format!("[{text}]");
        }
        let width = FlightCanvas::text_width(font, &text, scale);
        out.push(Label {
            id: pose.id,
            at: [x - width / 2., y - (font.height as f64 + 8.) * scale],
            size: [width, (font.height as f64 + 1.) * scale],
            text,
            color: trails::side_color(side),
        });
    }
    separate_labels(&mut out, scale);
    out
}

/// Lifts a label above any label already placed that it would overprint, so
/// aircraft that sit close together on the view each keep a readable name.
/// Labels keep their order, so the same picture always stacks the same way.
fn separate_labels(labels: &mut [Label], scale: f64) {
    // Names are drawn a little taller than their measured box, so keep a gap.
    let gap = 3. * scale.max(1.);
    for i in 1..labels.len() {
        for _ in 0..16 {
            let [x, y, w, h] = labels[i].rect();
            let blocked = labels[..i].iter().any(|other| {
                let [ox, oy, ow, oh] = other.rect();
                x < ox + ow && ox < x + w && y < oy + oh + gap && oy < y + h + gap
            });
            if !blocked {
                break;
            }
            labels[i].at[1] -= labels[i].size[1] + gap;
        }
    }
}

/// The debug panels and the right-click menu, drawn in their own 640x480
/// layer, with where each is.
struct PanelLayer<'a> {
    pixels: &'a [u8],
    panels: &'a [crate::controls_editor::Rect],
    menu: Option<crate::controls_editor::Rect>,
}

/// Everything drawn over the 3D view in one frame.
struct Interface<'a> {
    hidden: bool,
    /// The 640x480 layer with the transport bar, timer and notices; left
    /// out under the Escape menu, whose buttons sit where the bar is.
    bar: bool,
    labels: &'a [Label],
    model: &'a Model,
    panels: PanelLayer<'a>,
    /// The cockpit messages on screen, in the HUD's font and colour.
    messages: Option<Messages<'a>>,
    /// F7's bearing compass, in the same font and colour, and the bearing.
    compass: Option<(&'a tore_formats::font::Font, [u8; 3], f64)>,
}

/// Cockpit messages drawn as flight draws them, above the transport bar.
struct Messages<'a> {
    lines: &'a [String],
    font: &'a tore_formats::font::Font,
    color: [u8; 3],
}

/// Draws the interface over a blank view of `size`: the labels at the
/// view's own resolution, the debug panels, the 640x480 interface layer,
/// then the right-click menu over everything. While the interface is hidden
/// the view is left clear, so only the 3D picture shows.
fn compose(
    canvas: &mut FlightCanvas,
    size: [u32; 2],
    font: &tore_formats::font::Font,
    layer: &mut [u8],
    interface: &Interface,
) {
    canvas.blank(size);
    if interface.hidden {
        return;
    }
    let scale = Placement::new(size).scale();
    for label in interface.labels {
        canvas.text(font, &label.text, label.at, scale, label.color);
    }
    if let Some((font, color, bearing)) = interface.compass {
        crate::view_compass::draw_clear(canvas, font, color, bearing);
    }
    let panels = &interface.panels;
    canvas.centered_rects(panels.pixels, panels.panels);
    if let Some(messages) = &interface.messages {
        crate::flight_ui::draw_messages(
            canvas,
            messages.font,
            messages.color,
            messages.lines.iter().map(String::as_str),
            f64::from(
                overlay::HEIGHT as i32 - overlay::BAR_TOP
                    + overlay::banner_height(font, interface.model),
            ),
        );
    }
    if interface.bar {
        layer.fill(0);
        overlay::draw(layer, font, interface.model);
        canvas.anchored_layer(layer);
    }
    if let Some(menu) = panels.menu {
        canvas.centered_rects(panels.pixels, &[menu]);
    }
}

/// Every aircraft in `picture`: the plane the recording is for first, then
/// the others. A recording with no player (an observer's) has only the
/// others, so its empty player pose is never taken for an aircraft.
fn everyone(picture: &RenderSnapshot) -> impl Iterator<Item = &crate::snapshot::AircraftPose> {
    (picture.player.id != convert::NO_PLAYER)
        .then_some(&picture.player)
        .into_iter()
        .chain(&picture.targets)
}

/// Straight-line feet between two points.
fn feet(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// An aircraft's label for people: its label, else its type, else its id.
fn label_of(info: &BTreeMap<u32, AircraftInfo>, id: u32) -> String {
    info.get(&id).map_or_else(
        || format!("Aircraft {id}"),
        |a| {
            if !a.label.is_empty() {
                a.label.clone()
            } else if !a.name.is_empty() {
                a.name.clone()
            } else {
                format!("Aircraft {id}")
            }
        },
    )
}

/// Each missile's shooter and weapon name, from the launch events.
fn missiles(recording: &Recording) -> BTreeMap<u32, (u32, String)> {
    recording
        .events()
        .iter()
        .filter(|e| e.event.kind == vocab::kind::WEAPON_LAUNCH)
        .filter_map(|e| {
            let projectile = e.event.id(vocab::field::PROJECTILE)?;
            let shooter = e.event.subject?;
            let weapon = e
                .event
                .id(vocab::field::WEAPON)
                .and_then(|w| recording.weapon_info(w))
                .filter(|w| !w.name.is_empty())
                .map_or_else(
                    || "Missile".to_owned(),
                    |w| {
                        tore_formats::weapons::corrections::identity(&w.source)
                            .map_or_else(|| w.name.clone(), |(_, long)| long.into())
                    },
                );
            Some((projectile, (shooter, weapon)))
        })
        .collect()
}

/// What a replay lends the debug panels: the recording at the playhead.
struct ReplayData<'a> {
    recording: &'a Recording,
    trees: &'a mut RecordedTrees,
    info: &'a BTreeMap<u32, AircraftInfo>,
    missiles: &'a BTreeMap<u32, (u32, String)>,
    comms: &'a [TimedEvent],
    now: u64,
    /// The plane the recording is for.
    player: u32,
}

impl Data for ReplayData<'_> {
    fn now(&self) -> u64 {
        self.now
    }

    fn tree(&mut self, subject: u32, channel: &str) -> Option<(u64, TreeSample)> {
        self.trees.tree(self.recording, subject, channel, self.now)
    }

    fn name(&self, id: u32) -> String {
        label_of(self.info, id)
    }

    fn title(&self, kind: Kind, subject: u32) -> String {
        match kind {
            Kind::Guidance => match self.missiles.get(&subject) {
                Some((owner, weapon)) => format!("{weapon} from {}", self.name(*owner)),
                None => format!("missile {subject}"),
            },
            Kind::Thought | Kind::Telemetry => match self.info.get(&subject) {
                Some(a) if !a.name.is_empty() && !a.label.is_empty() => {
                    format!("{}  {}", a.label, a.name)
                }
                _ => self.name(subject),
            },
        }
    }

    fn missing(&self, kind: Kind, subject: u32) -> String {
        let who = self.name(subject);
        match kind {
            Kind::Thought
                if subject == self.player || self.info.get(&subject).is_some_and(|a| a.human) =>
            {
                format!("{who} is flown by a person, so there is no AI thinking to show.")
            }
            Kind::Thought => format!("No AI thinking recorded for {who} up to this moment."),
            Kind::Telemetry => format!("No telemetry recorded for {who} up to this moment."),
            Kind::Guidance => "No guidance recorded for this missile up to this moment.".into(),
        }
    }

    fn comms(&self) -> (&[TimedEvent], usize) {
        let end = self.comms.partition_point(|e| e.tick <= self.now);
        (self.comms, end)
    }

    fn aircraft(&self) -> Vec<u32> {
        self.info.keys().copied().collect()
    }
}

/// What took the left button's press, so its release goes there too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Menu,
    Panels,
    Bar,
}

/// Where a screenshot of `recording` at `tick` goes in `folder`: the
/// recording's name and the tick, numbered on when that file exists.
fn screenshot_path(folder: &Path, recording: &Path, tick: u64) -> PathBuf {
    let name = recording
        .file_name()
        .map_or_else(|| "replay".to_owned(), |n| n.to_string_lossy().into_owned());
    let stem = name
        .trim_end_matches(".partial")
        .trim_end_matches(".tore-replay");
    let mut path = folder.join(format!("{stem}-tick{tick:07}.png"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem}-tick{tick:07}-{n}.png"));
        n += 1;
    }
    path
}

/// A cockpit message the HUD showed or queued, from the HUD's own record:
/// not the communication journal's copy of an AI text line, which names its
/// source, nor a message the HUD never showed.
fn hud_shown(event: &Event) -> bool {
    use vocab::{field, outcome};
    event.get(field::SOURCE).is_none()
        && event
            .string(field::OUTCOME)
            .is_none_or(|o| o == outcome::DELIVERED || o == outcome::QUEUED)
}

/// The cockpit message lines `entries` leave on screen, oldest first,
/// rebuilt as flight keeps them: a repeat moves to the bottom with a fresh
/// start, and at most seven lines show. Pass the last five seconds' entries.
fn message_lines(entries: &[TimedEvent]) -> Vec<String> {
    let mut lines: std::collections::VecDeque<String> = Default::default();
    for entry in entries {
        let event = &entry.event;
        if event.kind != vocab::kind::COMMS_HUD || !hud_shown(event) || event.text.trim().is_empty()
        {
            continue;
        }
        lines.retain(|line| *line != event.text);
        lines.push_back(event.text.clone());
        while lines.len() > crate::flight_ui::MESSAGE_LINES {
            lines.pop_front();
        }
    }
    lines.into()
}

/// The events the timeline marks: launches, kills, orders (an AI strike
/// decision among them, slice ST-G) and bookmarks.
fn markers(events: &[TimedEvent]) -> Vec<Marker> {
    events
        .iter()
        .filter_map(|e| {
            let kind = match e.event.kind.as_str() {
                vocab::kind::WEAPON_LAUNCH => MarkerKind::Launch,
                vocab::kind::COMBAT_DESTROYED => MarkerKind::Kill,
                vocab::kind::COMMS_ORDER | vocab::kind::AI_STRIKE => MarkerKind::Order,
                vocab::kind::PLAYER_BOOKMARK => MarkerKind::Bookmark,
                tore_replay::feedback::KIND => MarkerKind::Feedback,
                _ => return None,
            };
            Some(Marker {
                tick: tore_replay::feedback::Feedback::from_event(&e.event)
                    .map_or(e.tick, |f| f.tick),
                kind,
            })
        })
        .collect()
}

/// Each aircraft's recorded target over time: the tick and the target,
/// `None` once it has none. AI aircraft have their target changes. The
/// player's is the view target live flight's target views followed, sight
/// hold included; a recording from before those were recorded has the
/// player's designation, noted with every command the player gives.
pub(super) fn targets(
    events: &[TimedEvent],
    player: u32,
) -> BTreeMap<u32, Vec<(u64, Option<u32>)>> {
    let viewed = events
        .iter()
        .any(|e| e.event.kind == vocab::kind::PLAYER_VIEW_TARGET);
    let mut out: BTreeMap<u32, Vec<(u64, Option<u32>)>> = BTreeMap::new();
    for e in events {
        let Some(subject) = e.event.subject else {
            continue;
        };
        let target = match e.event.kind.as_str() {
            vocab::kind::PLAYER_VIEW_TARGET => e.event.object,
            // With view targets recorded, they alone give the player's.
            _ if viewed && subject == player => continue,
            vocab::kind::AI_TARGET => e.event.object.or_else(|| e.event.id(vocab::field::TO)),
            vocab::kind::PLAYER_COMMAND => e.event.object,
            _ => continue,
        };
        out.entry(subject).or_default().push((e.tick, target));
    }
    out
}

/// The building of `fill` standing at each of `places` (east, north, whole
/// feet), for a recording's knocked-down city buildings (nr-fx3). A place
/// with no building in this fill is left out.
pub(crate) fn city_index(
    fill: &tore_world::city::CityFill,
    places: &[[i32; 2]],
) -> std::collections::HashMap<[i32; 2], u32> {
    let wanted: std::collections::HashSet<[i32; 2]> = places.iter().copied().collect();
    let mut out = std::collections::HashMap::with_capacity(wanted.len());
    if wanted.is_empty() {
        return out;
    }
    for (n, i) in fill.layout.instances.iter().enumerate() {
        let at = [i.x, i.z];
        if wanted.contains(&at) {
            out.entry(at).or_insert(n as u32);
        }
    }
    out
}

pub struct Viewer {
    pub mission_timer: Option<crate::mission_timer::Timer>,
    local_lighting: (crate::cockpit_lighting::Lighting, i16),
    pub path: PathBuf,
    recording: Arc<Recording>,
    pub world: Terrain,
    /// The art and camera weather of `world`.
    pub scenery: Scenery,
    /// The recorded player's aircraft, drawn in the renderer's ownship slot.
    pub ownship: Airframe,
    /// Models of the other aircraft, in the recording's draw order.
    models: Vec<Airframe>,
    art: CombatArt,
    /// A start state for the player's airframe; every drawn player state
    /// is rebuilt over it.
    template: flight::State,
    scratch: flight::State,
    /// AC-130 muzzle flashes: the gunship rounds each picture showed.
    gun_flash: crate::gun_flash::Tracker,
    playback: Playback,
    /// The city fill (by address) and the count of recorded city changes
    /// it was last set to (nr-fx3), so the fill is set only when a seek or
    /// the clock crosses a change.
    city_shown: Option<(usize, usize)>,
    /// The fill's building at each place the recording knocks one down.
    city_index: std::collections::HashMap<[i32; 2], u32>,
    /// The tick each flat building fell on, for the burning city's smoke
    /// columns (nr-fx4); `None` while the city is not followed.
    city_falls: Option<Arc<crate::city_fires::Falls>>,
    /// The plane the recording is for (`draw.player`, plane 0 in single
    /// player's), or [`convert::NO_PLAYER`] for an observer's.
    player: u32,
    tracks: Tracks,
    scanner: Scanner,
    weather: WeatherTrack,
    /// Released chaff and flares, flown again from their releases.
    devices: DeviceTrack,
    /// The recording's laser spots, designations and guided weapons.
    marks: ground_marks::Marks,
    /// Where each airframe's engine outlets sit, for the afterburner
    /// lights: the player's airframe's, then each model's.
    outlets: (Vec<[f64; 3]>, Vec<Vec<[f64; 3]>>),
    /// Voices, tones and effects at normal speed.
    sound: ReplaySound,
    pub clock: Clock,
    info: BTreeMap<u32, AircraftInfo>,
    feedback_flags: Vec<tore_replay::feedback::Feedback>,
    feedback_open: bool,
    feedback_selected: usize,
    feedback_scroll: usize,
    markers: Vec<Marker>,
    /// Unique marker ticks, for PageUp and PageDown.
    marker_ticks: Vec<u64>,
    targets: BTreeMap<u32, Vec<(u64, Option<u32>)>>,
    /// Every comms and audio entry, in time order, for the Comms panel.
    comms: Vec<TimedEvent>,
    /// Each missile's shooter and weapon name.
    missiles: BTreeMap<u32, (u32, String)>,
    panels: Panels,
    trees: RecordedTrees,
    banner: super::banner::Inspector,
    drone_subject: Option<Target>,
    menu: Option<Menu>,
    /// A right-button press being watched for a click, and where it went
    /// down in view pixels.
    right_click: Option<(RightClick, [f64; 2])>,
    /// Where the pointer is in the panel layer, while the interface shows.
    point: Option<(f64, f64)>,
    left_owner: Option<Owner>,
    /// What the last frame drew that a right-click can pick.
    pickables: Vec<Pickable>,
    /// The debug panels' and menu's own layer.
    panel_layer: Vec<u8>,
    /// Panels asked for at start, opened on the first frame.
    requests: Vec<Request>,
    /// The view's size in the last frame.
    size: [u32; 2],
    /// Where the debug panels go: clear of the cockpit messages above the
    /// transport bar.
    layout: panels::Layout,
    view: u8,
    drone: Option<Drone>,
    /// Where the selected aircraft was last drawn, so a follow drone holds
    /// its place while the aircraft is missing from the recording.
    anchor: Option<[f64; 3]>,
    rig: Rig,
    selected: u32,
    /// Stable projectile identity, never an index into a changing list.
    selected_missile: Option<u32>,
    /// First 100 m crossing by weapon, indexed lazily from recorded history.
    weapon_stops: BTreeMap<u32, WeaponApproach>,
    /// Where the object view starts from when it is not the selected
    /// aircraft: a ground object or a weapon chosen with View from here.
    from: Option<Target>,
    /// What the object view looks at.
    look_at: Option<Target>,
    /// F6 was pressed: name the wingman once the camera has found it.
    announce_wingman: bool,
    /// The bearing F7's compass shows this frame, when it shows.
    compass: Option<f64>,
    /// The object view's names and range this frame.
    readout: Option<String>,
    look: [f32; 2],
    zoom: f32,
    camera: Camera,
    /// A frame has been drawn, so `camera` is one the viewer chose.
    shown: bool,
    camera_error: Option<&'static str>,
    pub ui: Ui,
    /// The Escape menu, and the play state it paused while open.
    pause: pause::Menu,
    /// Held drone movement keys.
    held: BTreeSet<String>,
    drone_input: crate::replay::drone::Controls,
    bar: overlay::Pointer,
    /// Where a right-drag last was, in window pixels.
    dragging: Option<[f64; 2]>,
    toast: Option<(String, Instant)>,
    layer: Vec<u8>,
    /// The renderer holds this viewer's world and models.
    entered: bool,
    last_frame: Option<Instant>,
    /// Set while the recording grows as it plays (an observer's).
    growth: Option<Growth>,
    /// Each aircraft's type, by its recorded id.
    kinds: BTreeMap<u32, AircraftId>,
    /// The targeting sight of each aircraft type's own sensor records, read
    /// from the import (the Sensor View, nr-replayviews).
    sights: Vec<(
        AircraftId,
        Option<(String, tore_sim::combat::sight::SightSensor)>,
    )>,
    /// Each aircraft type's gun, for the HUD camera's pipper.
    guns: Vec<(AircraftId, tore_formats::weapons::Weapon, [f64; 3])>,
    /// The Sensor View's picture: white hot, black hot, TV or night vision.
    sensor_mode: crate::sensor_console::SensorMode,
    /// Each aircraft's weapon bursts on the ground (`combat.ground_impact`)
    /// in tick order: where an AC-130's or a strafer's rounds went.
    ground_impacts: BTreeMap<u32, Vec<(u64, [f64; 3])>>,
    /// Recorded HUD and sight samples (nr-replayhud). Empty on an older file.
    display: tore_replay::display::Track,
    /// Weapons named by `aircraft.hud`, plus the AC-130's three guns.
    hud_weapons: BTreeMap<String, tore_formats::weapons::Weapon>,
    /// The HUD camera's tape: which frames make a picture, and the last one.
    tape: views::TapeState,
    /// One combat configuration per drawn aircraft type, for the HUD.
    hud_configs: Vec<(
        tore_formats::aircraft::AircraftId,
        tore_sim::combat::live::Configuration,
    )>,
}

/// How much of a growing recording can be scrubbed: the last ten minutes
/// (the design's rule for the observer view). Older frames stay in the file
/// and are never shown.
pub const LIVE_WINDOW_TICKS: u64 = 10 * 60 * 120;

/// What a viewer on a growing recording keeps beyond a finished one's.
struct Growth {
    /// The import, for the art of weapons that appear later.
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    /// The weapon shapes already loaded.
    shapes: BTreeSet<String>,
    /// How many events the viewer has read, to read them again when more
    /// come.
    events: usize,
}

impl Viewer {
    pub(crate) fn loading_eye(&mut self) -> [f64; 3] {
        self.playback
            .frame(self.clock.tick())
            .and_then(|(frames, at)| {
                frames[at]
                    .aircraft
                    .iter()
                    .find(|a| a.id == self.selected)
                    .map(|a| a.position)
            })
            .unwrap_or(self.template.position)
    }
    pub(crate) fn loading_art(&self) -> &CombatArt {
        &self.art
    }
    pub(crate) fn loading_models(&self) -> &[Airframe] {
        &self.models
    }
    pub(crate) fn loading_entered(&mut self) {
        self.entered = true;
    }

    /// Opens a recording and loads what drawing it needs: its world, the
    /// player's airframe, the other aircraft models and the weapon art.
    pub fn open(
        path: &Path,
        resources: &BTreeMap<String, Vec<u8>>,
        options: &Options,
    ) -> AppResult<Self> {
        let recording =
            Arc::new(Recording::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        for problem in recording.problems() {
            log::warn!("Replay {}: {problem}", path.display());
        }
        Self::load(path, recording, resources, options, None)
    }

    /// Opens the live view of a mission being flown: `recording` is the
    /// first read of the observer's growing recording, and [`Viewer::grow`]
    /// is given each newer read of it. The playhead follows the newest frame.
    pub fn open_live(
        recording: Arc<Recording>,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        options: &Options,
    ) -> AppResult<Self> {
        let growth = Growth {
            resources: Arc::clone(&resources),
            shapes: BTreeSet::new(),
            events: 0,
        };
        Self::load(
            Path::new("observer"),
            recording,
            &resources,
            options,
            Some(growth),
        )
    }

    fn load(
        path: &Path,
        recording: Arc<Recording>,
        resources: &BTreeMap<String, Vec<u8>>,
        options: &Options,
        growth: Option<Growth>,
    ) -> AppResult<Self> {
        if recording.first_tick().is_none() {
            return Err(format!("{}: the recording holds no frames", path.display()).into());
        }
        let world = crate::replay::identity::terrain(resources, &recording.header().world)?;
        let scenery = Scenery::build(resources, &world)?;
        let identities = crate::replay::convert::Identities::of(&recording);
        let presentation = crate::replay::convert::Presentation::from_header(recording.header());
        let player = match identities.aircraft.get(&presentation.player) {
            Some(id) => *id,
            None => {
                if presentation.player != convert::NO_PLAYER {
                    log::warn!("Replay: the recording has no player aircraft; drawing an F/A-18D");
                }
                AircraftId::F18
            }
        };
        let ownship = Airframe::load(resources, player)?;
        let mut models: Vec<Airframe> = Vec::new();
        for id in &presentation.models {
            if !models.iter().any(|m| m.profile.id == *id) {
                models.push(Airframe::load(resources, *id)?);
            }
        }
        let mut art = CombatArt::load(resources)?;
        art.add_shapes(super::store_art::shapes(&recording), resources);
        let mut viewer = Self::assemble(
            path,
            recording,
            (world, scenery),
            (ownship, models, art),
            options,
            growth,
        )?;
        viewer.local_lighting = crate::assets::data_directory()
            .ok()
            .and_then(|path| crate::preferences::read(&path.join("preferences-v1.conf")).ok())
            .and_then(|text| crate::preferences::Preferences::parse(&text).ok())
            .map_or((Default::default(), 0), |prefs| {
                (prefs.lighting, prefs.brightness)
            });
        viewer.playback.add_motors(resources);
        viewer.load_sights(resources);
        // Other aircraft's launches name their weapon record.
        viewer
            .sound
            .set_launches(crate::audio::LaunchSounds::of(resources));
        Ok(viewer)
    }

    /// A viewer from loaded parts.
    fn assemble(
        path: &Path,
        recording: Arc<Recording>,
        (world, scenery): (Terrain, Scenery),
        (ownship, models, art): (Airframe, Vec<Airframe>, CombatArt),
        options: &Options,
        growth: Option<Growth>,
    ) -> AppResult<Self> {
        let (Some(first), Some(last)) = (recording.first_tick(), recording.last_tick()) else {
            return Err(format!("{}: the recording holds no frames", path.display()).into());
        };
        let playback = Playback::new(Arc::clone(&recording));
        let player = playback.presentation.player;
        let template = ownship.start(&world);
        let mut clock = if growth.is_some() {
            Clock::new_live(first.max(last.saturating_sub(LIVE_WINDOW_TICKS)), last)
        } else {
            Clock::new(first, last)
        };
        if let Some(speed) = options.speed {
            if speed < 0. {
                clock.end();
            }
            if !clock.set_speed(speed) {
                return Err(format!(
                    "{speed} is not a replay speed: use 0.125, 0.25, 0.5, 0.75, 1, 2, 4, 8 or 16, negative for reverse"
                )
                .into());
            }
        }
        if let Some(tick) = options.tick {
            clock.seek(tick as f64);
            if options.speed.is_none() {
                clock.pause();
            }
        }
        let info: BTreeMap<u32, AircraftInfo> =
            recording.aircraft().map(|a| (a.id, a.clone())).collect();
        // The recorded player is watched first; an observer's recording has
        // none, and starts on its first aircraft.
        let selected = options
            .aircraft
            .or_else(|| (player != convert::NO_PLAYER).then_some(player))
            .or_else(|| info.keys().next().copied())
            .unwrap_or(0);
        if options.aircraft.is_some() && !info.contains_key(&selected) {
            return Err(format!("the recording has no aircraft {selected}").into());
        }
        let mut rig = Rig::for_plane(player);
        rig.select(Reference::Aircraft(selected));
        let pause = if growth.is_some() {
            pause::Menu::watching(&ownship.flight_menu)
        } else {
            pause::Menu::new(&ownship.flight_menu)
        };
        let mut viewer = Self {
            mission_timer: None,
            local_lighting: (Default::default(), 0),
            path: path.to_path_buf(),
            scanner: if growth.is_some() {
                Scanner::start_live(Arc::clone(&recording))
            } else {
                Scanner::start(Arc::clone(&recording))
            },
            tracks: Tracks::new(first),
            // A headless probe never steps the weather, so its recording
            // keeps the launch sky throughout.
            weather: WeatherTrack::new(
                &world,
                &scenery,
                if recording.header().mission == tore_replay::MissionKind::Probe {
                    0
                } else {
                    last
                },
            ),
            sound: ReplaySound::new(
                Arc::clone(&recording),
                std::iter::once(&ownship.profile).chain(models.iter().map(|m| &m.profile)),
            ),
            // Read from the events by `read_events` below.
            targets: BTreeMap::new(),
            missiles: BTreeMap::new(),
            devices: DeviceTrack::new(&[]),
            marks: ground_marks::Marks::default(),
            outlets: (
                ownship.contrail_offsets.clone(),
                models
                    .iter()
                    .map(|model| model.contrail_offsets.clone())
                    .collect(),
            ),
            recording,
            world,
            scenery,
            scratch: template.clone(),
            gun_flash: Default::default(),
            template,
            ownship,
            models,
            art,
            playback,
            city_shown: None,
            city_index: std::collections::HashMap::new(),
            city_falls: None,
            player,
            clock,
            info,
            feedback_flags: Vec::new(),
            feedback_open: false,
            feedback_selected: 0,
            feedback_scroll: 0,
            markers: Vec::new(),
            marker_ticks: Vec::new(),
            comms: Vec::new(),
            panels: Panels::default(),
            trees: RecordedTrees::default(),
            banner: super::banner::Inspector::default(),
            drone_subject: None,
            menu: None,
            right_click: None,
            point: None,
            left_owner: None,
            pickables: Vec::new(),
            panel_layer: vec![0; overlay::WIDTH * overlay::HEIGHT * 4],
            requests: options.panels.clone(),
            size: [overlay::WIDTH as u32, overlay::HEIGHT as u32],
            layout: panels::REPLAY,
            view: if options.look_at.is_some() {
                OBJECT
            } else {
                options.view.unwrap_or(EXTERNAL)
            },
            drone: None,
            anchor: None,
            rig,
            selected,
            selected_missile: None,
            weapon_stops: BTreeMap::new(),
            from: None,
            look_at: options.look_at,
            announce_wingman: false,
            compass: None,
            readout: None,
            look: [0.; 2],
            zoom: 1.,
            camera: Camera::new(),
            shown: false,
            camera_error: None,
            ui: options.ui,
            pause,
            held: BTreeSet::new(),
            drone_input: crate::replay::drone::Controls::new(&tore_input::Profile::default()),
            bar: overlay::Pointer::default(),
            dragging: None,
            toast: None,
            layer: vec![0; overlay::WIDTH * overlay::HEIGHT * 4],
            entered: false,
            last_frame: None,
            growth,
            kinds: BTreeMap::new(),
            sights: Vec::new(),
            guns: Vec::new(),
            sensor_mode: Default::default(),
            ground_impacts: BTreeMap::new(),
            display: tore_replay::display::Track::default(),
            hud_weapons: BTreeMap::new(),
            tape: Default::default(),
            hud_configs: Vec::new(),
        };
        viewer.read_events();
        if options.drone {
            viewer.drone_mode(Some(Mode::Follow));
        }
        if options.ui.comms {
            viewer.panels.open_comms(None);
        }
        if let Some(start) = options.menu {
            viewer.open_pause();
            viewer.pause.open_at(start, &mut viewer.clock);
        }
        Ok(viewer)
    }

    /// What the events say, read from the recording: the timeline's markers,
    /// the Comms panel's entries, each aircraft's target, the weapons and
    /// the chaff and flares. Read again when a growing recording has more.
    fn read_events(&mut self) {
        let events = self.recording.events();
        self.feedback_flags = self.recording.feedback().collect();
        self.markers = markers(events);
        self.markers.sort_by_key(|m| m.tick);
        self.marker_ticks = self.markers.iter().map(|m| m.tick).collect();
        self.marker_ticks.dedup();
        self.comms = events
            .iter()
            .filter(|e| panels::Channel::of(&e.event.kind).is_some())
            .cloned()
            .collect();
        self.targets = targets(events, self.player);
        self.missiles = missiles(&self.recording);
        self.devices = DeviceTrack::new(events);
        self.marks = ground_marks::Marks::from_events(events);
        self.kinds = convert::Identities::of(&self.recording).aircraft;
        self.ground_impacts = BTreeMap::new();
        for e in events
            .iter()
            .filter(|e| e.event.kind == vocab::kind::COMBAT_GROUND_IMPACT)
        {
            if let (Some(id), Some(x), Some(y), Some(z)) = (
                e.event.subject,
                e.event.num("x_ft"),
                e.event.num("y_ft"),
                e.event.num("z_ft"),
            ) {
                self.ground_impacts
                    .entry(id)
                    .or_default()
                    .push((e.tick, [x, y, z]));
            }
        }
        self.display = tore_replay::display::Track::from_events(events);
        if let Some(growth) = &mut self.growth {
            growth.events = events.len();
        }
    }

    /// What a live view's camera is on, for the host to send what is near
    /// it at the full rate: the selected aircraft, or the point the camera
    /// is at when it is free of every aircraft (the free drone, or the
    /// selected aircraft is not in the mission).
    pub fn camera_subject(&self) -> tore_session::wire::messages::Subject {
        use tore_session::wire::messages::Subject;
        let free = self.drone.as_ref().is_some_and(|d| d.mode == Mode::Free);
        if !free && self.info.contains_key(&self.selected) {
            return Subject::Aircraft(self.selected);
        }
        if self.shown {
            Subject::Point(self.camera.position.map(|feet| feet as i32))
        } else {
            Subject::None
        }
    }

    /// The menu of the observer screen of an away player's own aircraft
    /// (slice F2-O4): what its first rows offer now, or `None` for an
    /// observer's Stop Watching.
    pub fn set_away_menu(&mut self, rows: Option<pause::AwayRows>) {
        self.pause.set_away(rows);
    }

    /// Whether the view is of a mission being flown, a recording that grows
    /// as it plays.
    pub fn live(&self) -> bool {
        self.growth.is_some() && self.clock.is_live()
    }

    /// The recording this view reads has grown: `recording` is a newer read
    /// of it (the same frames, then more). The playhead keeps its place; a
    /// live one stays a second behind the newest frame. Frames older than
    /// ten minutes are no longer scrubbable.
    pub fn grow(&mut self, recording: Arc<Recording>) {
        let (Some(first), Some(last)) = (recording.first_tick(), recording.last_tick()) else {
            return;
        };
        if self.growth.is_none() || last <= self.recording.last_tick().unwrap_or(0) {
            return;
        }
        self.playback.extend(Arc::clone(&recording));
        if let Some(growth) = &self.growth {
            self.playback.add_motors(&growth.resources);
        }
        self.sound.extend(Arc::clone(&recording));
        self.scanner.feed(Arc::clone(&recording));
        self.weather.set_end(last);
        self.clock
            .grow(first.max(last.saturating_sub(LIVE_WINDOW_TICKS)), last);
        self.info = recording.aircraft().map(|a| (a.id, a.clone())).collect();
        if let Some(growth) = &mut self.growth {
            let new: Vec<&str> = super::store_art::shapes(&recording)
                .filter(|shape| growth.shapes.insert((*shape).to_owned()))
                .collect();
            self.art.add_shapes(new, &growth.resources);
        }
        let more = self
            .growth
            .as_ref()
            .is_some_and(|g| g.events != recording.events().len());
        self.recording = recording;
        if more {
            self.read_events();
        }
    }

    /// The window title.
    pub fn title(&self) -> String {
        if self.growth.is_some() {
            return "T.O.R.E-Fighters - Observing".into();
        }
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        format!("T.O.R.E-Fighters - Replay {name}")
    }

    /// Points the renderer at this viewer's world and the player's airframe.
    /// The world rebuild discards the renderer's aircraft, so the ownship
    /// slot is prepared after it.
    pub fn enter(&mut self, renderer: &mut Renderer) {
        if !self.entered {
            renderer.set_scenery(&self.scenery);
            renderer.prepare_aircraft(&self.ownship);
            self.entered = true;
        }
    }

    /// The renderer holds this viewer's world and aircraft.
    pub fn entered(&self) -> bool {
        self.entered
    }

    /// Waits for the background pass over the recording, so trails, fallen
    /// buildings and the weather are complete: for captures.
    pub fn finish_tracks(&mut self) {
        self.scanner.wait(&mut self.tracks);
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Shows a message along the top of the view for a few seconds.
    pub fn message(&mut self, text: impl Into<String>) {
        self.toast(text);
    }

    fn label(&self, id: u32) -> String {
        label_of(&self.info, id)
    }

    fn side(&self, id: u32) -> Side {
        if id == self.player {
            return self.info.get(&id).map_or(Side::Friendly, |a| match a.side {
                Side::Unknown => Side::Friendly,
                side => side,
            });
        }
        self.info.get(&id).map_or(Side::Unknown, |a| a.side)
    }

    /// Aircraft recorded on the current tick, in id order.
    fn present(&mut self) -> Vec<u32> {
        let tick = self.clock.tick();
        self.playback
            .frame(tick)
            .map(|(frames, at)| frames[at].aircraft.iter().map(|a| a.id).collect())
            .unwrap_or_default()
    }

    /// Tab and Shift+Tab: the next or previous aircraft on the current tick.
    pub fn cycle_aircraft(&mut self, forward: bool) {
        let ids = self.present();
        if ids.is_empty() {
            return;
        }
        let next = if forward {
            ids.iter()
                .copied()
                .find(|id| *id > self.selected)
                .unwrap_or(ids[0])
        } else {
            ids.iter()
                .copied()
                .rev()
                .find(|id| *id < self.selected)
                .unwrap_or(ids[ids.len() - 1])
        };
        self.select(next);
    }

    /// Selects an aircraft, which the object view then starts from too. An
    /// Alt or Ctrl reference carries over to the new aircraft's target or
    /// missiles.
    fn select(&mut self, id: u32) {
        self.from = None;
        self.drone_subject = None;
        if id != self.selected {
            self.selected = id;
            self.anchor = None;
            let reference = match self.rig.reference {
                Reference::Target => Reference::Target,
                Reference::Missile => {
                    self.rig.follow_missiles_of(id);
                    Reference::Missile
                }
                Reference::Player | Reference::Aircraft(_) => Reference::Aircraft(id),
            };
            self.rig.select(reference);
            self.camera_error = None;
            self.look = [0.; 2];
            self.panels.follow(id);
            if self.view == flight_views::MISSILE && reference != Reference::Aircraft(id) {
                self.show_view(self.view, reference);
            }
        }
    }

    /// F1 to F12: a flight view on the selected aircraft, leaving the drone.
    pub fn set_view(&mut self, view: u8) {
        if view == flight_views::MISSILE {
            let ids = self.active_missiles(self.clock.tick());
            let advance = self.view == view
                && self.drone.is_none()
                && self.rig.reference == Reference::Aircraft(self.selected);
            if advance && ids.is_empty() {
                self.show_view(0, Reference::Aircraft(self.selected));
                return;
            }
            self.selected_missile = active_successor(self.selected_missile, &ids, advance);
        }
        self.show_view(view, Reference::Aircraft(self.selected));
    }

    /// A flight view from `reference`: the selected aircraft, its target
    /// (Alt) or its newest missile (Ctrl), as flight's modifiers choose.
    /// F6 again on the same reference moves to the next wingman.
    fn show_view(&mut self, view: u8, reference: Reference) {
        let again = view == flight_views::WING
            && self.view == flight_views::WING
            && self.drone.is_none()
            && self.rig.reference == reference;
        self.drone = None;
        self.view = view;
        self.look = [0.; 2];
        self.zoom = 1.;
        self.camera_error = None;
        if reference == Reference::Missile {
            self.rig.follow_missiles_of(self.selected);
        }
        if again {
            self.rig.next_wingman();
        } else {
            // The fly-by point is chosen afresh each time the view is picked.
            self.rig.select(reference);
        }
        if view == flight_views::MISSILE && reference != Reference::Aircraft(self.selected) {
            let tick = self.clock.tick();
            let owner = match reference {
                Reference::Target => self.view_target(tick),
                Reference::Aircraft(id) => Some(id),
                _ => Some(self.selected),
            };
            let picture = self.playback.picture(tick, 1.);
            let active = picture
                .projectiles
                .iter()
                .filter(|p| Some(p.owner) == owner && !p.gun)
                .map(|p| p.id)
                .max();
            self.selected_missile =
                active.or_else(|| owner.and_then(|owner| self.newest_weapon(tick, owner)));
        }
        self.announce_wingman = view == flight_views::WING;
    }

    /// Alt or Ctrl with a view key, as in flight: Alt looks from the selected
    /// aircraft's target, Ctrl from its newest missile. True when taken.
    pub fn view_key(&mut self, name: &str, alt: bool, control: bool) -> bool {
        // The Escape menu takes every key while it is open.
        if alt == control || self.pause.is_open() {
            return false;
        }
        let Some(view) = flight_views::key(name) else {
            return false;
        };
        self.menu = None;
        self.show_view(
            view,
            if alt {
                Reference::Target
            } else {
                Reference::Missile
            },
        );
        true
    }

    /// The object view on what it looks at now, leaving the drone.
    fn object_view(&mut self) {
        if self.view != OBJECT || self.drone.is_some() {
            self.drone = None;
            self.view = OBJECT;
            self.look = [0.; 2];
            self.zoom = 1.;
        }
        self.camera_error = None;
    }

    /// Where the object view starts from: the selected aircraft unless View
    /// from here chose a ground object or a weapon.
    fn from(&self) -> Target {
        self.from.unwrap_or(Target::Aircraft(self.selected))
    }

    /// The launcher rails as the recording had them at `tick`, in the shape
    /// the scenery draws them from (format 3).
    /// Shows the city fill as the recording left it at `tick` (nr-fx3): its
    /// knocked-down buildings flat, and so burning, whether the clock got
    /// there playing, seeking or playing backwards. A recording names each
    /// building by where it stands; the rebuilt fill's building there falls,
    /// and a place where it has none is skipped. A recording from before
    /// the city section keeps the city whole.
    pub(super) fn follow_city(&mut self, tick: u64) {
        let Some(fill) = &self.world.city_fill else {
            return;
        };
        // A developer aid: replay as a build from before the city section.
        if std::env::var("TORE_REPLAY_CITY").as_deref() == Ok("0") {
            return;
        }
        let address = std::sync::Arc::as_ptr(fill) as usize;
        let key = (address, self.playback.city_changes_at(tick));
        if self.city_shown == Some(key) {
            return;
        }
        if self.city_shown.is_none_or(|(at, _)| at != address) {
            self.city_index = city_index(fill, &self.playback.city_all());
        }
        self.city_shown = Some(key);
        let mut ids: Vec<u32> = self
            .playback
            .city_at(tick)
            .iter()
            .filter_map(|at| self.city_index.get(at).copied())
            .collect();
        ids.sort_unstable();
        fill.set_flattened(&ids);
        // Each flat building burns from the tick it fell on, so a seek
        // shows its smoke column as tall as playing through would have.
        let fell = self
            .playback
            .city_falls_at(tick)
            .into_iter()
            .filter_map(|(at, when)| Some((*self.city_index.get(&at)?, when as f64)))
            .collect();
        self.city_falls = Some(Arc::new(crate::city_fires::Falls::new(fell)));
    }

    /// The burning city's clock at the playhead `now` (ticks): see
    /// [`crate::city_fires::Clock`]. `None` while the city is not followed.
    pub(super) fn city_clock(&self, now: f64) -> Option<crate::city_fires::Clock> {
        // A developer aid: the columns age on the wall clock from the moment
        // each is first seen, as they did before nr-fx4.
        if std::env::var("TORE_REPLAY_CITY_CLOCK").as_deref() == Ok("0") {
            return None;
        }
        self.city_falls
            .as_ref()
            .map(|falls| crate::city_fires::Clock {
                tick: now,
                falls: Arc::clone(falls),
            })
    }

    pub(super) fn rails(&self, tick: u64) -> tore_world::surface::SurfaceState {
        use tore_world::surface::{MountStock, SurfaceState, SurfaceUnitState, UnitId};
        let units = self
            .tracks
            .stock_at(tick)
            .into_iter()
            .map(|(unit, mounts)| {
                let mut state = SurfaceUnitState::new(UnitId(unit));
                state.armed = true;
                let size = mounts.keys().max().map_or(0, |m| usize::from(*m) + 1);
                state.mounts = vec![MountStock::default(); size];
                for (index, stock) in mounts {
                    state.mounts[usize::from(index)] = MountStock {
                        loaded: stock.loaded,
                        reserve: stock.reserve,
                        ordinal: 0,
                    };
                }
                state
            })
            .collect();
        SurfaceState::new(0, units)
    }

    /// Shows the gun-flash tracker `picture`: the rounds, launches and flak
    /// the recording holds, and the wrecks of the surface units it lost (as
    /// the ground rows flight would have), with the tick each one died on so
    /// a seek does not start a wreck's smoke afresh (format 3).
    pub(super) fn observe_surface(
        (tracks, recording): (&Tracks, &Recording),
        tracker: &mut crate::gun_flash::Tracker,
        picture: &RenderSnapshot,
        (tick, now): (u64, f64),
        mounts: &impl Fn(u32) -> Option<crate::gun_flash::Mount>,
    ) {
        let deaths = tracks.deaths(tick);
        let rows: Vec<crate::snapshot::AircraftPose> = deaths
            .keys()
            .filter_map(|id| {
                let unit = recording.surface_info(*id)?;
                let position = picture
                    .surface
                    .iter()
                    .find(|pose| pose.id.0 == *id)
                    .map_or(unit.position, |pose| pose.position);
                Some(crate::snapshot::AircraftPose {
                    id: *id,
                    position,
                    crashed: true,
                    damage: crate::snapshot::Damage {
                        hp: 0,
                        initial_hp: unit.hit_points,
                        ..Default::default()
                    },
                    ..Default::default()
                })
            })
            .collect();
        tracker.set_deaths(deaths);
        if rows.is_empty() {
            tracker.observe(picture, now, mounts);
        } else {
            let mut seen = picture.clone();
            seen.targets.extend(rows);
            tracker.observe(&seen, now, mounts);
        }
    }

    /// Ground objects of the recorded world still standing at `tick`, in id
    /// order, with where each is: a unit that follows a route is where the
    /// recording had it, not at its starting place.
    fn ground_objects(&self, tick: u64, picture: &RenderSnapshot) -> Vec<(u32, [f64; 3])> {
        let destroyed = self.tracks.destroyed(tick);
        self.world
            .airport_scene
            .objects
            .iter()
            .filter(|o| !destroyed.contains(&o.id))
            .map(|o| {
                let at = picture
                    .surface
                    .iter()
                    .find(|pose| pose.id.0 == o.id)
                    .map_or(o.bounds.center, |pose| pose.position);
                (o.id, at)
            })
            .collect()
    }

    /// Where `object` is in `picture` at `tick`, for the object view: an
    /// aircraft flying or not yet destroyed, a standing ground object, or a
    /// weapon in flight.
    fn body_of(&self, picture: &RenderSnapshot, tick: u64, object: Target) -> Option<Body> {
        match object {
            Target::Aircraft(id) if id == self.player => Some(Body::posed(&picture.player, 1.)),
            Target::Aircraft(id) => picture
                .target(id)
                .filter(|pose| pose.airborne || pose.damage.hp > 0)
                .map(|pose| Body::posed(pose, 1.)),
            Target::Ground(id) => {
                let object = self
                    .world
                    .airport_scene
                    .objects
                    .iter()
                    .find(|o| o.id == id)?;
                if self.tracks.destroyed(tick).contains(&id) {
                    return None;
                }
                // A unit on a route is where it drove to, facing the way it
                // faces now.
                if let Some(pose) = picture.surface.iter().find(|pose| pose.id.0 == id) {
                    let [yaw, pitch, bank] = pose.attitude;
                    return Some(Body::new(
                        id,
                        pose.position,
                        [0.; 3],
                        Basis::new(yaw, pitch, bank),
                    ));
                }
                let b = object.bounds;
                Some(Body::new(
                    id,
                    b.center,
                    [0.; 3],
                    Basis::new(b.heading, 0., 0.),
                ))
            }
            Target::Missile(id) => picture
                .projectiles
                .iter()
                .find(|p| p.id == id && !p.gun)
                .map(|p| Body::weapon(p, 1.)),
            Target::Nothing => None,
        }
    }

    /// A name for `object` as people read it: an aircraft's label, a ground
    /// object's name, or a weapon and who fired it.
    fn object_name(&self, object: Target) -> String {
        match object {
            Target::Aircraft(id) => self.label(id),
            Target::Ground(id) => self
                .recording
                .surface_info(id)
                .map(|unit| unit.label.clone())
                .filter(|label| !label.is_empty())
                .or_else(|| {
                    self.world
                        .airport_scene
                        .objects
                        .iter()
                        .find(|o| o.id == id)
                        .and_then(|o| {
                            [&o.name, &o.object_type]
                                .into_iter()
                                .find(|n| !n.trim().is_empty())
                                .cloned()
                        })
                })
                .unwrap_or_else(|| format!("Ground object {id}")),
            Target::Missile(id) => match self.missiles.get(&id) {
                Some((owner, weapon)) => format!("{weapon} from {}", self.label(*owner)),
                None => format!("Weapon {id}"),
            },
            Target::Nothing => "nothing".into(),
        }
    }

    /// Everything the object view can look at on the current tick, in a
    /// stable order: aircraft, weapons in flight, then standing ground
    /// objects, each by id, without the object the view starts from.
    fn objects_now(&mut self) -> Vec<Target> {
        let tick = self.clock.tick();
        let picture = self.playback.picture(tick, self.clock.alpha());
        let from = self.from();
        let player = self.player;
        let mut out: Vec<Target> = everyone(&picture)
            .filter(|pose| pose.id == player || pose.airborne || pose.damage.hp > 0)
            .map(|pose| Target::Aircraft(pose.id))
            .chain(
                self.ground_objects(tick, &picture)
                    .into_iter()
                    .map(|(id, _)| Target::Ground(id)),
            )
            .chain(
                picture
                    .projectiles
                    .iter()
                    .filter(|p| !p.gun)
                    .map(|p| Target::Missile(p.id)),
            )
            .filter(|t| *t != from)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// O and Shift+O: the object view looks at the next or previous object
    /// present now.
    fn cycle_look_at(&mut self, forward: bool) {
        let objects = self.objects_now();
        let (Some(first), Some(last)) = (objects.first(), objects.last()) else {
            self.toast("Nothing else to look at now");
            return;
        };
        let next = match self.look_at {
            None if forward => *first,
            None => *last,
            Some(now) if forward => *objects.iter().find(|t| **t > now).unwrap_or(first),
            Some(now) => *objects.iter().rev().find(|t| **t < now).unwrap_or(last),
        };
        self.look_at = Some(next);
        self.object_view();
    }

    /// What View from here looks at when nothing else is chosen: a weapon's
    /// own target while it flies, otherwise the nearest other aircraft.
    fn first_look_at(&mut self) -> Option<Target> {
        let tick = self.clock.tick();
        let picture = self.playback.picture(tick, self.clock.alpha());
        let from = self.from();
        let at = self.body_of(&picture, tick, from)?.position();
        if let Target::Missile(id) = from
            && let Some(target) = picture
                .projectiles
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.target)
                .map(Target::Aircraft)
            && self.body_of(&picture, tick, target).is_some()
        {
            return Some(target);
        }
        everyone(&picture)
            .map(|pose| Target::Aircraft(pose.id))
            .filter(|t| *t != from)
            .filter_map(|t| Some((t, self.body_of(&picture, tick, t)?.position())))
            .min_by(|a, b| feet(a.1, at).total_cmp(&feet(b.1, at)))
            .map(|(t, _)| t)
    }

    /// Where the selected aircraft is drawn in `picture`.
    fn anchor(&self, picture: &RenderSnapshot) -> Option<[f64; 3]> {
        if self.selected == self.player {
            return Some(picture.player.position);
        }
        picture.target(self.selected).map(|t| t.position)
    }

    /// Backquote: from a flight view to the follow drone, then the free
    /// drone, then back. `Some(mode)` switches straight to that drone.
    pub fn drone_mode(&mut self, mode: Option<Mode>) {
        self.drone_subject = None;
        let tick = self.clock.tick();
        let recorded = self.playback.aircraft(tick, self.selected);
        let anchor = recorded.as_ref().map(|a| a.position);
        let heading = recorded.as_ref().map_or(0., |a| a.attitude[0]);
        let next = match (mode, &self.drone) {
            (Some(mode), _) => Some(mode),
            (None, None) => Some(Mode::Follow),
            (None, Some(drone)) if drone.mode == Mode::Follow => Some(Mode::Free),
            (None, Some(_)) => None,
        };
        match next {
            None => self.drone = None,
            Some(mode) => match &mut self.drone {
                Some(drone) => drone.set_mode(mode, anchor),
                None => {
                    self.drone = Some(Drone::from_camera(mode, &self.camera, anchor, heading));
                }
            },
        }
        self.camera_error = None;
    }

    fn camera_label(&self) -> String {
        match &self.drone {
            Some(drone) if drone.mode == Mode::Follow => "Drone follow".into(),
            Some(_) => "Drone free".into(),
            None if self.view == OBJECT => format!(
                "Object: {} > {}",
                self.object_name(self.from()),
                self.look_at
                    .map_or_else(|| "nothing".to_owned(), |t| self.object_name(t))
            ),
            None => view_name(self.view).to_owned(),
        }
    }

    /// The camera button: the next flight view in F-key order, the object
    /// view when something to look at is chosen, then the two drones, then
    /// the first view again.
    fn next_camera(&mut self) {
        match &self.drone {
            Some(drone) if drone.mode == Mode::Follow => self.drone_mode(Some(Mode::Free)),
            Some(_) => self.set_view(VIEW_ORDER[0]),
            None => {
                let at = VIEW_ORDER.iter().position(|v| *v == self.view);
                match at.map(|i| i + 1) {
                    Some(i) if i < VIEW_ORDER.len() => self.set_view(VIEW_ORDER[i]),
                    Some(_) if self.look_at.is_some() => self.object_view(),
                    _ => self.drone_mode(Some(Mode::Follow)),
                }
            }
        }
    }

    /// Esc: opens the Escape menu over the view, pausing playback, and
    /// lets go of every held key, drag and press.
    fn open_pause(&mut self) {
        self.pause.open(&mut self.clock);
        self.menu = None;
        self.release();
    }

    /// Carries out an Escape menu choice.
    fn pause_choice(&mut self, choice: pause::Choice) -> Command {
        use pause::{Choice, Part};
        match choice {
            Choice::None => Command::None,
            Choice::Click => Command::Click,
            Choice::Leave => Command::Leave,
            Choice::Exit => Command::Exit,
            Choice::Graphics => Command::Graphics,
            Choice::Sound => Command::Sound,
            Choice::Controls => Command::Controls,
            Choice::TakeBack => Command::TakeBack,
            Choice::Spawn => Command::Spawn,
            Choice::LeaveGame => Command::LeaveGame,
            Choice::SpawnBlocked => {
                if let Some(why) = self.pause.away().and_then(|a| a.spawn_blocked.clone()) {
                    self.message(why);
                }
                Command::Click
            }
            Choice::Toggle(part) => {
                match part {
                    Part::Labels => self.ui.labels = !self.ui.labels,
                    Part::Timer => self.ui.timer = !self.ui.timer,
                    Part::Trails => self.ui.trails = !self.ui.trails,
                    Part::Marks => self.ui.marks = !self.ui.marks,
                    Part::Banner => self.ui.banner = !self.ui.banner,
                    Part::Comms => {
                        self.panels.toggle_comms();
                        self.ui.comms = self.panels.comms_open();
                    }
                }
                Command::Click
            }
        }
    }

    /// A key pressed or released. `name` is the key as the app names it
    /// (letters lower case, `` ` `` for the backquote key). While the
    /// Escape menu is open it takes every key.
    pub fn key(&mut self, name: &str, pressed: bool, repeat: bool, shift: bool) -> Command {
        if self.feedback_open {
            if pressed {
                let count = self.feedback_flags.len();
                match name {
                    "Escape" => self.feedback_open = false,
                    "ArrowDown" if count > 0 => {
                        self.feedback_scroll = 0;
                        self.feedback_selected = (self.feedback_selected + 1) % count
                    }
                    "ArrowUp" if count > 0 => {
                        self.feedback_scroll = 0;
                        self.feedback_selected = (self.feedback_selected + count - 1) % count
                    }
                    "PageDown" => self.scroll_feedback(20),
                    "PageUp" => self.scroll_feedback(-20),
                    "Enter" => self.seek_feedback(),
                    _ => {}
                }
            }
            return Command::None;
        }
        if self.pause.is_open() {
            if !pressed {
                return Command::None;
            }
            let choice = self.pause.key(name, &mut self.clock);
            return self.pause_choice(choice);
        }
        let mut modifiers = winit::keyboard::ModifiersState::empty();
        modifiers.set(winit::keyboard::ModifiersState::SHIFT, shift);
        if self.bound_key(name, pressed, repeat, modifiers) {
            return Command::None;
        }
        if !pressed {
            return Command::None;
        }
        // An open right-click menu takes its keys first, held ones too.
        if let Some(menu) = &mut self.menu {
            match menu.key(name, shift) {
                Outcome::Ignored => {}
                Outcome::Handled => return Command::None,
                Outcome::Close => {
                    self.menu = None;
                    return Command::None;
                }
                Outcome::Chosen(action) => {
                    self.menu = None;
                    self.perform(action);
                    return Command::None;
                }
            }
        }
        // Stepping and jumping repeat while held; switches do not.
        match name {
            "ArrowLeft" | "ArrowRight" => {
                let sign = if name == "ArrowLeft" { -1. } else { 1. };
                if shift {
                    self.clock.jump(sign * 30.);
                } else if self.clock.paused() {
                    self.clock.step(sign as i64);
                } else {
                    self.clock.jump(sign * 5.);
                }
                return Command::None;
            }
            "ArrowUp" if !repeat => self.clock.faster(),
            "ArrowDown" if !repeat => self.clock.slower(),
            // Zoom as in flight, keypad plus and minus included.
            "=" => self.zoom = (self.zoom * 1.1).min(4.),
            "-" => self.zoom = (self.zoom / 1.1).max(0.5),
            _ if repeat => return Command::None,
            // Recenter the look, keeping the view and the zoom.
            "Numpad5" => self.look = [0.; 2],
            "/" if shift => self.look = [0.; 2],
            "Space" => self.clock.toggle(),
            "j" => self.clock.reverse(),
            "k" => self.clock.pause(),
            "l" => self.clock.forward(),
            "Home" => self.clock.start(),
            "End" => self.clock.end(),
            "PageUp" | "PageDown" => {
                if !self.clock.marker(&self.marker_ticks, name == "PageDown") {
                    self.toast("No more markers that way");
                }
            }
            "Tab" => self.cycle_aircraft(!shift),
            "o" => self.cycle_look_at(!shift),
            "h" => {
                self.ui.hidden = !self.ui.hidden;
                self.menu = None;
            }
            "n" => self.ui.labels = !self.ui.labels,
            "t" => self.ui.timer = !self.ui.timer,
            "c" => {
                self.panels.toggle_comms();
                self.ui.comms = self.panels.comms_open();
            }
            "i" => self.toggle_panel(Kind::Thought, self.selected),
            "f" => self.toggle_panel(Kind::Telemetry, self.selected),
            "g" => match self.newest_missile(self.selected) {
                Some(id) => self.toggle_panel(Kind::Guidance, id),
                None => {
                    let who = self.label(self.selected);
                    self.toast(format!("{who} has no missile in flight"));
                }
            },
            "x" => {
                self.panels.close_all();
                self.ui.comms = false;
            }
            "m" => self.menu_on_selected(),
            "r" if shift => {
                self.ui.trails = true;
                self.ui.trail_length = (self.ui.trail_length + 1) % trails::LENGTHS.len();
                let seconds = trails::LENGTHS[self.ui.trail_length];
                self.toast(format!("Trails: {seconds} seconds"));
            }
            "r" => {
                self.ui.trails = !self.ui.trails;
                self.toast(if self.ui.trails {
                    "Trails on (Shift+R for length)"
                } else {
                    "Trails off"
                });
            }
            "b" => {
                self.ui.marks = !self.ui.marks;
                let words = match (self.ui.marks, self.marks.any()) {
                    (false, _) => "Laser and bomb marks off",
                    (true, true) => "Laser and bomb marks on",
                    (true, false) => "Laser and bomb marks on (this recording has none)",
                };
                self.toast(words);
            }
            "p" => return Command::Screenshot,
            "Escape" => self.open_pause(),
            // The HUD camera and the Sensor View (nr-replayviews): the
            // aircraft's video recorder, the HUD's tape and the pod's.
            "v" if shift => self.taped_view_key(tape_views::SENSOR),
            "v" => self.taped_view_key(tape_views::HUD_CAMERA),
            key if self.sensor_mode_key(key, shift) => {}
            key => {
                if let Some(view) = flight_views::key(key) {
                    self.set_view(view);
                }
            }
        }
        Command::None
    }

    pub fn set_input_profile(&mut self, profile: &tore_input::Profile) {
        self.release();
        self.drone_input = crate::replay::drone::Controls::new(profile);
    }

    /// Configured drone keys run before the fixed replay shortcuts. A release
    /// uses the binding claimed on press, even if a modifier was released first.
    pub fn bound_key(
        &mut self,
        name: &str,
        pressed: bool,
        repeat: bool,
        modifiers: winit::keyboard::ModifiersState,
    ) -> bool {
        if self.feedback_open {
            return false;
        }
        if pressed && repeat && !self.drone_input.claimed(name) {
            return false;
        }
        if pressed && (self.pause.is_open() || self.menu.is_some() || name == "Escape") {
            return false;
        }
        let taken = self.drone_input.key(name, pressed, modifiers);
        self.apply_drone_controls();
        taken
    }

    pub fn drone_event(&mut self, event: tore_input::Event) {
        if self.pause.is_open() || self.menu.is_some() {
            return;
        }
        self.drone_input.event(event);
        self.apply_drone_controls();
    }

    pub fn drone_devices(&mut self, present: impl Fn(&str) -> bool) {
        self.drone_input.retain_devices(present);
        self.apply_drone_controls();
    }

    pub fn drone_mouse(&mut self, control: &str, pressed: bool) {
        if pressed && (self.pause.is_open() || self.menu.is_some()) {
            return;
        }
        self.drone_input.mouse(control, pressed);
        self.apply_drone_controls();
    }

    fn apply_drone_controls(&mut self) {
        self.held = [
            ("drone-forward", "w"),
            ("drone-backward", "s"),
            ("drone-left", "a"),
            ("drone-right", "d"),
            ("drone-up", "e"),
            ("drone-down", "q"),
        ]
        .into_iter()
        .filter(|(action, _)| self.drone_input.held(action))
        .map(|(_, key)| key.to_owned())
        .collect();
        for command in self.drone_input.commands() {
            match command.as_str() {
                "drone-cycle" => self.drone_mode(None),
                "drone-follow" => self.drone_mode(Some(Mode::Follow)),
                "drone-free" => self.drone_mode(Some(Mode::Free)),
                "drone-faster" | "drone-slower" => {
                    if let Some(drone) = &mut self.drone {
                        drone.wheel(if command == "drone-faster" { 1 } else { -1 });
                    }
                }
                _ => {}
            }
        }
    }

    /// The pointer left the window.
    pub fn pointer_left(&mut self) {
        self.bar.hover = None;
        self.dragging = None;
        self.right_click = None;
        self.point = None;
        self.panels.hover = None;
    }

    /// Lets go of every held key and drag, when the window loses focus.
    pub fn release(&mut self) {
        self.drone_input.release();
        self.held.clear();
        self.dragging = None;
        self.right_click = None;
        self.left_owner = None;
        self.bar = overlay::Pointer::default();
        self.pause.widget.cancel_press();
    }

    /// The layer point under a view point, when the interface shows.
    fn layer_point(&self, point: Option<[f64; 2]>, size: [u32; 2]) -> Option<(f64, f64)> {
        point
            .filter(|_| !self.ui.hidden)
            .and_then(|p| Placement::new(size).layer(p))
    }

    /// The point in the panels' layer under a view point, when the
    /// interface shows.
    fn panel_point(&self, point: Option<[f64; 2]>, size: [u32; 2]) -> Option<(f64, f64)> {
        point
            .filter(|_| !self.ui.hidden)
            .map(|p| Placement::new(size).centered(p))
    }

    /// The pointer moved to `point` in view pixels (`None` off the window);
    /// `window` is the same point in window pixels, for looking around, and
    /// `radians` how far one pixel turns the view.
    pub fn pointer(
        &mut self,
        point: Option<[f64; 2]>,
        window: [f64; 2],
        size: [u32; 2],
        radians: [f64; 2],
    ) {
        if self.pause.is_open() {
            return;
        }
        if let Some((click, _)) = &mut self.right_click {
            click.moved(window);
        }
        let layer = self.layer_point(point, size);
        let at = self.panel_point(point, size);
        self.point = at;
        let over_menu = match &mut self.menu {
            Some(menu) => {
                menu.pointer(at);
                at.is_some_and(|at| menu.contains(at))
            }
            None => false,
        };
        self.panels.pointer(self.layout, at.filter(|_| !over_menu));
        self.bar.moved(
            layer.filter(|_| !over_menu || self.bar.scrubbing),
            &mut self.clock,
        );
        if self.drone.is_some() {
            if !self.drone_input.held("drone-look") {
                self.dragging = None;
            } else if self.dragging.is_none() {
                self.dragging = Some(window);
            }
        }
        if let Some(last) = self.dragging {
            let delta = [
                (window[0] - last[0]) * radians[0],
                (window[1] - last[1]) * radians[1],
            ];
            self.dragging = Some(window);
            match &mut self.drone {
                Some(drone) => drone.look([delta[0], -delta[1]]),
                None => {
                    crate::look::nudge(&mut self.look, [delta[0] as f32, -delta[1] as f32], true)
                }
            }
        }
    }

    /// The left button went down or up at `point` in view pixels: the
    /// Escape menu takes it while open, then the right-click menu (a press
    /// outside closes it), then the debug panels, then the transport bar.
    pub fn left(&mut self, pressed: bool, point: Option<[f64; 2]>, size: [u32; 2]) -> Command {
        if self.pause.is_open() {
            // The menu is drawn where flight draws it, centred on the view.
            let at = point.map(|p| Placement::new(size).centered(p));
            let choice = self.pause.pointer(at, pressed, &mut self.clock);
            return self.pause_choice(choice);
        }
        let layer = self.layer_point(point, size);
        if self.feedback_open
            && let Some((x, y)) = layer
            && (8. ..632.).contains(&x)
            && (32. ..426.).contains(&y)
        {
            if !pressed && x < 248. && (40. ..232.).contains(&y) {
                let index = self.feedback_selected / 8 * 8 + ((y - 40.) / 24.) as usize;
                if index < self.feedback_flags.len() {
                    self.feedback_selected = index;
                    self.seek_feedback();
                }
            }
            return Command::None;
        }
        let at = self.panel_point(point, size);
        if pressed {
            let owner = if let Some(menu) = &mut self.menu {
                if !menu.down(at) {
                    self.menu = None;
                }
                Owner::Menu
            } else if self.panels.down(self.layout, at) {
                Owner::Panels
            } else {
                self.bar.down(layer, &mut self.clock);
                Owner::Bar
            };
            self.left_owner = Some(owner);
            return Command::None;
        }
        match self.left_owner.take() {
            Some(Owner::Menu) => {
                let chosen = self.menu.as_mut().map(|menu| menu.up(at));
                if let Some(Outcome::Chosen(action)) = chosen {
                    self.menu = None;
                    self.perform(action);
                }
                return Command::None;
            }
            Some(Owner::Panels) => {
                let data = ReplayData {
                    recording: &self.recording,
                    trees: &mut self.trees,
                    info: &self.info,
                    missiles: &self.missiles,
                    comms: &self.comms,
                    now: self.clock.tick(),
                    player: self.player,
                };
                self.panels.up(self.layout, at, &data);
                self.ui.comms = self.panels.comms_open();
                return Command::None;
            }
            Some(Owner::Bar) | None => {}
        }
        let Some(control) = self.bar.up(layer) else {
            return Command::None;
        };
        if !overlay::transport(control, &mut self.clock) {
            match control {
                Control::Camera => self.next_camera(),
                Control::Aircraft => self.cycle_aircraft(true),
                Control::HideUi => self.ui.hidden = true,
                Control::Feedback if self.recording.flagged() => {
                    self.feedback_open = !self.feedback_open;
                    self.clock.pause();
                }
                _ => {}
            }
        }
        Command::None
    }

    /// The right button went down or up at `window`, in window pixels, and
    /// at `point` in the view's own pixels on a view of `size`: dragging
    /// with it turns the view, and a click (released within `slop` window
    /// pixels, never having strayed further) opens the right-click menu.
    pub fn right(
        &mut self,
        pressed: bool,
        window: [f64; 2],
        point: Option<[f64; 2]>,
        size: [u32; 2],
        slop: f64,
    ) {
        if self.pause.is_open() {
            self.dragging = None;
            self.right_click = None;
            return;
        }
        self.drone_mouse("button:right", pressed);
        if pressed {
            self.dragging = Some(window);
            self.right_click = point.map(|p| (RightClick::press(window, slop), p));
            return;
        }
        self.dragging = None;
        if let Some((click, at)) = self.right_click.take()
            && click.release(window)
        {
            self.open_menu(at, size);
        }
    }

    /// Mouse wheel notches: a menu or panel under the pointer scrolls,
    /// otherwise the drone's speed, or zoom in a flight view.
    pub fn wheel(&mut self, notches: i32) {
        if self.feedback_open {
            self.scroll_feedback(-notches * 3);
            return;
        }
        if self.pause.is_open() {
            return;
        }
        if let Some(menu) = &mut self.menu
            && self.point.is_some_and(|at| menu.contains(at))
        {
            menu.wheel(notches);
            return;
        }
        let data = ReplayData {
            recording: &self.recording,
            trees: &mut self.trees,
            info: &self.info,
            missiles: &self.missiles,
            comms: &self.comms,
            now: self.clock.tick(),
            player: self.player,
        };
        if self.panels.wheel(self.layout, self.point, notches, &data) {
            return;
        }
        if self.drone.is_some() {
            let control = if notches > 0 {
                "wheel:up"
            } else {
                "wheel:down"
            };
            for _ in 0..notches.unsigned_abs().min(8) {
                self.drone_mouse(control, true);
                self.drone_mouse(control, false);
            }
            let speed = self.drone.as_ref().unwrap().speed();
            self.toast(format!("Drone speed {speed:.0} ft/s"));
        } else {
            self.zoom = (self.zoom * 1.1f32.powi(notches)).clamp(0.5, 4.);
        }
    }

    fn open_panel(&mut self, kind: Kind, subject: u32) {
        if let Err(reason) = self.panels.open(kind, subject) {
            self.toast(reason);
        }
    }

    fn toggle_panel(&mut self, kind: Kind, subject: u32) {
        if let Err(reason) = self.panels.toggle(kind, subject) {
            self.toast(reason);
        }
    }

    /// The newest missile `owner` has in flight at the playhead.
    fn newest_missile(&mut self, owner: u32) -> Option<u32> {
        let (frames, at) = self.playback.frame(self.clock.tick())?;
        frames[at]
            .projectiles
            .iter()
            .filter(|p| {
                p.owner == owner
                    && self
                        .recording
                        .weapon_info(p.weapon)
                        .is_none_or(|w| w.class == WeaponClass::Missile)
            })
            .min_by_key(|p| p.age)
            .map(|p| p.id)
    }

    /// F12 uses every active non-gun weapon in the same recording tick as the object list.
    fn active_missiles(&mut self, tick: u64) -> Vec<u32> {
        let Some((frames, at)) = self.playback.frame(tick) else {
            return Vec::new();
        };
        let mut ids: Vec<_> = frames[at]
            .projectiles
            .iter()
            .filter(|p| {
                self.recording
                    .weapon_info(p.weapon)
                    .is_some_and(|w| w.class != WeaponClass::Gun)
            })
            .map(|p| p.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn object_list_items(&mut self) -> Vec<context_menu::Item> {
        let entries: Vec<_> = self
            .present()
            .into_iter()
            .map(|id| self.menu_entry(id))
            .collect();
        let mut items = context_menu::jump_items(
            &entries,
            context_menu::Options {
                live: false,
                labels: self.ui.labels,
                trails: self.ui.trails,
            },
        );
        let picture = self.playback.picture(self.clock.tick(), self.clock.alpha());
        let mut weapons: Vec<_> = picture.projectiles.iter().filter(|p| !p.gun).collect();
        weapons.sort_by_key(|p| p.id);
        if !weapons.is_empty() {
            items.push(context_menu::Item {
                label: "ACTIVE WEAPONS".into(),
                detail: String::new(),
                action: None,
            });
            items.extend(weapons.into_iter().map(|p| context_menu::Item {
                label: format!("{} #{}", p.weapon.trim_end_matches(".JT"), p.id),
                detail: self.label(p.owner),
                action: Some(Action::ViewFrom(Target::Missile(p.id))),
            }));
        }
        items
    }

    /// An aircraft as the menus list it.
    fn menu_entry(&self, id: u32) -> context_menu::Aircraft {
        let info = self.info.get(&id);
        context_menu::Aircraft {
            id,
            label: self.label(id),
            name: info.map(|a| a.name.clone()).unwrap_or_default(),
            side: self.side(id),
            wing: info.map_or(0, |a| a.wing),
            member: info.map_or(0, |a| a.member),
            ai: id != self.player && info.is_some_and(|a| !a.human),
            sight: self.sight_of(id).is_some(),
        }
    }

    /// The right-click menu for `target`, opened at layer point `at`.
    fn build_menu(&mut self, target: Target, at: (f64, f64)) -> Menu {
        let options = context_menu::Options {
            live: false,
            labels: self.ui.labels,
            trails: self.ui.trails,
        };
        let (title, items) = match target {
            Target::Aircraft(id) => {
                let entry = self.menu_entry(id);
                (entry.title(), context_menu::aircraft_items(&entry, options))
            }
            Target::Missile(id) => {
                let (owner, title) = match self.missiles.get(&id) {
                    Some((owner, weapon)) => (
                        Some(*owner),
                        format!("{weapon} from {}", self.label(*owner)),
                    ),
                    None => (None, format!("Missile {id}")),
                };
                let owner = owner.map(|o| self.menu_entry(o));
                (
                    title,
                    context_menu::missile_items(id, owner.as_ref(), options),
                )
            }
            Target::Ground(id) => (
                self.object_name(target),
                context_menu::ground_items(id, options),
            ),
            Target::Nothing => ("Jump to an object".to_owned(), self.object_list_items()),
        };
        Menu::new(target, title, items, at, &self.ownship.font)
    }

    /// A right-click at `at` in view pixels on a view of `size`: the menu
    /// for a panel's aircraft or missile when on a panel, otherwise for the
    /// aircraft or missile drawn nearest the pointer, otherwise the list of
    /// aircraft. Nothing opens while the interface is hidden.
    fn open_menu(&mut self, at: [f64; 2], size: [u32; 2]) {
        if self.ui.hidden {
            return;
        }
        let placement = Placement::new(size);
        let layer = placement.centered(at);
        if placement.layer(at).and_then(overlay::hit) == Some(Control::Camera) {
            self.open_camera_menu(layer);
            return;
        }
        let on_panel = match self.panels.hit(self.layout, layer) {
            Some(panels::Hit::Body(side) | panels::Hit::Pin(side) | panels::Hit::Close(side)) => {
                self.panels.slot(side).map(|p| match p.kind {
                    Kind::Guidance => Target::Missile(p.subject),
                    Kind::Thought | Kind::Telemetry => Target::Aircraft(p.subject),
                })
            }
            _ => None,
        };
        let target = on_panel.unwrap_or_else(|| {
            context_menu::pick(
                at,
                size,
                &self.camera,
                &self.pickables,
                context_menu::PICK_RADIUS * placement.scale(),
            )
        });
        self.menu = Some(self.build_menu(target, layer));
    }

    fn refresh_object_menu(&mut self) {
        if self
            .menu
            .as_ref()
            .is_some_and(|m| m.target == Target::Nothing && !m.camera_choices)
        {
            let items = self.object_list_items();
            self.menu.as_mut().unwrap().replace_items(items);
        }
    }

    fn open_camera_menu(&mut self, at: (f64, f64)) {
        let mut items: Vec<_> = VIEW_ORDER
            .into_iter()
            .chain([2, OBJECT, tape_views::HUD_CAMERA, tape_views::SENSOR])
            .map(|view| Item {
                label: view_name(view).into(),
                detail: if self.drone.is_none() && self.view == view {
                    "Current"
                } else {
                    ""
                }
                .into(),
                action: match view {
                    OBJECT => self.look_at.is_some(),
                    tape_views::SENSOR => self.sight_of(self.selected).is_some(),
                    _ => true,
                }
                .then_some(Action::Camera(view)),
            })
            .collect();
        for (mode, label) in [(Mode::Follow, "Drone follow"), (Mode::Free, "Drone free")] {
            items.push(Item {
                label: label.into(),
                detail: if self.drone.as_ref().is_some_and(|d| d.mode == mode) {
                    "Current"
                } else {
                    ""
                }
                .into(),
                action: Some(Action::DroneMode(mode)),
            });
        }
        let mut menu = Menu::new(
            Target::Nothing,
            "Choose view".into(),
            items,
            at,
            &self.ownship.font,
        );
        menu.camera_choices = true;
        self.menu = Some(menu);
    }

    /// M: the right-click menu on the selected aircraft, where the last
    /// frame drew it or else in the middle of the view.
    fn menu_on_selected(&mut self) {
        let camera = copy(&self.camera);
        self.menu_at_selected(&camera);
    }

    /// The right-click menu on the selected aircraft, where `camera` shows
    /// it or else in the middle of the view.
    fn menu_at_selected(&mut self, camera: &Camera) {
        if self.ui.hidden {
            return;
        }
        let size = self.size;
        let at = self
            .pickables
            .iter()
            .find(|p| p.target == Target::Aircraft(self.selected))
            .and_then(|p| camera.project(size, p.position))
            .unwrap_or([f64::from(size[0]) / 2., f64::from(size[1]) / 2.]);
        let layer = Placement::new(size).centered(at);
        self.menu = Some(self.build_menu(Target::Aircraft(self.selected), layer));
    }

    /// Does what a menu item says.
    fn perform(&mut self, action: Action) {
        match action {
            Action::Camera(OBJECT) => self.object_view(),
            Action::Camera(view @ (tape_views::HUD_CAMERA | tape_views::SENSOR)) => {
                self.taped_view_key(view)
            }
            Action::Camera(view) => self.set_view(view),
            Action::HudCamera(id) => {
                self.select(id);
                self.taped_view_key(tape_views::HUD_CAMERA);
            }
            Action::SensorView(id) => {
                self.select(id);
                self.taped_view_key(tape_views::SENSOR);
            }
            Action::DroneMode(mode) => self.drone_mode(Some(mode)),
            Action::Follow(id) => {
                self.select(id);
                self.set_view(EXTERNAL);
            }
            Action::Cockpit(id) => {
                self.select(id);
                self.set_view(0);
            }
            Action::Drone(id) => {
                self.select(id);
                match self.playback.aircraft(self.clock.tick(), id) {
                    Some(a) => {
                        self.drone = Some(Drone::beside(Mode::Follow, a.position, a.attitude[0]));
                        self.camera_error = None;
                    }
                    None => self.toast("That aircraft is not in the recording now"),
                }
            }
            Action::DroneMissile(id) => {
                let found = self
                    .playback
                    .frame(self.clock.tick())
                    .and_then(|(frames, at)| {
                        frames[at]
                            .projectiles
                            .iter()
                            .find(|p| p.id == id)
                            .map(|p| (p.position, p.direction[0].atan2(p.direction[2])))
                    });
                match found {
                    Some((position, heading)) => {
                        self.drone = Some(Drone::beside(Mode::Free, position, heading));
                        self.drone_subject = Some(Target::Missile(id));
                        self.camera_error = None;
                    }
                    None => self.toast("That missile is not in the recording now"),
                }
            }
            Action::Thought(id) => self.open_panel(Kind::Thought, id),
            Action::Telemetry(id) => self.open_panel(Kind::Telemetry, id),
            Action::Guidance(id) => self.open_panel(Kind::Guidance, id),
            Action::Comms(id) => self.panels.open_comms(Some(id)),
            Action::Labels => self.ui.labels = !self.ui.labels,
            Action::Trails => self.ui.trails = !self.ui.trails,
            Action::Jump(id) => self.select(id),
            Action::ViewFrom(target) => {
                match target {
                    Target::Aircraft(id) => self.select(id),
                    Target::Nothing => return,
                    other => self.from = Some(other),
                }
                if self.look_at.is_none_or(|t| t == self.from()) {
                    self.look_at = self.first_look_at();
                }
                self.object_view();
            }
            Action::LookAt(target) => {
                if target == self.from() {
                    self.toast("The object view already starts from there");
                } else {
                    self.look_at = Some(target);
                    self.object_view();
                }
            }
        }
        self.ui.comms = self.panels.comms_open();
    }

    /// What a right-click can pick in `picture`: every recorded aircraft,
    /// with its name label when one is drawn, every weapon but gun rounds,
    /// and the `ground` objects still standing, by id and place.
    fn pickables_for(
        picture: &RenderSnapshot,
        labels: &[Label],
        ground: &[(u32, [f64; 3])],
    ) -> Vec<Pickable> {
        let labels: Vec<(u32, [f64; 4])> = labels.iter().map(|l| (l.id, l.rect())).collect();
        let mut out = context_menu::pickables(picture, &labels, |_| true);
        out.extend(ground.iter().map(|&(id, position)| Pickable {
            target: Target::Ground(id),
            position,
            label: None,
        }));
        out
    }

    /// The panels and menu asked for at start, once the first picture is
    /// known and `camera` shows it.
    fn open_requests(&mut self, camera: &Camera) {
        for request in std::mem::take(&mut self.requests) {
            match request {
                Request::Thought => self.open_panel(Kind::Thought, self.selected),
                Request::Telemetry => self.open_panel(Kind::Telemetry, self.selected),
                Request::Guidance => match self.newest_missile(self.selected) {
                    Some(id) => self.open_panel(Kind::Guidance, id),
                    None => {
                        let who = self.label(self.selected);
                        self.toast(format!("{who} has no missile in flight"));
                    }
                },
                Request::Comms => self.panels.open_comms(None),
                Request::Menu => self.menu_at_selected(camera),
            }
        }
        self.ui.comms = self.panels.comms_open();
    }

    /// The UI covers the view, or the Escape menu is open: the pointer
    /// shows.
    pub fn pointer_visible(&self) -> bool {
        !self.ui.hidden || self.pause.is_open()
    }

    /// The target of `id` at `tick` as the recording knows it.
    fn target_of(&self, id: u32, tick: u64) -> Option<Option<u32>> {
        let list = self.targets.get(&id)?;
        let at = list.partition_point(|(t, _)| *t <= tick);
        at.checked_sub(1).map(|i| list[i].1)
    }

    /// The target the flight views use at `tick`: the selected aircraft's
    /// when the recording knows it, otherwise the player's.
    fn view_target(&self, tick: u64) -> Option<u32> {
        self.target_of(self.selected, tick)
            .or_else(|| self.target_of(self.player, tick))
            .flatten()
    }

    /// The scene the flight views read, built from the picture: the
    /// selected aircraft's target when the recording knows it, otherwise
    /// the player's.
    fn scene(&self, picture: &RenderSnapshot, tick: u64) -> Scene {
        // Playing backwards everything moves the other way, so the fly-by
        // point goes ahead of the motion the viewer sees; paused, nothing
        // moves, and the direction it will play on in decides.
        let sign = if self.clock.direction() == Direction::Reverse {
            -1.
        } else {
            1.
        };
        let target = self.view_target(tick);
        let wing_of = |id: u32| {
            let info = self.info.get(&id)?;
            (info.wing > 0).then(|| {
                (
                    info.side == Side::Friendly,
                    u8::try_from(info.wing).unwrap_or(u8::MAX),
                    u8::try_from(info.member).unwrap_or(u8::MAX),
                )
            })
        };
        Scene::of_picture(
            Body::posed(&picture.player, sign),
            target,
            picture,
            wing_of,
            sign,
        )
    }

    /// Find the first approach in recorded time, once per weapon. Keeping the
    /// eye after a miss makes direct seeks and reverse playback independent of
    /// viewing history, including a seek which skips the entire approach.
    fn weapon_stop_at(&mut self, id: u32, tick: u64) -> Option<flight_views::WeaponStop> {
        let approach = self.weapon_stops.get(&id).copied().unwrap_or_default();
        if let Some((when, stop)) = approach.stopped {
            return (when <= tick).then_some(stop);
        }
        let mut target = approach.target.or_else(|| {
            self.recording
                .events_between(0, tick)
                .iter()
                .find_map(|entry| {
                    let event = &entry.event;
                    (event.kind == "weapon.launch"
                        && event.get("projectile").and_then(|v| v.as_i64()) == Some(i64::from(id)))
                    .then_some(event.object)
                    .flatten()
                })
        });
        let launched = self
            .playback
            .frame(tick)
            .and_then(|(frames, at)| {
                frames[at]
                    .projectiles
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| tick.saturating_sub(u64::from(p.age)))
            })
            .unwrap_or(0);
        for before in approach.scanned.max(launched)..=tick {
            let Some((frames, at)) = self.playback.frame(before) else {
                continue;
            };
            let frame = &frames[at];
            if frame.tick != before {
                continue;
            }
            let Some(p) = frame.projectiles.iter().find(|p| p.id == id) else {
                continue;
            };
            target = p.target.or(target);
            let aim = self
                .marks
                .scene(before)
                .weapons
                .iter()
                .find(|w| w.projectile == id)
                .and_then(|w| w.aim)
                .or_else(|| {
                    target.and_then(|id| {
                        frame
                            .aircraft
                            .iter()
                            .find(|a| a.id == id)
                            .map(|a| a.position)
                            .or_else(|| {
                                frame
                                    .surface
                                    .iter()
                                    .find(|u| u.id == id)
                                    .map(|u| u.position)
                            })
                            .or_else(|| {
                                self.world
                                    .airport_scene
                                    .objects
                                    .iter()
                                    .find(|o| o.id == id)
                                    .map(|o| o.bounds.center)
                            })
                    })
                });
            let d = p.direction;
            let body = Body::missile(
                p.id,
                p.position,
                d.map(|v| v * p.speed),
                Basis::new(d[0].atan2(d[2]), d[1].atan2(d[0].hypot(d[2])), 0.),
            );
            if let Some(stop) =
                flight_views::weapon_stop(body, aim, |x, z| self.world.surface(x, z).height)
            {
                self.weapon_stops.insert(
                    id,
                    WeaponApproach {
                        scanned: before + 1,
                        target,
                        stopped: Some((before, stop)),
                    },
                );
                return Some(stop);
            }
        }
        self.weapon_stops.insert(
            id,
            WeaponApproach {
                scanned: approach.scanned.max(tick + 1),
                target,
                stopped: None,
            },
        );
        None
    }

    /// Chase placement from one recorded picture, also used for its final pose.
    fn weapon_pose(&mut self, picture: &RenderSnapshot, tick: u64, id: u32) -> Option<Camera> {
        let p = picture.projectiles.iter().find(|p| p.id == id && !p.gun)?;
        let target = p
            .target
            .and_then(|id| {
                self.body_of(picture, tick, Target::Aircraft(id))
                    .or_else(|| self.body_of(picture, tick, Target::Ground(id)))
            })
            .map_or_else(
                || std::array::from_fn(|i| p.position[i] + p.direction[i] * 1000.),
                |b| b.position(),
            );
        Some(self.weapon_stop_at(id, tick).map_or_else(
            || flight_views::relation(Body::weapon(p, 1.), target),
            |stop| flight_views::stopped_weapon_camera(stop, p.position),
        ))
    }

    /// Reconstruct the hold from history, never from the direction of playback
    /// or the previous rendered frame. The bounded search includes miss expiry
    /// and self-destruct, which need not have an impact event.
    fn weapon_chase(
        &mut self,
        picture: &RenderSnapshot,
        tick: u64,
        id: Option<u32>,
    ) -> Option<(u32, Camera)> {
        if let Some(id) = id
            && let Some(camera) = self.weapon_pose(picture, tick, id)
        {
            return Some((id, camera));
        }
        for before in (tick.saturating_sub(flight_views::weapon_hold_ticks())..tick).rev() {
            let (frames, at) = self.playback.frame(before)?;
            // Do not extend a pose across a missing recording interval.
            if frames[at].tick != before {
                continue;
            }
            let found = frames[at]
                .projectiles
                .iter()
                .filter(|p| {
                    id.is_none_or(|id| p.id == id)
                        && self
                            .recording
                            .weapon_info(p.weapon)
                            .is_some_and(|w| w.class != WeaponClass::Gun)
                })
                .map(|p| p.id)
                .min();
            if let Some(id) = found {
                let last = self.playback.picture(before, 1.);
                return self
                    .weapon_pose(&last, before, id)
                    .map(|camera| (id, camera));
            }
        }
        None
    }

    /// The camera for this frame: the drone, or the selected flight view.
    /// A view that cannot be shown (no target, no wingman) says why once and
    /// shows the aircraft from outside; an aircraft no longer recorded keeps
    /// the last camera.
    fn frame_camera(
        &mut self,
        picture: &RenderSnapshot,
        tick: u64,
        seconds: f64,
        shift: bool,
    ) -> Camera {
        let anchor = self.anchor(picture).or(self.anchor);
        self.anchor = anchor;
        self.compass = None;
        self.readout = None;
        if let Some(drone) = &mut self.drone {
            self.drone_input.shift(shift);
            drone.step(
                seconds,
                &self.held,
                self.drone_input.held("drone-boost"),
                anchor,
                ground(&self.world),
            );
            return drone.camera(anchor, ground(&self.world));
        }
        if self.view == OBJECT {
            return self.object_camera(picture, tick);
        }
        let scene = self.scene(picture, tick);
        self.rig.observe(&scene);
        if matches!(self.view, tape_views::HUD_CAMERA | tape_views::SENSOR) {
            // The HUD camera and the Sensor View make their own camera
            // (viewer/views.rs); this is the view from outside they fall
            // back to when they cannot be shown.
            return self
                .rig
                .camera(EXTERNAL, &scene, Camera::new(), [0.; 2], 1.)
                .unwrap_or_else(|_| copy(&self.camera));
        }
        let result = if self.view == flight_views::MISSILE {
            let unmodified = self.rig.reference == Reference::Aircraft(self.selected);
            if self.selected_missile.is_none() && unmodified {
                self.selected_missile = active_successor(None, &self.active_missiles(tick), false);
            }
            let id = self.selected_missile;
            (id.is_some() || unmodified)
                .then(|| self.weapon_chase(picture, tick, id))
                .flatten()
                .map(|(id, mut camera)| {
                    if self.rig.reference == Reference::Aircraft(self.selected) {
                        self.selected_missile = Some(id);
                    }
                    camera.zoom = self.zoom;
                    camera
                })
                .ok_or("No active weapon for this view")
        } else {
            self.rig
                .camera(self.view, &scene, Camera::new(), self.look, self.zoom)
        };
        match result {
            Ok(camera) => {
                self.camera_error = None;
                if std::mem::take(&mut self.announce_wingman)
                    && let Some(id) = self.rig.wingman()
                {
                    let name = self.label(id);
                    self.toast(format!("Wingman view: {name}"));
                }
                // F7 from the selected aircraft carries the bearing compass.
                if self.view == flight_views::TARGET
                    && self.rig.reference == Reference::Aircraft(self.selected)
                    && let Some(target) = self.view_target(tick)
                    && let Some(from) = self.body_of(picture, tick, Target::Aircraft(self.selected))
                    && let Some(to) = self.body_of(picture, tick, Target::Aircraft(target))
                {
                    self.compass =
                        Some(crate::view_compass::bearing(from.position(), to.position()));
                }
                camera
            }
            Err(reason) => {
                self.announce_wingman = false;
                if self.camera_error != Some(reason) {
                    self.camera_error = Some(reason);
                    self.toast(reason);
                }
                if self.view == flight_views::MISSILE {
                    let mut forward = Rig::default();
                    forward.select(Reference::Aircraft(self.selected));
                    return forward
                        .camera(0, &scene, Camera::new(), [0.; 2], self.zoom)
                        .unwrap_or_else(|_| copy(&self.camera));
                }
                // The aircraft from outside; if it has left the scene, the
                // last camera shown, or at first the player from outside.
                let mut outside = self.rig.clone();
                let mut player = Rig::default();
                player.select(Reference::Aircraft(self.player));
                outside
                    .camera(EXTERNAL, &scene, Camera::new(), self.look, self.zoom)
                    .or_else(|error| {
                        if self.shown {
                            Err(error)
                        } else {
                            player.camera(EXTERNAL, &scene, Camera::new(), [0.; 2], 1.)
                        }
                    })
                    .unwrap_or_else(|_| copy(&self.camera))
            }
        }
    }

    /// The object view's camera: from the object it starts from toward the
    /// one it looks at, at any range. With either end missing it says why
    /// once and shows the starting object from outside, or keeps the last
    /// camera when that is gone too; it comes back when both are there.
    fn object_camera(&mut self, picture: &RenderSnapshot, tick: u64) -> Camera {
        let from = self.body_of(picture, tick, self.from());
        let to = self.look_at.and_then(|t| self.body_of(picture, tick, t));
        if let (Some(from), Some(to), Some(look_at)) = (from, to, self.look_at) {
            self.camera_error = None;
            self.readout = Some(format!(
                "{} > {}   {:.1} nmi",
                self.object_name(self.from()),
                self.object_name(look_at),
                feet(from.position(), to.position()) / NMI
            ));
            let mut camera = flight_views::object_camera(from, to.position(), ground(&self.world));
            camera.zoom = self.zoom;
            return camera;
        }
        let reason = if self.look_at.is_none() {
            "Nothing to look at: press O or right-click an object"
        } else if from.is_none() {
            "The object the view starts from is not in the recording now"
        } else {
            "The object to look at is not in the recording now"
        };
        if self.camera_error != Some(reason) {
            self.camera_error = Some(reason);
            self.toast(reason);
        }
        let player = (!self.shown && self.player != convert::NO_PLAYER)
            .then(|| Body::posed(&picture.player, 1.));
        match from.or(player) {
            Some(from) => {
                let mut camera = flight_views::outside(from);
                camera.zoom = self.zoom;
                camera
            }
            None => copy(&self.camera),
        }
    }

    /// The player as drawn: its pose over the airframe's start state, with
    /// the recorded load factor and the roll rate its attitudes imply.
    fn player_state(&mut self, picture: &RenderSnapshot, tick: u64) -> flight::State {
        let mut state = snapshot::pose_state(&self.template, &picture.player);
        state.ticks = tick;
        if let Some(now) = self.playback.aircraft(tick, self.player) {
            state.g = now.g;
            state.roll_rate = tick
                .checked_sub(1)
                .and_then(|t| self.playback.aircraft(t, self.player))
                .map_or(0., |before| roll_rate(before.attitude, now.attitude));
        }
        state
    }

    /// Trail ribbons for every aircraft and guided weapon in the picture.
    fn trails(
        &self,
        picture: &RenderSnapshot,
        tick: u64,
        camera: &Camera,
        height: f64,
        out: &mut Vec<f32>,
    ) {
        let origin = self.scenery.origin;
        let seconds = trails::LENGTHS[self.ui.trail_length];
        for pose in everyone(picture) {
            let Some(samples) = self.tracks.aircraft.get(&pose.id) else {
                continue;
            };
            let points = trails::path(samples, tick, seconds, pose.position);
            trails::ribbon_world(
                out,
                &points,
                trails::side_color(self.side(pose.id)),
                camera,
                origin,
                height,
            );
        }
        for projectile in picture.projectiles.iter().filter(|p| !p.gun) {
            let Some(path) = self.tracks.missiles.get(&projectile.id) else {
                continue;
            };
            let points = trails::path(&path.samples, tick, seconds, projectile.position);
            let color = trails::weapon_color(self.side(path.owner));
            trails::ribbon_world(out, &points, color, camera, origin, height);
        }
    }

    /// Name labels over the aircraft in the picture, in their side's colour,
    /// placed for a view of `size` pixels. The selected aircraft's label is
    /// bracketed; an aircraft the camera sits inside has none.
    fn labels(&self, picture: &RenderSnapshot, camera: &Camera, size: [u32; 2]) -> Vec<Label> {
        name_labels(
            everyone(picture),
            camera,
            size,
            &self.ownship.font,
            Some(self.selected),
            &|id| (self.label(id), self.side(id)),
        )
    }

    /// Whether the laser and bomb marks are drawn: switched on, and the
    /// recording has any.
    fn marks_shown(&self) -> bool {
        self.ui.marks && self.marks.any()
    }

    /// Whether the marks' shapes are drawn: H hides them (the designation
    /// square, laser spots, weapon lines) with the rest of the interface
    /// (John, 2026-10-15).
    fn marks_drawn(&self) -> bool {
        self.marks_shown() && !self.ui.hidden
    }

    /// What the marks draw at `tick` in `picture`: laser spots, designations,
    /// guided weapons and landings, placed where the picture has their
    /// aircraft, weapons and ground objects.
    fn mark_prims(&self, picture: &RenderSnapshot, tick: u64) -> Vec<ground_marks::Prim> {
        let scene = self.marks.scene(tick);
        if scene == ground_marks::Scene::default() {
            return Vec::new();
        }
        let wants_objects =
            !scene.designations.is_empty() || scene.weapons.iter().any(|w| w.target.is_some());
        let objects: BTreeMap<u32, [f64; 3]> = if wants_objects {
            self.ground_objects(tick, picture).into_iter().collect()
        } else {
            BTreeMap::new()
        };
        let aircraft = |id: u32| everyone(picture).find(|p| p.id == id).map(|p| p.position);
        let projectile = |id: u32| {
            picture
                .projectiles
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.position)
        };
        let object = |id: u32| objects.get(&id).copied();
        let side = |id: u32| self.side(id);
        let name = |id: u32| crate::replay::panels::ascii(&self.label(id));
        ground_marks::draw(
            &scene,
            &ground_marks::Positions {
                aircraft: &aircraft,
                projectile: &projectile,
                object: &object,
            },
            &ground_marks::Who {
                player: self.player,
                side: &side,
                name: &name,
            },
        )
    }

    /// The words beside the marks, placed for a view of `size` and kept
    /// clear of `others`, the name labels already there. They have no
    /// aircraft behind them, so a right-click does not pick them.
    fn mark_labels(
        &self,
        picture: &RenderSnapshot,
        tick: u64,
        camera: &Camera,
        size: [u32; 2],
        others: &[Label],
    ) -> Vec<Label> {
        if !self.marks_shown() {
            return Vec::new();
        }
        let scale = Placement::new(size).scale();
        let font = &self.ownship.font;
        let mut all = others.to_vec();
        for prim in self.mark_prims(picture, tick) {
            let ground_marks::Prim::Text { at, text, color } = prim else {
                continue;
            };
            let Some([x, y]) = camera.project(size, at) else {
                continue;
            };
            let text = crate::replay::panels::ascii(&text);
            let width = FlightCanvas::text_width(font, &text, scale);
            all.push(Label {
                id: u32::MAX,
                // Clear of the shape drawn at the point.
                at: [x - width / 2., y - (font.height as f64 + 26.) * scale],
                size: [width, (font.height as f64 + 1.) * scale],
                text,
                color,
            });
        }
        separate_labels(&mut all, scale);
        all.split_off(others.len())
    }

    /// The cockpit messages on screen at `tick`, oldest first.
    fn messages(&self, tick: u64) -> Vec<String> {
        let life = (crate::flight_ui::MESSAGE_LIFETIME.as_secs_f64()
            * tore_replay::TICKS_PER_SECOND as f64) as u64;
        message_lines(
            self.recording
                .events_between(tick.saturating_sub(life - 1), tick),
        )
    }

    /// Radio, tower and crew lines heard in the last four seconds. Cockpit
    /// messages show as flight shows them instead ([`Self::messages`]).
    fn subtitles(&self, tick: u64) -> Vec<String> {
        let events = self
            .recording
            .events_between(tick.saturating_sub(SUBTITLE_TICKS - 1), tick);
        events
            .iter()
            .filter_map(|e| {
                let event = &e.event;
                // Once per line: at its delivery, not when it was queued
                // or cut off.
                let heard = vocab::heard(event);
                let speaker = event
                    .string(vocab::field::SPEAKER)
                    .map(str::to_owned)
                    .or_else(|| event.subject.map(|id| self.label(id)));
                match event.kind.as_str() {
                    vocab::kind::COMMS_RADIO | vocab::kind::COMMS_TOWER if heard => {
                        Some(match speaker {
                            Some(who) => format!("{who}: {}", event.text),
                            None => event.text.clone(),
                        })
                    }
                    vocab::kind::COMMS_CREW if heard => Some(format!(
                        "{}: {}",
                        event.string(vocab::field::SPEAKER).unwrap_or("Crew"),
                        event.text
                    )),
                    _ => None,
                }
            })
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// The object explicitly viewed, with aircraft selection as the drone fallback.
    fn banner_subject(&mut self, tick: u64) -> Target {
        if self.drone.is_some() {
            return self
                .drone_subject
                .unwrap_or(Target::Aircraft(self.selected));
        }
        if self.view == OBJECT {
            // View from a weapon or ground object keeps that explicit subject.
            return self
                .from
                .filter(|t| !matches!(t, Target::Aircraft(_)))
                .or(self.look_at)
                .unwrap_or_else(|| self.from());
        }
        if self.camera_error.is_some() {
            return Target::Aircraft(self.selected);
        }
        if self.view == flight_views::MISSILE {
            return self
                .selected_missile
                .map_or(Target::Aircraft(self.selected), Target::Missile);
        }
        if self.view == flight_views::WING {
            return Target::Aircraft(self.rig.wingman().unwrap_or(self.selected));
        }
        match self.rig.reference {
            Reference::Target => self
                .view_target(tick)
                .map_or(Target::Aircraft(self.selected), Target::Aircraft),
            Reference::Missile => self
                .rig
                .missile_subject()
                .map_or(Target::Aircraft(self.selected), Target::Missile),
            Reference::Aircraft(id) => Target::Aircraft(id),
            Reference::Player => Target::Aircraft(self.player),
        }
    }

    fn object_banner(&mut self, tick: u64) -> Option<super::banner::Banner> {
        let subject = self.banner_subject(tick);
        if let Target::Ground(id) = subject
            && self.tracks.destroyed(tick).contains(&id)
        {
            return None;
        }
        let ground = if let Target::Ground(id) = subject {
            self.world
                .airport_scene
                .objects
                .iter()
                .find(|o| o.id == id)
                .map(|o| super::banner::Ground {
                    active: self
                        .world
                        .surface
                        .unit(tore_world::surface::UnitId(id))
                        .is_some_and(|u| {
                            u.kind == tore_world::surface::UnitKind::Active || u.route.is_some()
                        }),
                    name: self.object_name(subject),
                    position: o.bounds.center,
                })
        } else {
            None
        };
        let previous = tick.checked_sub(1).and_then(|t| self.playback.frame(t));
        let (frames, at) = self.playback.frame(tick)?;
        self.banner.inspect(
            &self.recording,
            &frames[at],
            previous.as_ref().map(|(frames, at)| &frames[*at]),
            subject,
            ground,
        )
    }

    /// The interface model for this frame.
    fn model(&self, tick: u64) -> Model {
        let active = if self.clock.paused() {
            Control::Pause
        } else {
            match self.clock.direction() {
                Direction::Reverse => Control::Reverse,
                Direction::Forward if self.clock.speed() > 1. => Control::FastForward,
                Direction::Forward => Control::Play,
            }
        };
        let aircraft = match self.info.get(&self.selected) {
            Some(info) if !info.name.is_empty() => {
                format!("{} {}", self.label(self.selected), info.name)
            }
            _ => self.label(self.selected),
        };
        Model {
            feedback: self.feedback_flags.clone(),
            feedback_open: self.feedback_open,
            feedback_selected: self.feedback_selected,
            feedback_scroll: self.feedback_scroll,
            first: self.clock.first(),
            last: self.clock.last(),
            position: self.clock.position(),
            markers: self.markers.clone(),
            active: Some(active),
            pressed: self.bar.pressed,
            hover: self.bar.hover,
            speed: self.clock.label(),
            time: format!(
                "{} / {}",
                clock::timestamp(self.clock.position()),
                clock::timestamp(self.clock.last() as f64)
            ),
            camera: self.camera_label(),
            aircraft,
            timer: self.ui.timer.then(|| {
                format!(
                    "{}   tick {}",
                    clock::timestamp(self.clock.position()),
                    clock::grouped(tick)
                )
            }),
            toast: self
                .toast
                .as_ref()
                .filter(|(_, at)| at.elapsed() < TOAST)
                .map(|(text, _)| text.clone()),
            readout: self.readout.clone(),
            // The Comms panel lists every line, where the subtitles would
            // sit over it.
            subtitles: if self.ui.subtitles && !self.panels.comms_open() {
                self.subtitles(tick)
            } else {
                Vec::new()
            },
            banner: Vec::new(),
            subtitle_lift: crate::flight_ui::message_band(&self.ownship.hud_font).ceil() as i32,
        }
    }

    /// Advances playback by the real time since the last frame and draws
    /// it: the 3D view through the same renderer calls live flight makes,
    /// then the interface. The stretch played sounds on `audio`. `screen`
    /// is a 640x480 layer the app draws over everything, the Graphics or
    /// Sound screen opened from the Escape menu.
    pub fn frame(
        &mut self,
        renderer: &mut Renderer,
        canvas: &mut FlightCanvas,
        shift: bool,
        audio: Option<&crate::audio::Audio>,
        screen: Option<&[u8]>,
    ) -> AppResult<Timing> {
        crate::frame_log::replay_context(self.view, &self.world.theater.name, self.clock.speed());
        self.enter(renderer);
        let now = Instant::now();
        let seconds = self
            .last_frame
            .map_or(0., |last| (now - last).as_secs_f64().min(0.25));
        self.last_frame = Some(now);
        self.scanner.poll(&mut self.tracks);
        self.weather.build(
            &mut self.world,
            &mut self.scenery,
            &self.tracks,
            WEATHER_BUDGET,
        );
        let from = self.clock.position();
        if !self.bar.scrubbing {
            self.clock.advance(seconds);
        }
        let tick = self.clock.tick();
        let picture = self.playback.picture(tick, self.clock.alpha());
        self.refresh_object_menu();
        self.weather
            .seek(&mut self.world, &mut self.scenery, &self.tracks, tick);
        self.follow_city(tick);
        let player = self.player_state(&picture, tick);
        let camera = self.frame_camera(&picture, tick, seconds, shift);
        // The HUD camera and the Sensor View: their own camera, and a
        // picture shown through the CRT pass with their overlay.
        // The HUD camera shows the tape's last picture again between the 24
        // a second it makes (replay::tape_pacing), and while nothing moves.
        let held = self.tape_held(
            renderer,
            now,
            (tick, self.clock.alpha()),
            screen.is_none() && !self.pause.is_open(),
        );
        let taped = if held.is_some() {
            None
        } else {
            self.taped_view(&picture, tick, self.clock.alpha(), renderer.flight_size())
        };
        let camera = match (&taped, &held) {
            (Some(t), _) => copy(&t.camera),
            (None, Some(held)) => copy(held),
            (None, None) => camera,
        };
        let held = held.is_some();
        // The back view from the player's seat draws its airframe but is
        // heard from the cockpit, as flight hears it.
        let seat = crate::mirrors::pilot_eye(
            picture.player.position,
            Body::posed(&picture.player, 1.).basis(),
        );
        let listening = (self.drone.is_none()
            && self.player != convert::NO_PLAYER
            && self.view == 3
            && camera.hidden_target.is_none()
            && feet(camera.position, seat) < 1.)
            .then(|| {
                let mut inside = copy(&camera);
                inside.hidden_target = Some(self.player);
                inside
            });
        let moment = sound::Moment {
            from,
            clock: &self.clock,
            scrubbing: self.bar.scrubbing,
            camera: listening.as_ref().unwrap_or(&camera),
            view: self.drone.is_none().then_some(self.view),
            selected: self.selected,
        };
        self.sound.frame(audio, &moment);
        let size = renderer.flight_size();
        if !held {
            self.feed_renderer(renderer, &picture, tick, &camera, &player, size);
        }
        let composed = Instant::now();
        self.overlay(&picture, tick, &camera, size, canvas);
        if let Some(screen) = screen {
            canvas.legacy_layer(screen, 1.);
        }
        let drawn = Instant::now();
        if let Some(t) = taped {
            self.tape_made(&t.camera, size, (tick, self.clock.alpha()));
            renderer.set_console(t.frame, t.overlay, t.overlay_size);
        } else if held {
            renderer.repeat_console();
        } else {
            self.tape.forget();
        }
        let presented = renderer.draw(
            &canvas.pixels,
            Some((&camera, &self.world, &self.scenery)),
            Some(canvas.size),
            None,
            crate::frame_rate::Surface::Replay {
                right_panel: self.panels.right_open(),
                hud_camera: self.view == tape_views::HUD_CAMERA,
            },
        )?;
        self.camera = camera;
        self.shown = true;
        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        Ok(Timing {
            presented,
            scene: ms(composed - now),
            interface: ms(drawn - composed),
            present: ms(drawn.elapsed()),
        })
    }

    /// Hands the moment to the renderer: the sky, the weather, the smoke, the
    /// effects, every aircraft and ground unit, the combat geometry and the
    /// player's own aircraft, as seen from `camera`. A frame that shows the
    /// HUD camera's last picture again does not do this.
    fn feed_renderer(
        &mut self,
        renderer: &mut Renderer,
        picture: &RenderSnapshot,
        tick: u64,
        camera: &Camera,
        player: &flight::State,
        size: [u32; 2],
    ) {
        self.scenery
            .resolve_palette(&self.world, camera.position[1]);
        self.scenery.set_origin(camera.position);
        let vapor = match self.playback.vapor(tick, &self.ownship, &mut self.scratch) {
            Some(vapor) => crate::vapor_vertices(
                &vapor,
                &self.world,
                player,
                self.ownship.streamer_points(player),
            ),
            None => Vec::new(),
        };
        renderer.vapor(&vapor);
        renderer.set_city_clock(self.city_clock(tick as f64 + self.clock.alpha()));
        let [smoke, contrails] = self.playback.smoke(tick);
        // Chaff and flares flown again from their releases, lit and lighting
        // the scene with every lit afterburner, as live flight draws them.
        let devices = self.devices.at(tick, &self.world);
        renderer.smoke(&self.art.smoke, [smoke, contrails], devices);
        renderer.effects(&self.art.effects, &picture.effects, &picture.marks);
        let (player_outlets, model_outlets) = &self.outlets;
        let mut glows = Vec::new();
        if picture.player.engine.flame {
            glows.extend(render_snapshot::afterburner_glow(
                picture.player.position,
                picture.player.attitude,
                player_outlets,
            ));
        }
        glows.extend(render_snapshot::target_glows(picture, |pose| {
            render_snapshot::engine_outlets(
                pose.aircraft,
                self.ownship.profile.id,
                &self.models,
                model_outlets,
                player_outlets,
            )
        }));
        // AC-130 muzzle flashes from the recorded rounds, on the recorded
        // barrels, as live flight draws them.
        let flash_now = tick as f64 + self.clock.alpha();
        let mounts = crate::gun_flash::mounts(picture, None);
        self.gun_flash
            .set_fire_widths(std::sync::Arc::clone(&self.scenery.fire_widths));
        Self::observe_surface(
            (&self.tracks, &self.recording),
            &mut self.gun_flash,
            picture,
            (tick, flash_now),
            &mounts,
        );
        let guns = self.gun_flash.draw(flash_now, &mounts);
        renderer.emitters(devices, &glows, &guns);
        let destroyed = self.tracks.destroyed(tick);
        let rails = self.rails(tick);
        self.scenery.surface.step_aim(
            picture,
            &self.world.surface,
            &self.tracks.aim_tracks(tick),
            Some(&rails),
        );
        renderer.airports(self.scenery.static_geometry_replay(&destroyed, &rails));
        // Routed units on the move, the men, deck crew and parked aircraft
        // pieces.
        renderer.surface_units(&self.scenery.surface_vertices(
            picture,
            &|id| !destroyed.contains(&id),
            camera,
        ));
        if let Some(art) = &self.art.escape {
            renderer.escapees(
                art,
                &art.vertices_for(
                    picture
                        .pilots
                        .iter()
                        .map(|p| (p.position, p.heading, p.phase)),
                    &self.ownship.palette,
                    camera.position,
                    self.scenery.origin,
                ),
            );
        }
        renderer.dummies(render_snapshot::aircraft_batches(
            picture,
            &self.models,
            &self.art,
            camera,
            &self.world,
            &self.scenery,
        ));
        let mut combat = render_snapshot::combat_geometry(
            picture,
            &self.art,
            &self.ownship,
            player,
            camera,
            &self.world,
            &self.scenery,
        );
        if self.ui.trails {
            self.trails(
                picture,
                tick,
                camera,
                f64::from(size[1]),
                &mut combat.vertices,
            );
        }
        if self.marks_drawn() {
            let prims = self.mark_prims(picture, tick);
            ground_marks::geometry(
                &prims,
                camera,
                self.scenery.origin,
                f64::from(size[1]),
                &mut combat.vertices,
            );
        }
        renderer.combat(&combat);
        renderer.aircraft(
            &self.ownship,
            player,
            &pylons::for_flown(
                &picture.player.stores,
                player,
                &self.ownship,
                &self.art,
                &self.scenery,
            ),
            self.player != convert::NO_PLAYER && camera.hidden_target != Some(self.player),
            camera,
            &self.world,
            &self.scenery,
        );
        // No cockpit, HUD or mirrors in a replay; the cockpit pass keeps its
        // switches between frames, so they are turned off every frame.
        renderer.cockpit(player, camera, false, false, &[], &self.scenery.palette);
    }

    /// Draws the interface over the view: labels, the debug panels, the
    /// layer and the right-click menu, or nothing while it is hidden. Notes
    /// what a right-click can pick in this picture.
    fn overlay(
        &mut self,
        picture: &RenderSnapshot,
        tick: u64,
        camera: &Camera,
        size: [u32; 2],
        canvas: &mut FlightCanvas,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Ui);
        let labels = if self.ui.labels && !self.ui.hidden {
            self.labels(picture, camera, size)
        } else {
            Vec::new()
        };
        self.size = size;
        self.pickables = Self::pickables_for(picture, &labels, &self.ground_objects(tick, picture));
        // The words beside laser spots and guided weapons are drawn with the
        // name labels, with the labels switch, but are not picked.
        let drawn: Vec<Label> = if self.ui.hidden {
            Vec::new()
        } else {
            let marks = self.mark_labels(picture, tick, camera, size, &labels);
            labels.iter().cloned().chain(marks).collect()
        };
        if !self.requests.is_empty() {
            self.open_requests(camera);
        }
        // The cockpit messages print above the transport bar as flight
        // prints them above the window's edge; the debug panels and the
        // subtitles stay clear of the seven lines they can fill.
        let band = crate::flight_ui::message_band(&self.ownship.hud_font);
        let banner = if self.ui.banner && !self.ui.hidden && !self.pause.is_open() {
            self.object_banner(tick)
                .map_or_else(Vec::new, |b| b.lines())
        } else {
            Vec::new()
        };
        let banner_height = overlay::lines_height(&self.ownship.font, banner.len());
        let subtitle_rows = if self.ui.subtitles && !self.panels.comms_open() {
            self.subtitles(tick).len()
        } else {
            0
        };
        let subtitle_band = if subtitle_rows > 0 {
            29 + (subtitle_rows as i32 - 1) * (self.ownship.font.height as i32 + 10)
        } else {
            0
        };
        self.layout =
            panels::REPLAY.clear_of(overlay::BAR_TOP - banner_height - subtitle_band, band);
        let messages = if self.ui.subtitles && !self.ui.hidden {
            self.messages(tick)
        } else {
            Vec::new()
        };
        let hud_color = self.ownship.cockpit_palette(
            &self.world,
            &self.scenery,
            camera.position[1],
            self.local_lighting.1,
        )[usize::from(self.ownship.hud.primary_color)];
        let hud_color = self.local_lighting.0.hud(hud_color, self.local_lighting.1);
        let mut panel_rects = Vec::new();
        let mut menu_rect = None;
        let model = if self.ui.hidden {
            Model::default()
        } else {
            self.panel_layer.fill(0);
            let font = &self.ownship.font;
            let mut data = ReplayData {
                recording: &self.recording,
                trees: &mut self.trees,
                info: &self.info,
                missiles: &self.missiles,
                comms: &self.comms,
                now: tick,
                player: self.player,
            };
            panel_rects = self
                .panels
                .draw(&mut self.panel_layer, font, self.layout, &mut data);
            if let Some(menu) = &self.menu {
                menu.draw(&mut self.panel_layer, font);
                menu_rect = Some(menu.rect());
            }
            self.ui.comms = self.panels.comms_open();
            let mut model = self.model(tick);
            model.banner = banner;
            model.subtitle_lift += banner_height;
            model
        };
        compose(
            canvas,
            size,
            &self.ownship.font,
            &mut self.layer,
            &Interface {
                hidden: self.ui.hidden,
                bar: !self.pause.is_open(),
                labels: &drawn,
                model: &model,
                panels: PanelLayer {
                    pixels: &self.panel_layer,
                    panels: &panel_rects,
                    menu: menu_rect,
                },
                messages: (!messages.is_empty()).then(|| Messages {
                    lines: &messages,
                    font: &self.ownship.hud_font,
                    color: hud_color,
                }),
                compass: self
                    .compass
                    .map(|bearing| (&self.ownship.hud_font, hud_color, bearing)),
            },
        );
        if self.live()
            && !self.pause.is_open()
            && !self.ui.hidden
            && let Some(timer) = self.mission_timer
        {
            let mut blocks: Vec<[f64; 4]> = drawn.iter().map(Label::rect).collect();
            let [w, h] = size.map(f64::from);
            let scale = (w / 640.).min(h / 480.);
            blocks.extend(panel_rects.iter().map(|&(x, y, pw, ph)| {
                [
                    (w - 640. * scale) / 2. + f64::from(x) * scale,
                    (h - 480. * scale) / 2. + f64::from(y) * scale,
                    f64::from(pw) * scale,
                    f64::from(ph) * scale,
                ]
            }));
            if self.view == tape_views::HUD_CAMERA {
                blocks.push([w * 0.70, h * 0.07, w * 0.26, 24.]);
            }

            blocks.push(crate::mission_timer::counter_rect(
                size,
                crate::frame_rate::Surface::Replay {
                    right_panel: self.panels.right_open(),
                    hud_camera: self.view == tape_views::HUD_CAMERA,
                },
            ));
            timer.draw_corner(canvas, &self.ownship.hud_font, &blocks);
        }
        // The Escape menu goes over everything, the hidden interface too,
        // where flight draws its own.
        if self.pause.is_open() {
            self.layer.fill(0);
            let ui = self.ui;
            self.pause.widget.draw(
                &mut self.layer,
                &self.ownship.font,
                &self.pause.tree,
                self.pause.look(),
                &|label| pause::state(&ui, label),
            );
            if self.live()
                && let Some(timer) = self.mission_timer
            {
                timer.draw_menu(&mut self.layer, &self.ownship.font);
            }
            canvas.legacy_layer(&self.layer, 1.);
        }
    }

    /// Saves the last frame's 3D view, without the interface, as a PNG in
    /// `folder`, named after the recording and the tick. Returns the file
    /// written.
    pub fn screenshot(&mut self, renderer: &mut Renderer, folder: &Path) -> AppResult<PathBuf> {
        std::fs::create_dir_all(folder)?;
        let path = screenshot_path(folder, &self.path, self.clock.tick());
        self.save_png(renderer, &path)?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.toast(format!("Saved screenshots/{name}"));
        Ok(path)
    }

    /// Writes the last frame's 3D view, without the interface, to `path` as
    /// a PNG at the view's size (at most 1920x1080).
    pub fn save_png(&self, renderer: &mut Renderer, path: &Path) -> AppResult<()> {
        let [width, height] = renderer.flight_size();
        let pixels = renderer.scene_pixels(
            crate::sim_renderer::ViewKind::Main,
            &self.camera,
            &self.world,
            &self.scenery,
            width,
            height,
            false,
        )?;
        std::fs::write(
            path,
            crate::replay::png::encode_rgba(width, height, &pixels)?,
        )?;
        Ok(())
    }

    /// The camera of the last frame drawn.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }
}

impl Viewer {
    fn scroll_feedback(&mut self, delta: i32) {
        if let Some(f) = self.feedback_flags.get(self.feedback_selected) {
            let max = overlay::feedback_lines(&self.ownship.font, f)
                .len()
                .saturating_sub(27);
            self.feedback_scroll = self
                .feedback_scroll
                .saturating_add_signed(delta as isize)
                .min(max);
        }
    }
    fn seek_feedback(&mut self) {
        self.feedback_scroll = 0;
        if let Some(f) = self.feedback_flags.get(self.feedback_selected) {
            self.clock.seek(f.tick as f64);
            self.clock.pause();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_follows_selected_aircraft_weapons_and_explicit_objects() {
        let dir = TempDir::new("viewer-banner-selection");
        let mut v = viewer(&dir, &Options::default());
        v.clock.seek(180.);
        assert_eq!(v.banner_subject(180), Target::Aircraft(v.player));
        v.select(1);
        assert_eq!(v.banner_subject(180), Target::Aircraft(1));
        v.select(0);
        v.set_view(flight_views::MISSILE);
        assert_eq!(v.banner_subject(180), Target::Missile(f::MISSILE));
        v.from = Some(Target::Ground(9001));
        v.look_at = Some(Target::Aircraft(1));
        v.object_view();
        assert_eq!(v.banner_subject(180), Target::Ground(9001));
        v.from = None;
        v.look_at = Some(Target::Missile(f::MISSILE));
        assert_eq!(v.banner_subject(180), Target::Missile(f::MISSILE));
        v.drone_mode(Some(Mode::Free));
        v.drone_subject = Some(Target::Missile(f::MISSILE));
        assert_eq!(v.banner_subject(180), Target::Missile(f::MISSILE));
        v.select(0);
        assert_eq!(v.banner_subject(180), Target::Aircraft(0));
        assert!(Ui::default().banner);
        assert!(!Ui::parse("none").unwrap().banner);
        assert!(Ui::parse("banner").unwrap().banner);
    }

    #[test]
    fn f12_and_object_menu_follow_overlapping_recorded_weapon_lifetimes() {
        let dir = TempDir::new("viewer-overlapping-weapons");
        let recording = Arc::new(f::recording_with_weapons(dir.path(), "weapons", true));
        let mut v = viewer_recording(recording, &Options::default());
        v.clock.seek(140.);
        let listed: Vec<_> = v
            .object_list_items()
            .iter()
            .filter_map(|i| i.action)
            .collect();
        for id in [7, 9, 13] {
            assert!(listed.contains(&Action::ViewFrom(Target::Missile(id))));
        }
        assert!(
            !listed.contains(&Action::ViewFrom(Target::Missile(14))),
            "gun in object menu"
        );
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(7));
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(9));
        frame_at(&mut v, 150); // New missile does not steal the view.
        assert_eq!(v.selected_missile, Some(9));
        let expired = frame_at(&mut v, 200);
        assert_eq!(v.selected_missile, Some(9));
        assert_eq!(expired.hidden_target, None); // The ended shot holds its chase pose.
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(11));
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(13)); // Bombs participate too.
        assert_eq!(frame_at(&mut v, 200).hidden_projectile, None);
        v.select(1);
        assert!(v.view_key("F12", false, true));
        assert_eq!(frame_at(&mut v, 200).hidden_projectile, None);
        assert_eq!(v.rig.missile_subject(), Some(13));
        assert!(v.view_key("F1", false, true));
        assert_eq!(frame_at(&mut v, 200).hidden_projectile, Some(13));
        v.set_view(flight_views::MISSILE);
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(7));
        frame_at(&mut v, 240);
        assert!(
            !v.object_list_items()
                .iter()
                .any(|i| i.action == Some(Action::ViewFrom(Target::Missile(13))))
        );
        frame_at(&mut v, 300);
        assert_eq!(v.selected_missile, Some(7));
        frame_at(&mut v, 700);
        assert_eq!(v.selected_missile, Some(7));
        assert_eq!(v.camera_error, Some("No active weapon for this view"));
        frame_at(&mut v, 140); // Reverse seek rebuilds the active set.
        assert_eq!(v.selected_missile, Some(7));
        assert!(v.camera_error.is_none());
    }

    #[test]
    fn missile_cycle_preserves_identity_when_shots_spawn_and_expire() {
        let mut selected = active_successor(None, &[10, 20, 30], false);
        assert_eq!(selected, Some(10));
        selected = active_successor(selected, &[10, 20, 30], true);
        assert_eq!(selected, Some(20));
        assert_eq!(
            active_successor(selected, &[10, 20, 30, 40], false),
            Some(20)
        );
        assert_eq!(active_successor(selected, &[10, 30, 40], false), Some(30));
        assert_eq!(active_successor(selected, &[10, 30, 40], true), Some(30));
        assert_eq!(active_successor(Some(40), &[10, 30], false), Some(10));
        assert_eq!(active_successor(Some(40), &[], false), None);
        assert_eq!(active_successor(None, &[50], true), Some(50));
    }

    #[test]
    fn replay_object_list_adds_and_retires_weapons_and_f12_tracks_a_live_id() {
        let dir = TempDir::new("viewer-live-weapons");
        let mut v = viewer(&dir, &Options::default());
        let weapons = |items: Vec<context_menu::Item>| {
            items
                .into_iter()
                .filter_map(|i| {
                    if let Some(Action::ViewFrom(Target::Missile(id))) = i.action {
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        assert!(weapons(v.object_list_items()).is_empty());
        v.clock.seek((f::LAUNCH + 1) as f64);
        assert_eq!(weapons(v.object_list_items()), vec![f::MISSILE]);
        press(&mut v, "F12");
        assert_eq!(v.selected_missile, Some(f::MISSILE));
        v.clock.seek(f::LAST as f64);
        assert!(weapons(v.object_list_items()).is_empty());
        let picture = v.playback.picture(v.clock.tick(), 1.);
        v.frame_camera(&picture, v.clock.tick(), 0., false);
        assert_eq!(v.selected_missile, Some(f::MISSILE));
        // Seeking backward restores the same followed weapon.
        v.clock.seek((f::LAUNCH + 1) as f64);
        let picture = v.playback.picture(v.clock.tick(), 1.);
        v.frame_camera(&picture, v.clock.tick(), 0., false);
        assert_eq!(v.selected_missile, Some(f::MISSILE));
    }

    #[test]
    fn weapon_chase_matches_live_f12_and_director_through_expiry_and_reverse() {
        let dir = TempDir::new("weapon-chase");
        let mut v = viewer(&dir, &Options::default());
        v.clock.seek(180.);
        v.select(1);
        v.set_view(flight_views::MISSILE);
        let picture = v.playback.picture(180, 1.);
        let replay = v.frame_camera(&picture, 180, 0., false);
        let scene = v.scene(&picture, 180);
        let mut live = Rig::default();
        live.follow_missiles_of(1);
        live.select(Reference::Aircraft(1));
        let expected = live
            .camera(flight_views::MISSILE, &scene, Camera::new(), [0.; 2], 1.)
            .unwrap();
        let director = v.director_weapon_camera(&picture, 180, 1, [0.; 3]);
        for camera in [&replay, &director] {
            assert_eq!(camera.position, expected.position);
            assert_eq!(camera.yaw, expected.yaw);
            assert_eq!(camera.pitch, expected.pitch);
            assert_eq!(camera.hidden_projectile, None);
            assert!(
                camera
                    .project([1920, 1080], picture.projectiles[0].position)
                    .is_some()
            );
        }
        let offset = v.director_weapon_camera(&picture, 180, 1, [6., 12., -40.]);
        assert_ne!(offset.position, director.position);
        assert!(
            offset
                .project([1920, 1080], picture.projectiles[0].position)
                .is_some()
        );
        let mut ended = v.playback.picture(f::IMPACT, 1.);
        let mut older = picture.projectiles[0].clone();
        older.id = f::MISSILE - 1;
        ended.projectiles.push(older); // Still flying, but must not take over.
        let post = v.director_weapon_camera(&ended, f::IMPACT, 1, [0.; 3]);
        assert_eq!(post.hidden_target, Some(1));
        let backward = v.director_weapon_camera(&picture, 180, 1, [0.; 3]);
        assert_eq!(backward.position, director.position);
    }

    #[test]
    fn f12_hold_reconstructs_exact_boundaries_reverse_and_view_interruptions() {
        let dir = TempDir::new("f12-hold");
        let mut v = viewer(&dir, &Options::default());
        v.clock.seek((f::IMPACT - 1) as f64);
        v.set_view(flight_views::MISSILE);
        let last = frame_at(&mut v, f::IMPACT - 1);
        let end = f::IMPACT + flight_views::weapon_hold_ticks();
        for tick in [f::IMPACT, end - 1, f::IMPACT + 60, f::IMPACT + 60] {
            let held = frame_at(&mut v, tick);
            assert_eq!(held.position, last.position);
            assert_eq!((held.yaw, held.pitch), (last.yaw, last.pitch));
            assert!(v.camera_error.is_none());
        }
        assert_eq!(frame_at(&mut v, end).hidden_target, Some(v.selected));
        assert_eq!(frame_at(&mut v, end - 1).position, last.position);
        assert_ne!(frame_at(&mut v, f::IMPACT - 2).position, last.position);
        frame_at(&mut v, f::IMPACT + 30);
        press(&mut v, "F10");
        assert_ne!(frame_at(&mut v, f::IMPACT + 30).position, last.position);
        // Directly opening F12 inside the hold needs no previously drawn pose.
        v.selected_missile = None;
        v.clock.seek((f::IMPACT + 30) as f64);
        v.set_view(flight_views::MISSILE);
        assert_eq!(frame_at(&mut v, f::IMPACT + 30).position, last.position);
        press(&mut v, "F12"); // No live successor: leave the hold immediately.
        assert_eq!(v.view, 0);
        assert_eq!(
            frame_at(&mut v, f::IMPACT + 30).hidden_target,
            Some(v.selected)
        );
    }

    #[test]
    fn f12_approach_is_identical_on_direct_seek_forward_and_reverse() {
        let dir = TempDir::new("f12-approach");
        let template = f::recording(dir.path(), "template");
        let path = dir.path().join("approach.tore-replay");
        let mut writer =
            tore_replay::Writer::create(&path, &tore_replay::Header::default()).unwrap();
        for aircraft in template.aircraft() {
            writer.register_aircraft(aircraft).unwrap();
        }
        for weapon in template.weapons() {
            writer.register_weapon(weapon).unwrap();
        }
        for tick in f::FIRST..=f::LAST {
            let mut frame = f::frame(tick);
            for p in &mut frame.projectiles {
                if p.id == f::MISSILE {
                    let target = frame.aircraft.iter().find(|a| a.id == 0).unwrap().position;
                    p.position = [
                        target[0],
                        target[1],
                        target[2] - (f::IMPACT - tick) as f64 * 4.,
                    ];
                    p.direction = [0., 0., 1.];
                    p.speed = 480.;
                    if tick > 200 {
                        p.target = None;
                    } // Lost guidance must not lose the camera's aim.
                }
            }
            writer.push(&frame).unwrap();
        }
        writer.finish(&tore_replay::Footer::default()).unwrap();
        let recording = Arc::new(Recording::open(path).unwrap());
        let mut v = viewer_recording(recording.clone(), &Options::default());
        v.clock.seek(247.);
        v.set_view(flight_views::MISSILE);
        let before = frame_at(&mut v, 247);
        let stopped = frame_at(&mut v, 248); // 82 ticks * 4 feet = 328 feet.
        assert_ne!(before.position, stopped.position);
        let mut live = Rig::default();
        live.follow_missiles_of(1);
        live.select(Reference::Aircraft(1));
        let launched = v.playback.picture(f::LAUNCH, 1.);
        live.observe(&v.scene(&launched, f::LAUNCH));
        let lost = v.playback.picture(248, 1.);
        assert_eq!(lost.projectiles[0].target, None);
        let scene = live.prepare(v.scene(&lost, 248), flight_views::MISSILE, &v.world);
        let live_camera = live
            .camera(flight_views::MISSILE, &scene, Camera::new(), [0.; 2], 1.)
            .unwrap();
        assert_eq!(live_camera.position, stopped.position);
        for tick in [249, 329, 330, 509, 300, 248] {
            assert_eq!(frame_at(&mut v, tick).position, stopped.position);
        }
        assert_eq!(frame_at(&mut v, 247).position, before.position);
        assert_eq!(frame_at(&mut v, 510).hidden_target, Some(v.selected));
        let mut seek = viewer_recording(
            recording,
            &Options {
                tick: Some(350),
                view: Some(flight_views::MISSILE),
                ..Options::default()
            },
        );
        assert_eq!(frame_at(&mut seek, 350).position, stopped.position);
        assert_eq!(frame_at(&mut seek, 247).position, before.position);
    }

    #[test]
    fn f12_hold_pause_and_playback_rate_use_recorded_time() {
        let dir = TempDir::new("f12-clock-hold");
        let mut v = viewer(&dir, &Options::default());
        v.clock.seek((f::IMPACT - 1) as f64);
        v.set_view(flight_views::MISSILE);
        let last = frame_at(&mut v, f::IMPACT - 1);
        v.clock.seek(f::IMPACT as f64);
        v.clock.pause();
        assert!(!v.clock.advance(10.));
        let tick = v.clock.tick();
        assert_eq!(frame_at(&mut v, tick).position, last.position);
        assert!(v.clock.set_speed(2.));
        v.clock.advance(0.5); // One game second.
        let tick = v.clock.tick();
        assert_eq!(tick, f::IMPACT + 120);
        assert_eq!(frame_at(&mut v, tick).position, last.position);
        v.clock.advance(0.25); // Exactly 1.5 game seconds.
        let tick = v.clock.tick();
        assert_eq!(frame_at(&mut v, tick).hidden_target, Some(v.selected));
        assert!(v.clock.set_speed(-2.));
        v.clock.advance(0.25);
        let tick = v.clock.tick();
        assert_eq!(frame_at(&mut v, tick).position, last.position);
    }

    #[test]
    fn modified_f12_holds_its_owner_and_reselects_when_aircraft_changes() {
        let dir = TempDir::new("f12-owner-hold");
        let mut v = viewer(&dir, &Options::default());
        v.clock.seek((f::IMPACT - 1) as f64);
        v.select(1);
        assert!(v.view_key("F12", false, true));
        let last = frame_at(&mut v, f::IMPACT - 1);
        assert_eq!(frame_at(&mut v, f::IMPACT + 60).position, last.position);
        assert_eq!(
            v.banner_subject(f::IMPACT + 60),
            Target::Missile(f::MISSILE)
        );
        v.select(0); // This aircraft never fired a missile in the fixture.
        assert_eq!(v.selected_missile, None);
        assert_eq!(frame_at(&mut v, f::IMPACT + 60).hidden_target, Some(0));
    }

    #[test]
    fn labels_over_close_aircraft_are_lifted_apart() {
        let label = |id: u32, x: f64, y: f64| Label {
            id,
            text: format!("A{id}"),
            at: [x, y],
            size: [40., 10.],
            color: [255, 255, 255],
        };
        let overlaps = |a: &Label, b: &Label| {
            let [ax, ay, aw, ah] = a.rect();
            let [bx, by, bw, bh] = b.rect();
            ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
        };
        let mut labels = vec![
            label(0, 100., 100.),
            label(1, 110., 104.),
            label(2, 100., 100.),
            label(3, 400., 300.),
        ];
        separate_labels(&mut labels, 1.);
        for i in 0..labels.len() {
            for j in i + 1..labels.len() {
                assert!(!overlaps(&labels[i], &labels[j]), "{i} and {j} overlap");
            }
        }
        // The first label and a far one stay where they were.
        assert_eq!(labels[0].at, [100., 100.]);
        assert_eq!(labels[3].at, [400., 300.]);
        // Labels that were already apart are untouched.
        let mut apart = vec![label(0, 0., 0.), label(1, 100., 0.)];
        separate_labels(&mut apart, 2.);
        assert_eq!((apart[0].at, apart[1].at), ([0., 0.], [100., 0.]));
    }

    #[test]
    fn hiding_the_interface_leaves_nothing_over_the_view() {
        let font = tore_formats::font::Font {
            height: 5,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 4,
                    pixels: vec![(0, 0), (1, 1), (2, 2)],
                })
                .collect(),
        };
        let labels = [Label {
            id: 0,
            text: "[YOU]".into(),
            at: [800., 300.],
            size: [20., 6.],
            color: [110, 170, 255],
        }];
        let model = Model {
            last: 1_000,
            speed: "1x".into(),
            timer: Some("00:00.0".into()),
            subtitles: vec!["You: 'Fox two'".into()],
            ..Default::default()
        };
        // A panel on the left and a menu over the bar's corner.
        let mut panel_pixels = vec![0; overlay::WIDTH * overlay::HEIGHT * 4];
        let panel = (6, 28, 254, 398);
        let menu = (500, 400, 100, 60);
        for (x, y, w, h) in [panel, menu] {
            for yy in y..y + h {
                for xx in x..x + w {
                    panel_pixels[(yy * 640 + xx) as usize * 4..][..4]
                        .copy_from_slice(&[1, 2, 3, 255]);
                }
            }
        }
        let mut interface = Interface {
            hidden: false,
            bar: true,
            labels: &labels,
            model: &model,
            panels: PanelLayer {
                pixels: &panel_pixels,
                panels: &[panel],
                menu: Some(menu),
            },
            messages: None,
            compass: None,
        };
        let mut canvas = FlightCanvas::default();
        let mut layer = vec![0; overlay::WIDTH * overlay::HEIGHT * 4];
        compose(&mut canvas, [1280, 720], &font, &mut layer, &interface);
        let covered = |c: &FlightCanvas| c.pixels.chunks_exact(4).filter(|p| p[3] != 0).count();
        assert!(covered(&canvas) > 10_000);
        let at =
            |c: &FlightCanvas, x: usize, y: usize| c.pixels[(y * 1280 + x) * 4..][..4].to_vec();
        assert_eq!(at(&canvas, 800, 300), [110, 170, 255, 255]);
        // The layer is 960 wide on this view, 160 in from the left: the
        // panel, and the menu drawn over the transport bar.
        assert_eq!(at(&canvas, 160 + 100, 300), [1, 2, 3, 255]);
        assert_eq!(at(&canvas, 160 + 825, 670), [1, 2, 3, 255]);
        // Hidden, even with stale layers, nothing is drawn.
        interface.hidden = true;
        compose(&mut canvas, [1280, 720], &font, &mut layer, &interface);
        assert_eq!(canvas.size, [1280, 720]);
        assert_eq!(covered(&canvas), 0);
    }

    use crate::replay::fixture as f;
    use crate::replay::tests::TempDir;
    use tore_session::wire::messages::Subject;

    /// A viewer over the synthetic recording, with synthetic art and world.
    fn viewer(dir: &TempDir, options: &Options) -> Viewer {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let recording = Arc::new(f::recording(dir.path(), &format!("viewer-{n}")));
        viewer_recording(recording, options)
    }
    fn viewer_recording(recording: Arc<Recording>, options: &Options) -> Viewer {
        viewer_growing(recording, options, None)
    }
    fn viewer_growing(
        recording: Arc<Recording>,
        options: &Options,
        growth: Option<Growth>,
    ) -> Viewer {
        let art = CombatArt::synthetic(BTreeMap::new());
        let mut ownship = crate::combat_view::render_hash_tests::hornet_airframe(true);
        // Menus measure their text, so the font needs its glyphs.
        ownship.font = crate::replay::panels::tests::font();
        Viewer::assemble(
            Path::new("/x/test.tore-replay"),
            recording,
            (
                tore_world::test_support::terrain(),
                crate::scenery::tests::scenery(),
            ),
            (ownship, Vec::new(), art),
            options,
            growth,
        )
        .unwrap()
    }

    fn press(viewer: &mut Viewer, key: &str) -> Command {
        // A fresh press, with any previous press of this key released first.
        viewer.key(key, false, false, false);
        viewer.key(key, true, false, false)
    }

    #[test]
    fn feedback_marker_and_list_seek_to_opening_tick_not_submit() {
        let dir = TempDir::new("viewer-feedback");
        let path = dir.path().join("feedback.tore-replay");
        let mut writer =
            tore_replay::Writer::create(&path, &tore_replay::Header::default()).unwrap();
        for tick in 0..=120 {
            let mut frame = tore_replay::Frame {
                tick,
                ..Default::default()
            };
            if tick == 120 {
                frame.events.push(
                    tore_replay::feedback::Feedback {
                        tick: 12,
                        submit_tick: 120,
                        text: "bump".into(),
                        ..Default::default()
                    }
                    .event(),
                );
            }
            writer.push(&frame).unwrap();
        }
        writer.finish(&tore_replay::Footer::default()).unwrap();
        let mut v = viewer_recording(
            Arc::new(Recording::open(path).unwrap()),
            &Options::default(),
        );
        assert_eq!(
            v.markers,
            vec![Marker {
                tick: 12,
                kind: MarkerKind::Feedback
            }]
        );
        v.feedback_open = true;
        v.clock.end();
        press(&mut v, "Enter");
        assert_eq!(v.clock.tick(), 12);
        assert!(v.clock.paused());
        assert_eq!(v.model(12).feedback[0].text, "bump");
        press(&mut v, "Escape");
        assert!(!v.feedback_open);
    }

    #[test]
    fn hide_ui_hides_the_designation_square_with_the_labels() {
        let dir = TempDir::new("viewer-marks-hidden");
        let mut v = viewer(&dir, &Options::default());
        v.marks = ground_marks::Marks::from_events(&[TimedEvent {
            tick: 0,
            event: tore_replay::Event::new(vocab::kind::PLAYER_DESIGNATE)
                .with_subject(0)
                .with_object(0x5000_0001)
                .with(vocab::field::CHANNEL, vocab::designate::RADAR),
        }]);
        v.ui.marks = true;
        assert!(v.marks_drawn(), "the square shows with the interface");
        press(&mut v, "h");
        assert!(v.ui.hidden && !v.marks_drawn(), "H hides the square too");
        press(&mut v, "h");
        assert!(!v.ui.hidden && v.marks_drawn(), "and H brings it back");
    }

    #[test]
    fn keys_drive_playback() {
        let dir = TempDir::new("viewer-keys");
        let mut v = viewer(&dir, &Options::default());
        assert!(!v.clock.paused());
        press(&mut v, "Space");
        assert!(v.clock.paused());
        // Held keys do not toggle again.
        v.key("Space", true, true, false);
        assert!(v.clock.paused());
        // Paused, the arrows step a tick; with Shift they jump 30 seconds.
        press(&mut v, "ArrowRight");
        assert_eq!(v.clock.position(), (f::FIRST + 1) as f64);
        v.key("ArrowRight", true, true, false);
        assert_eq!(v.clock.position(), (f::FIRST + 2) as f64);
        press(&mut v, "ArrowLeft");
        v.key("ArrowRight", true, false, true);
        assert_eq!(v.clock.position(), f::LAST as f64);
        press(&mut v, "Home");
        assert_eq!(v.clock.position(), f::FIRST as f64);
        // Playing, they jump 5 seconds.
        press(&mut v, "l");
        press(&mut v, "ArrowRight");
        assert_eq!(v.clock.position(), (f::FIRST + 600) as f64);
        press(&mut v, "l");
        assert_eq!(v.clock.speed(), 2.);
        press(&mut v, "ArrowUp");
        assert_eq!(v.clock.speed(), 4.);
        press(&mut v, "ArrowDown");
        press(&mut v, "ArrowDown");
        press(&mut v, "ArrowDown");
        assert_eq!(v.clock.speed(), 0.75);
        press(&mut v, "j");
        assert_eq!(
            (v.clock.direction(), v.clock.speed()),
            (Direction::Reverse, 1.)
        );
        press(&mut v, "k");
        assert!(v.clock.paused());
        press(&mut v, "End");
        assert_eq!(v.clock.position(), f::LAST as f64);
        // Markers: order, launch, bookmark and kill.
        press(&mut v, "Home");
        for expected in [f::ORDER, f::LAUNCH, f::BOOKMARK, f::KILL] {
            press(&mut v, "PageDown");
            assert_eq!(v.clock.position(), expected as f64);
        }
        press(&mut v, "PageDown");
        assert!(v.toast.is_some());
        press(&mut v, "PageUp");
        assert_eq!(v.clock.position(), f::BOOKMARK as f64);
    }

    #[test]
    fn rebound_drone_keys_move_while_paused_and_release_after_modifier_or_focus_loss() {
        use winit::keyboard::ModifiersState as M;
        let dir = TempDir::new("viewer-drone-bindings");
        let mut v = viewer(&dir, &Options::default());
        let profile = tore_input::Profile::parse("tore-input 1\ndisable replay-keyboard w\nbind replay-keyboard Ctrl-u drone-forward hold\n").unwrap();
        v.set_input_profile(&profile);
        v.clock.pause();
        v.drone_mode(Some(Mode::Free));
        let tick = v.clock.tick();
        let picture = v.playback.picture(tick, 0.);
        let before = v.frame_camera(&picture, tick, 0., false).position;
        assert!(!v.bound_key("w", true, false, M::empty()));
        assert!(v.bound_key("u", true, false, M::CONTROL));
        let moved = v.frame_camera(&picture, tick, 0.1, false).position;
        assert!((0..3).map(|i| (moved[i] - before[i]).powi(2)).sum::<f64>() > 100.);
        assert!(v.clock.paused());
        assert!(v.bound_key("u", false, false, M::empty()));
        assert_eq!(v.frame_camera(&picture, tick, 0.1, false).position, moved);
        v.bound_key("u", true, false, M::CONTROL);
        v.release();
        assert!(!v.bound_key("u", true, true, M::CONTROL));
        assert!(v.held.is_empty());
    }

    #[test]
    fn keys_switch_views_aircraft_and_the_interface() {
        let dir = TempDir::new("viewer-views");
        let mut v = viewer(&dir, &Options::default());
        assert_eq!((v.view, v.selected), (EXTERNAL, 0));
        press(&mut v, "F2");
        assert_eq!(v.view, 3);
        press(&mut v, "F12");
        assert_eq!(v.view, flight_views::MISSILE);
        press(&mut v, "F11");
        assert_eq!(v.view, flight_views::MISSILE);
        // Tab walks the aircraft recorded now; the late one is not there yet.
        for expected in [1, 2, 0, 1] {
            press(&mut v, "Tab");
            assert_eq!(v.selected, expected);
        }
        v.key("Tab", true, false, true);
        assert_eq!(v.selected, 0);
        v.clock.seek(f::LATE_FROM as f64);
        v.key("Tab", true, false, true);
        assert_eq!(v.selected, f::LATE);
        // Backquote: follow drone, free drone, back to the view.
        press(&mut v, "`");
        assert_eq!(v.drone.as_ref().map(|d| d.mode), Some(Mode::Follow));
        press(&mut v, "`");
        assert_eq!(v.drone.as_ref().map(|d| d.mode), Some(Mode::Free));
        press(&mut v, "`");
        assert!(v.drone.is_none());
        assert_eq!(v.view, flight_views::MISSILE);
        // Drone keys are held while pressed.
        press(&mut v, "w");
        assert!(v.held.contains("w"));
        v.key("w", false, false, false);
        assert!(v.held.is_empty());
        press(&mut v, "q");
        v.release();
        assert!(v.held.is_empty());
        // The interface parts.
        let before = v.ui;
        for key in ["n", "t", "c", "r"] {
            press(&mut v, key);
        }
        assert_eq!(
            (v.ui.labels, v.ui.timer, v.ui.comms, v.ui.trails),
            (!before.labels, !before.timer, !before.comms, !before.trails)
        );
        v.ui.trails = false;
        v.key("r", true, false, true);
        assert!(v.ui.trails);
        assert_eq!(trails::LENGTHS[v.ui.trail_length], 60);
        // B switches the laser and bomb marks, on at first.
        assert!(before.marks);
        press(&mut v, "b");
        assert!(!v.ui.marks);
        press(&mut v, "b");
        assert!(v.ui.marks);
        assert_eq!(press(&mut v, "p"), Command::Screenshot);
        // H hides everything. Esc opens the menu over the hidden
        // interface, with the pointer; closing it leaves the interface
        // hidden.
        press(&mut v, "h");
        assert!(v.ui.hidden && !v.pointer_visible());
        assert_eq!(press(&mut v, "Escape"), Command::None);
        assert!(v.pause.is_open() && v.ui.hidden && v.pointer_visible());
        assert_eq!(press(&mut v, "Escape"), Command::None);
        assert!(!v.pause.is_open() && v.ui.hidden && !v.pointer_visible());
    }

    /// Clicks at a point of the 640x480 layer on a 4:3 view twice its size.
    fn click(v: &mut Viewer, (x, y): (f64, f64)) -> Command {
        let (at, size) = (Some([x * 2., y * 2.]), [1280, 960]);
        v.left(true, at, size);
        v.left(false, at, size)
    }

    /// Where the Escape menu's tabs, rows and bottom buttons are drawn.
    const QUESTION_TAB: (f64, f64) = (10., 10.);
    // Tabs are 7 layer pixels a letter plus 16 wide, from x = 4: ? spans
    // 4 to 27, Control 27 to 92 and Pref 92 to 136.
    const CONTROL_TAB: (f64, f64) = (40., 10.);
    const PREF_TAB: (f64, f64) = (110., 10.);
    const RESUME: (f64, f64) = (100., 455.);
    fn menu_row(index: usize) -> (f64, f64) {
        (200., 55. + 19. * index as f64)
    }

    #[test]
    fn escape_opens_a_menu_that_pauses_and_resumes_as_before() {
        let dir = TempDir::new("viewer-escape-menu");
        let mut v = viewer(&dir, &Options::default());
        // Playing backwards at 2x, a drone key held.
        press(&mut v, "j");
        press(&mut v, "ArrowUp");
        press(&mut v, "w");
        assert_eq!(press(&mut v, "Escape"), Command::None);
        assert!(v.pause.is_open() && v.clock.paused() && v.held.is_empty());
        // It takes every key and click while open; Space and Enter choose
        // the focused row, as in flight.
        for key in ["h", "l", "w", "Tab", "n"] {
            assert_eq!(press(&mut v, key), Command::None);
        }
        assert_eq!(press(&mut v, "p"), Command::None);
        assert!(v.pause.is_open() && v.clock.paused() && !v.ui.hidden && v.held.is_empty());
        assert!(v.ui.labels && v.selected == 0);
        v.wheel(3);
        assert_eq!(v.zoom, 1.);
        // Resume plays on in the same direction and speed.
        assert_eq!(click(&mut v, RESUME), Command::Click);
        assert!(!v.pause.is_open() && !v.clock.paused());
        assert_eq!(
            (v.clock.direction(), v.clock.speed()),
            (Direction::Reverse, 2.)
        );
        // Paused before, paused after; Esc at the top level resumes too.
        press(&mut v, "k");
        press(&mut v, "Escape");
        press(&mut v, "Escape");
        assert!(!v.pause.is_open() && v.clock.paused());
        // An open right-click menu closes first.
        press(&mut v, "m");
        assert!(v.menu.is_some());
        press(&mut v, "Escape");
        assert!(v.menu.is_none() && !v.pause.is_open());
        press(&mut v, "Escape");
        assert!(v.pause.is_open());
    }

    #[test]
    fn the_escape_menu_ends_exits_opens_screens_and_sets_the_clock() {
        let dir = TempDir::new("viewer-escape-rows");
        let mut v = viewer(&dir, &Options::default());
        press(&mut v, "Escape");
        // ? > End Replay and Exit to Desktop, by mouse and by keyboard.
        assert_eq!(click(&mut v, menu_row(0)), Command::Leave);
        assert_eq!(click(&mut v, menu_row(1)), Command::Exit);
        press(&mut v, "ArrowUp");
        assert_eq!(press(&mut v, "Enter"), Command::Leave);
        // The Control tab opens the controls screen and stays on ?.
        assert_eq!(click(&mut v, CONTROL_TAB), Command::Controls);
        assert_eq!(click(&mut v, menu_row(0)), Command::Leave);
        // Pref > Graphics... and Sound... open those screens.
        assert_eq!(click(&mut v, PREF_TAB), Command::Click);
        assert_eq!(click(&mut v, menu_row(0)), Command::Graphics);
        assert_eq!(click(&mut v, menu_row(1)), Command::Sound);
        assert!(v.pause.is_open());
        // Time > 2x: the clock runs at 2x forwards once the menu closes.
        assert_eq!(click(&mut v, menu_row(2)), Command::Click);
        assert_eq!(click(&mut v, menu_row(3)), Command::Click);
        assert!(v.pause.is_open() && v.clock.paused());
        assert_eq!(
            (v.clock.direction(), v.clock.speed()),
            (Direction::Forward, 2.)
        );
        assert_eq!(click(&mut v, RESUME), Command::Click);
        assert!(!v.clock.paused() && v.clock.speed() == 2.);
        // Slow-motion, 1x, 4x and 8x set their speeds.
        press(&mut v, "Escape");
        for (row, speed) in [(1, 0.5), (2, 1.), (4, 4.), (5, 8.)] {
            click(&mut v, menu_row(row));
            assert_eq!(v.clock.speed(), speed);
        }
        // Paused closes the menu, pausing a replay that was playing and
        // playing one that was paused.
        click(&mut v, menu_row(0));
        assert!(!v.pause.is_open() && v.clock.paused());
        press(&mut v, "Escape");
        click(&mut v, menu_row(0));
        assert!(!v.pause.is_open() && !v.clock.paused() && v.clock.speed() == 8.);
        // Back up to the Pref rows, then the ? tab.
        press(&mut v, "Escape");
        press(&mut v, "Escape");
        assert_eq!(v.pause.widget.rows(&v.pause.tree).len(), 9);
        click(&mut v, QUESTION_TAB);
        assert_eq!(
            v.pause.widget.rows(&v.pause.tree)[0].label,
            pause::END_REPLAY
        );
    }

    #[test]
    fn escape_menu_rows_switch_the_interface_parts() {
        let dir = TempDir::new("viewer-escape-parts");
        let mut v = viewer(&dir, &Options::default());
        press(&mut v, "Escape");
        click(&mut v, PREF_TAB);
        let before = v.ui;
        let state = |v: &Viewer, label| pause::state(&v.ui, label);
        assert_eq!(state(&v, pause::COMMS), Some("Off"));
        for row in 3..8 {
            assert_eq!(click(&mut v, menu_row(row)), Command::Click);
        }
        assert_eq!(
            (v.ui.labels, v.ui.timer, v.ui.trails, v.ui.marks, v.ui.comms),
            (
                !before.labels,
                !before.timer,
                !before.trails,
                !before.marks,
                !before.comms
            )
        );
        assert_eq!(state(&v, pause::MARKS), Some("Off"));
        assert!(v.panels.comms_open());
        assert_eq!(state(&v, pause::COMMS), Some("On"));
        assert_eq!(state(&v, pause::BANNER), Some("On"));
        assert_eq!(click(&mut v, menu_row(8)), Command::Click);
        assert_eq!(state(&v, pause::BANNER), Some("Off"));
        assert!(!v.ui.banner);
        assert_eq!(state(&v, "Graphics..."), None);
    }

    #[test]
    fn replay_menu_option_opens_the_menu_paused_at_a_page() {
        let dir = TempDir::new("viewer-escape-start");
        let options = Options {
            menu: Some(pause::Start::Time),
            ..Options::default()
        };
        let mut v = viewer(&dir, &options);
        assert!(v.pause.is_open() && v.clock.paused());
        assert_eq!(v.pause.widget.rows(&v.pause.tree)[5].label, "8x");
        // It was playing, so closing it plays.
        press(&mut v, "Escape");
        press(&mut v, "Escape");
        assert!(!v.pause.is_open() && !v.clock.paused());
        let options = Options {
            menu: Some(pause::Start::Help),
            ..Options::default()
        };
        let v = viewer(&dir, &options);
        assert!(v.pause.widget.help);
    }

    #[test]
    fn clicks_on_the_bar_play_scrub_and_switch() {
        let dir = TempDir::new("viewer-clicks");
        let mut v = viewer(&dir, &Options::default());
        // A 4:3 view at twice the layer's size.
        let size = [1280, 960];
        let at = |control: Control| {
            let (x, y, w, h) = overlay::rect(control);
            Some([f64::from(x * 2 + w), f64::from(y * 2 + h)])
        };
        let click = |v: &mut Viewer, control| {
            v.left(true, at(control), size);
            v.left(false, at(control), size);
        };
        click(&mut v, Control::Pause);
        assert!(v.clock.paused());
        click(&mut v, Control::Play);
        assert!(!v.clock.paused());
        click(&mut v, Control::End);
        assert_eq!(v.clock.position(), f::LAST as f64);
        click(&mut v, Control::StepBack);
        assert_eq!(v.clock.position(), (f::LAST - 1) as f64);
        // Press on the timeline's middle, drag to its start, let go.
        let y = at(Control::Timeline).unwrap()[1];
        v.left(true, Some([640., y]), size);
        let middle = (f::FIRST + f::LAST) as f64 / 2.;
        assert!((v.clock.position() - middle).abs() < 1.);
        v.pointer(Some([16., y]), [0.; 2], size, [0.; 2]);
        assert_eq!(v.clock.position(), f::FIRST as f64);
        v.left(false, Some([16., y]), size);
        v.pointer(Some([640., y]), [0.; 2], size, [0.; 2]);
        assert_eq!(v.clock.position(), f::FIRST as f64);
        // The camera button steps through the views, then the drones.
        click(&mut v, Control::Camera);
        assert_eq!(v.view, flight_views::MISSILE);
        click(&mut v, Control::Camera);
        assert_eq!(v.drone.as_ref().map(|d| d.mode), Some(Mode::Follow));
        click(&mut v, Control::Camera);
        assert_eq!(v.drone.as_ref().map(|d| d.mode), Some(Mode::Free));
        click(&mut v, Control::Camera);
        assert_eq!((v.drone.is_none(), v.view), (true, 0));
        click(&mut v, Control::Aircraft);
        assert_eq!(v.selected, 1);
        // Hidden, the bar takes no clicks.
        click(&mut v, Control::HideUi);
        assert!(v.ui.hidden);
        let paused = v.clock.paused();
        click(&mut v, Control::Play);
        click(&mut v, Control::Pause);
        assert_eq!(v.clock.paused(), paused);
        // Right-drag turns the view, and opens no menu.
        v.ui.hidden = false;
        v.right(true, [100., 100.], Some([100., 100.]), size, 4.);
        v.pointer(None, [150., 80.], size, [0.01, 0.01]);
        assert!((v.look[0] - 0.5).abs() < 1e-6 && (v.look[1] - 0.2).abs() < 1e-6);
        v.right(false, [150., 80.], Some([150., 80.]), size, 4.);
        v.pointer(None, [400., 80.], size, [0.01, 0.01]);
        assert!((v.look[0] - 0.5).abs() < 1e-6);
        assert!(v.menu.is_none());
    }

    #[test]
    fn views_follow_recorded_targets_and_fall_back_outside() {
        let dir = TempDir::new("viewer-cameras");
        let mut v = viewer(&dir, &Options::default());
        let camera = |v: &mut Viewer, tick: u64| {
            let picture = v.playback.picture(tick, 1.);
            v.frame_camera(&picture, tick, 0., false)
        };
        // Before the player has a target the target view says so once and
        // shows the player from outside.
        press(&mut v, "F7");
        let outside = camera(&mut v, 50);
        assert_eq!(v.camera_error, Some("No current target for this view"));
        assert!(v.toast.is_some());
        let external = f::position(0, 50);
        let offset: f64 = (0..3)
            .map(|i| (outside.position[i] - external[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(offset > 100. && offset < 250., "{offset}");
        // From tick 100 the player targets aircraft 2: the view looks from
        // the player towards it.
        let toward = camera(&mut v, 200);
        assert_eq!(v.camera_error, None);
        let forward = Basis::new(f64::from(toward.yaw), f64::from(toward.pitch), 0.).forward;
        let [a, b] = [f::position(0, 200), f::position(2, 200)];
        let d: [f64; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(tore_sim::attitude::dot(forward, d.map(|x| x / length)) > 0.99);
        // Aircraft 1's own target is the player; its missile view works
        // while its missile flies.
        press(&mut v, "Tab");
        press(&mut v, "F12");
        camera(&mut v, f::LAUNCH + 20);
        assert_eq!(v.camera_error, None);
        camera(&mut v, f::IMPACT + 20);
        assert_eq!(v.camera_error, None);
        camera(&mut v, f::IMPACT + flight_views::weapon_hold_ticks());
        assert_eq!(v.camera_error, Some("No active weapon for this view"));
        // The front view sits in the aircraft and hides it.
        press(&mut v, "F1");
        let inside = camera(&mut v, 300);
        assert_eq!(inside.hidden_target, Some(1));
        let picture = v.playback.picture(300, 1.);
        let labels = v.labels(&picture, &inside, [1280, 960]);
        assert!(labels.iter().all(|l| !l.text.contains("1-1")));
        // Outside, the selected aircraft's label is bracketed and coloured
        // for its side.
        press(&mut v, "F10");
        let outside = camera(&mut v, 300);
        let labels = v.labels(&picture, &outside, [1280, 960]);
        let own = labels.iter().find(|l| l.text == "[Enemy 1-1]").unwrap();
        assert_eq!(own.color, trails::side_color(Side::Enemy));
    }

    /// The panels' view of the recording at `now`.
    fn data(v: &mut Viewer, now: u64) -> ReplayData<'_> {
        ReplayData {
            recording: &v.recording,
            trees: &mut v.trees,
            info: &v.info,
            missiles: &v.missiles,
            comms: &v.comms,
            now,
            player: v.player,
        }
    }

    /// The Comms panel's rows at `now`, unfiltered, as time, kind and words.
    fn comms_rows(v: &mut Viewer, now: u64) -> Vec<String> {
        let data = data(v, now);
        let (events, end) = data.comms();
        let rows = panels::rows_before(events, end, &panels::CommsPanel::default(), 20);
        rows.iter()
            .map(|row| {
                let e = panels::entry(events, row, &|id| data.name(id));
                let outcome = e
                    .outcome
                    .map(|(o, _)| format!(" [{o}]"))
                    .unwrap_or_default();
                format!("{} {} {}{outcome}", e.time, e.kind, e.main)
            })
            .collect()
    }

    #[test]
    fn cockpit_messages_are_rebuilt_as_flight_keeps_them() {
        use vocab::{field, outcome};
        let hud = |tick: u64, text: &str, result: &str| TimedEvent {
            tick,
            event: Event::new(vocab::kind::COMMS_HUD)
                .with(field::OUTCOME, result)
                .with_text(text),
        };
        let mut entries: Vec<TimedEvent> = (0..9)
            .map(|n| hud(n, &format!("Message {n}"), outcome::DELIVERED))
            .collect();
        // The seventh line newer than it pushed Message 1 off; that entry
        // and one the comms journal kept for an AI line show nothing more.
        entries.push(hud(9, "Message 1", outcome::REPLACED));
        entries.push(TimedEvent {
            tick: 9,
            event: Event::new(vocab::kind::COMMS_HUD)
                .with(field::SOURCE, vocab::source::HUD)
                .with_text("Attacking"),
        });
        // A repeat moves to the bottom.
        entries.push(hud(10, "Message 4", outcome::DELIVERED));
        assert_eq!(
            message_lines(&entries),
            [2, 3, 5, 6, 7, 8, 4].map(|n| format!("Message {n}"))
        );
        assert!(message_lines(&[]).is_empty());
        // Radio, crew and tower lines stay subtitles; messages left them.
        let dir = TempDir::new("viewer-messages");
        let v = viewer(&dir, &Options::default());
        assert!(v.messages(f::LAST).is_empty());
    }

    #[test]
    fn subtitles_and_the_comms_panel_follow_the_playhead() {
        let dir = TempDir::new("viewer-comms");
        let mut v = viewer(&dir, &Options::default());
        assert_eq!(v.subtitles(f::HEARD + 20), ["Enemy 1-1: Fox two"]);
        // The unheard call makes no subtitle; the heard one lasts four seconds.
        assert_eq!(v.subtitles(f::UNHEARD + 5), ["Enemy 1-1: Fox two"]);
        assert!(v.subtitles(f::HEARD + SUBTITLE_TICKS).is_empty());
        assert!(v.subtitles(f::HEARD - 1).is_empty());
        assert_eq!(
            comms_rows(&mut v, f::LAST),
            [
                "00:00.4 ORDER You: Engage my target",
                "00:01.1 RADIO Enemy 1-1: Fox two [heard by you]",
                "00:01.2 RADIO Enemy 1-2: Contact [not heard]",
            ]
        );
        // Playing backwards shows exactly what playing forwards showed.
        assert_eq!(comms_rows(&mut v, f::HEARD).len(), 2);
        assert!(comms_rows(&mut v, f::ORDER - 1).is_empty());
        let model = v.model(f::LAUNCH);
        assert_eq!(model.active, Some(Control::Play));
        assert_eq!(model.markers.len(), 4);
        assert_eq!(model.aircraft, "You F/A-18D");
        assert_eq!(model.camera, "F10 External");
        // C opens the Comms panel, which takes the subtitles' place.
        press(&mut v, "c");
        assert!(v.ui.comms && v.panels.comms_open());
        assert!(v.model(f::HEARD + 20).subtitles.is_empty());
        press(&mut v, "c");
        assert!(!v.ui.comms && !v.panels.comms_open());
        assert_eq!(v.model(f::HEARD + 20).subtitles.len(), 1);
    }

    #[test]
    fn panels_read_the_trees_at_the_playhead_in_either_direction() {
        let dir = TempDir::new("viewer-trees");
        let mut v = viewer(&dir, &Options::default());
        let channel = vocab::channel::AI_THOUGHT;
        for tick in [200, 900, 199, 181, 179, f::GAP.0 + 20, 30] {
            let (at, tree) = data(&mut v, tick).tree(1, channel).unwrap();
            // Inside the gap the last sample before it answers.
            let last = if (f::GAP.0..=f::GAP.1).contains(&tick) {
                f::GAP.0 - 1
            } else {
                tick
            };
            let expected = last / f::THOUGHT_EVERY * f::THOUGHT_EVERY;
            assert_eq!(at, expected, "{tick}");
            assert_eq!(tree.nodes, f::thought(expected));
        }
        assert!(data(&mut v, f::FIRST).tree(1, channel).is_none());
        let d = data(&mut v, 300);
        assert_eq!(d.title(Kind::Thought, 1), "Enemy 1-1  MiG-29");
        assert_eq!(
            d.title(Kind::Guidance, f::MISSILE),
            "Missile from Enemy 1-1"
        );
        assert!(d.missing(Kind::Thought, 0).contains("flown by a person"));
        assert!(
            d.missing(Kind::Thought, 2)
                .starts_with("No AI thinking recorded")
        );
    }

    /// A camera 3,000 feet south of and level with aircraft `id` at `tick`,
    /// looking at it.
    fn camera_on(id: u32, tick: u64) -> Camera {
        let at = f::position(id, tick);
        Drone::looking(Mode::Free, [at[0], at[1], at[2] - 3_000.], at, None).camera(None, |_, _| 0.)
    }

    /// The last frame showed `tick` through `camera` on a view of `size`.
    fn shown(v: &mut Viewer, tick: u64, camera: Camera, size: [u32; 2]) {
        v.clock.seek(tick as f64);
        let picture = v.playback.picture(tick, 1.);
        let ground = v.ground_objects(tick, &picture);
        v.pickables = Viewer::pickables_for(&picture, &[], &ground);
        v.camera = camera;
        v.size = size;
    }

    fn right_click(v: &mut Viewer, at: [f64; 2], size: [u32; 2]) {
        v.right(true, at, Some(at), size, 4.);
        v.right(false, [at[0] + 2., at[1] - 1.], Some(at), size, 4.);
    }

    #[test]
    fn right_clicking_the_camera_button_chooses_views_directly() {
        let dir = TempDir::new("viewer-camera-menu");
        for size in [[1280, 960], [1920, 1080], [800, 1200]] {
            let mut v = viewer(&dir, &Options::default());
            let (x, y, w, h) = overlay::rect(Control::Camera);
            let scale = Placement::new(size).scale();
            let at = [
                (f64::from(size[0]) - 640. * scale) / 2.
                    + (f64::from(x) + f64::from(w) / 2.) * scale,
                f64::from(size[1]) + (f64::from(y) + f64::from(h) / 2. - 480.) * scale,
            ];
            right_click(&mut v, at, size);
            v.refresh_object_menu();
            let menu = v.menu.as_mut().unwrap();
            assert!(menu.camera_choices);
            assert_eq!(menu.title, "Choose view");
            assert_eq!(menu.items.len(), VIEW_ORDER.len() + 6);
            menu.focus = menu
                .items
                .iter()
                .position(|item| item.action == Some(Action::Camera(flight_views::FLY_BY)))
                .unwrap();
            press(&mut v, "Enter");
            assert!(v.menu.is_none() && v.drone.is_none());
            assert_eq!(v.view, flight_views::FLY_BY);
            right_click(&mut v, at, size);
            v.refresh_object_menu();
            let menu = v.menu.as_mut().unwrap();
            menu.focus = menu
                .items
                .iter()
                .position(|item| item.action == Some(Action::DroneMode(Mode::Free)))
                .unwrap();
            press(&mut v, "Enter");
            assert_eq!(v.drone.as_ref().unwrap().mode, Mode::Free);
            // Scene clicks still open the object menu.
            right_click(&mut v, [10., 10.], size);
            assert!(!v.menu.as_ref().unwrap().camera_choices);
            v.ui.hidden = true;
            v.menu = None;
            right_click(&mut v, at, size);
            assert!(v.menu.is_none());
        }
    }

    #[test]
    fn right_clicks_pick_what_is_drawn_and_the_menu_acts_on_it() {
        let dir = TempDir::new("viewer-menu");
        let mut v = viewer(&dir, &Options::default());
        v.clock.pause();
        for size in [[1280, 960], [1920, 1080], [800, 1200]] {
            shown(&mut v, 300, camera_on(1, 300), size);
            let middle = [f64::from(size[0]) / 2., f64::from(size[1]) / 2.];
            right_click(&mut v, [middle[0] + 4., middle[1] - 3.], size);
            let menu = v.menu.take().unwrap();
            assert_eq!(menu.target, Target::Aircraft(1), "{size:?}");
            assert_eq!(menu.title, "Enemy 1-1  MiG-29");
            // The menu opens beside the click in the centred panel layer.
            let layer = Placement::new(size).centered(middle);
            let (x, y, _, _) = menu.rect();
            assert!((f64::from(x) - layer.0).abs() < 12. && (f64::from(y) - layer.1).abs() < 12.);
        }
        let size = [1280, 960];
        shown(&mut v, 300, camera_on(1, 300), size);
        // A drag opens nothing.
        v.right(true, [640., 480.], Some([640., 480.]), size, 4.);
        v.pointer(Some([700., 480.]), [700., 480.], size, [0.; 2]);
        v.right(false, [700., 480.], Some([640., 480.]), size, 4.);
        assert!(v.menu.is_none());
        // Keys move through the menu and choose: AI thinking, after the
        // HUD camera.
        right_click(&mut v, [640., 480.], size);
        for _ in 0..6 {
            assert_eq!(press(&mut v, "ArrowDown"), Command::None);
        }
        assert_eq!(press(&mut v, "Enter"), Command::None);
        assert!(v.menu.is_none());
        let panel = v.panels.slot(panels::Side::Left).unwrap();
        assert_eq!((panel.kind, panel.subject), (Kind::Thought, 1));
        // Esc closes the menu, not the viewer.
        right_click(&mut v, [640., 480.], size);
        assert_eq!(press(&mut v, "Escape"), Command::None);
        assert!(v.menu.is_none());
        // Follow, cockpit and drone switch the camera to the aircraft.
        for (action, check) in [
            (Action::Follow(1), (1, EXTERNAL, false)),
            (Action::Cockpit(2), (2, 0, false)),
            (Action::Drone(1), (1, 0, true)),
        ] {
            v.perform(action);
            assert_eq!((v.selected, v.view, v.drone.is_some()), check, "{action:?}");
        }
        // Empty space lists the aircraft recorded now; a pick jumps to one.
        v.drone = None;
        right_click(&mut v, [20., 20.], size);
        let menu = v.menu.as_ref().unwrap();
        assert_eq!(menu.target, Target::Nothing);
        let jumps: Vec<_> = menu.items.iter().filter_map(|i| i.action).collect();
        assert!(jumps.contains(&Action::Jump(3)) && jumps.contains(&Action::Jump(0)));
        press(&mut v, "Home");
        press(&mut v, "Enter");
        assert_eq!(v.selected, 0);
        // A left click outside the menu closes it and does nothing else.
        right_click(&mut v, [20., 20.], size);
        let paused = v.clock.paused();
        let play = overlay::rect(Control::Play);
        let on_play = Some([f64::from(play.0 * 2 + 4), f64::from(play.1 * 2 + 4)]);
        v.left(true, on_play, size);
        v.left(false, on_play, size);
        assert!(v.menu.is_none());
        assert_eq!(v.clock.paused(), paused);
        // A missile in flight: its guidance.
        let tick = f::LAUNCH + 40;
        let missile = f::position(1, f::LAUNCH);
        let eye = [missile[0] + 400., missile[1] + 100., missile[2] - 3_000.];
        let at = v.playback.picture(tick, 1.).projectiles[0].position;
        let camera = Drone::looking(Mode::Free, eye, at, None).camera(None, |_, _| 0.);
        shown(&mut v, tick, camera, size);
        right_click(&mut v, [640., 480.], size);
        let menu = v.menu.as_ref().unwrap();
        assert_eq!(menu.target, Target::Missile(f::MISSILE));
        assert_eq!(menu.title, "Missile from Enemy 1-1");
        press(&mut v, "Enter");
        assert!(v.panels.find(Kind::Guidance, f::MISSILE).is_some());
        // Hidden, a right-click opens nothing.
        press(&mut v, "h");
        right_click(&mut v, [640., 480.], size);
        assert!(v.menu.is_none());
    }

    /// The camera the viewer chooses at `tick`, as a frame would show it.
    fn frame_at(v: &mut Viewer, tick: u64) -> Camera {
        v.clock.seek(tick as f64);
        let picture = v.playback.picture(tick, 1.);
        let camera = v.frame_camera(&picture, tick, 0., false);
        v.camera = copy(&camera);
        v.shown = true;
        camera
    }

    fn toast(v: &Viewer) -> String {
        v.toast.as_ref().map(|(t, _)| t.clone()).unwrap_or_default()
    }

    #[test]
    fn replay_hud_camera_uses_local_ink_but_not_instrument_or_flood_levels() {
        let dir = TempDir::new("viewer-local-lighting");
        let mut v = viewer(&dir, &Options::default());
        // The general viewer fixture carries no HUD font. Supply synthetic ink.
        for model in std::iter::once(&mut v.ownship).chain(v.models.iter_mut()) {
            model.hud_font = tore_formats::font::Font {
                height: 7,
                glyphs: (0..256)
                    .map(|_| tore_formats::font::Glyph {
                        advance: 6,
                        pixels: vec![(0, 0)],
                    })
                    .collect(),
            };
        }
        press(&mut v, "v");
        let tick = v.recording.first_tick().unwrap();
        let picture = v.playback.picture(tick, 1.);
        let draw = |v: &mut Viewer| {
            v.taped_view(&picture, tick, 0., [640, 480])
                .unwrap()
                .overlay
        };
        let green = draw(&mut v);
        v.local_lighting.0.colour = 3;
        let cyan = draw(&mut v);
        assert!(green != cyan, "HUD camera must use the local colour");
        v.local_lighting.0.instruments = 1;
        v.local_lighting.0.flood = 0;
        assert!(
            cyan == draw(&mut v),
            "instrument/flood levels must leave HUD ink unchanged"
        );
        v.local_lighting.1 = -256;
        let dim = draw(&mut v);
        v.local_lighting.1 = 256;
        assert!(
            dim != draw(&mut v),
            "HUD camera must use the local brightness"
        );
    }

    #[test]
    fn v_opens_the_hud_camera_and_shift_v_needs_a_sight() {
        let dir = TempDir::new("viewer-taped");
        let mut v = viewer(&dir, &Options::default());
        let shifted = |v: &mut Viewer, key: &str| {
            v.key(key, false, false, true);
            v.key(key, true, false, true)
        };
        // The fixture's aircraft have no sight: the Sensor View says so and
        // the view stays.
        let before = v.view;
        shifted(&mut v, "v");
        assert_eq!(v.view, before);
        assert!(toast(&v).contains("no targeting sight"), "{}", toast(&v));
        press(&mut v, "v");
        assert_eq!(v.view, tape_views::HUD_CAMERA);
        // Digits are the Sensor View's pictures only inside it.
        press(&mut v, "2");
        assert_eq!(v.sensor_mode, crate::sensor_console::SensorMode::WhiteHot);
        // Give the selected aircraft's type a sight: Shift+F7 opens the
        // Sensor View in TV, and 4, 1 and Backslash change the picture.
        let kind = v.kinds[&v.selected];
        v.sights
            .push((kind, Some(("TEST POD".into(), tape_views::pave_spike()))));
        shifted(&mut v, "F7");
        assert_eq!(v.view, tape_views::SENSOR);
        assert_eq!(v.sensor_mode, crate::sensor_console::SensorMode::Tv);
        press(&mut v, "4");
        assert_eq!(
            v.sensor_mode,
            crate::sensor_console::SensorMode::NightVision
        );
        press(&mut v, "\\");
        assert_eq!(v.sensor_mode, crate::sensor_console::SensorMode::WhiteHot);
        // An F-key leaves to the flight view.
        press(&mut v, "F1");
        assert_eq!(v.view, 0);
    }

    fn forward(c: &Camera) -> [f64; 3] {
        Basis::new(f64::from(c.yaw), f64::from(c.pitch), -f64::from(c.roll)).forward
    }

    fn toward(from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
        let d: [f64; 3] = std::array::from_fn(|i| to[i] - from[i]);
        let length = feet(from, to);
        d.map(|x| x / length)
    }

    #[test]
    fn views_match_flight_wingmen_back_view_modifiers_and_look_keys() {
        let dir = TempDir::new("viewer-flight-keys");
        let mut v = viewer(&dir, &Options::default());
        v.clock.pause();
        press(&mut v, "Tab");
        assert_eq!(v.selected, 1);
        // F6 names the wingman; again, the next one, wrapping round.
        press(&mut v, "F6");
        frame_at(&mut v, 300);
        assert_eq!(toast(&v), "Wingman view: Enemy 1-2");
        for expected in ["Enemy 1-3", "Enemy 1-2"] {
            press(&mut v, "F6");
            frame_at(&mut v, 300);
            assert_eq!(toast(&v), format!("Wingman view: {expected}"));
        }
        // F2 looks from the pilot's eye over the airframe, which shows, but
        // its own label does not.
        press(&mut v, "F2");
        let back = frame_at(&mut v, 300);
        let picture = v.playback.picture(300, 1.);
        let pose = picture.target(1).unwrap();
        let body = Body::posed(pose, 1.);
        let eye = crate::mirrors::pilot_eye(pose.position, body.basis());
        assert!(feet(back.position, eye) < 1e-6);
        assert_eq!((back.hidden_target, back.hidden_projectile), (None, None));
        let labels = v.labels(&picture, &back, [1280, 960]);
        assert!(labels.iter().all(|l| l.id != 1));
        // F1 still hides it.
        press(&mut v, "F1");
        assert_eq!(frame_at(&mut v, 300).hidden_target, Some(1));
        // Alt: from its target, the player. Ctrl: from its newest missile.
        assert!(v.view_key("F1", true, false));
        assert_eq!(v.rig.reference, Reference::Target);
        let c = frame_at(&mut v, 300);
        let player = v.playback.picture(300, 1.).player.position;
        assert_eq!((c.position, c.hidden_target), (player, Some(0)));
        assert!(v.view_key("F1", false, true));
        let c = frame_at(&mut v, f::LAUNCH + 20);
        assert_eq!(c.hidden_projectile, Some(f::MISSILE));
        assert_eq!(v.camera_error, None);
        // Once it has hit, the view says so and shows the aircraft outside.
        frame_at(&mut v, f::IMPACT + 20);
        assert_eq!(
            v.camera_error,
            Some("No last-launched missile for this view")
        );
        // Unmodified, the aircraft itself again; Ctrl+Alt and other keys are
        // not views.
        press(&mut v, "F1");
        assert_eq!(v.rig.reference, Reference::Aircraft(1));
        assert!(!v.view_key("F1", true, true));
        assert!(!v.view_key("x", true, false));
        // The Escape menu takes Alt and Ctrl view keys too.
        press(&mut v, "Escape");
        assert!(!v.view_key("F7", true, false));
        assert_eq!((v.view, v.rig.reference), (0, Reference::Aircraft(1)));
        press(&mut v, "Escape");
        // Recentering keeps the view and zoom; = and - zoom as in flight.
        v.look = [0.4, 0.2];
        press(&mut v, "=");
        assert!((v.zoom - 1.1).abs() < 1e-6);
        press(&mut v, "Numpad5");
        assert_eq!((v.look, v.view), ([0.; 2], 0));
        assert!((v.zoom - 1.1).abs() < 1e-6);
        v.look = [0.4, 0.2];
        press(&mut v, "/");
        assert_eq!(v.look, [0.4, 0.2]);
        v.key("/", true, false, true);
        assert_eq!(v.look, [0.; 2]);
        for _ in 0..30 {
            v.key("=", true, true, false);
        }
        assert_eq!(v.zoom, 4.);
        for _ in 0..30 {
            press(&mut v, "-");
        }
        assert_eq!(v.zoom, 0.5);
    }

    #[test]
    fn f7_carries_the_bearing_compass_from_the_selected_aircraft() {
        let dir = TempDir::new("viewer-compass");
        let mut v = viewer(&dir, &Options::default());
        v.clock.pause();
        press(&mut v, "F7");
        // No target yet: no compass.
        frame_at(&mut v, 50);
        assert_eq!(v.compass, None);
        frame_at(&mut v, 200);
        let picture = v.playback.picture(200, 1.);
        let target = picture.target(2).unwrap().position;
        let expected = crate::view_compass::bearing(picture.player.position, target);
        assert!((v.compass.unwrap() - expected).abs() < 1e-9);
        // Only from the aircraft itself, and only in F7.
        v.view_key("F7", true, false);
        frame_at(&mut v, 200);
        assert_eq!(v.compass, None);
        press(&mut v, "F10");
        frame_at(&mut v, 200);
        assert_eq!(v.compass, None);
    }

    #[test]
    fn fly_by_goes_ahead_of_the_motion_either_way() {
        let dir = TempDir::new("viewer-fly-by");
        let mut v = viewer(&dir, &Options::default());
        let tick = 300;
        let picture = v.playback.picture(tick, 1.);
        let pose = picture.player.clone();
        let right = Body::posed(&pose, 1.).basis().right;
        let point = |sign: f64| -> [f64; 3] {
            std::array::from_fn(|i| {
                pose.position[i]
                    + pose.velocity[i] * 3. * sign
                    + right[i] * 300.
                    + if i == 1 { 100. } else { 0. }
            })
        };
        press(&mut v, "F9");
        v.clock.pause();
        assert!(feet(frame_at(&mut v, tick).position, point(1.)) < 1e-6);
        press(&mut v, "j");
        v.clock.pause();
        press(&mut v, "F9");
        assert!(feet(frame_at(&mut v, tick).position, point(-1.)) < 1e-6);
        // Paused, the point stays put.
        assert!(feet(frame_at(&mut v, tick).position, point(-1.)) < 1e-6);
    }

    fn site(id: u32, name: &str, center: [f64; 3]) -> tore_sim::airport::StaticObject {
        use tore_sim::airport::{OrientedBox, SourceKey, StaticObject};
        StaticObject {
            id,
            source: SourceKey {
                layout: "test".into(),
                ordinal: id,
            },
            name: name.into(),
            object_type: "Bunker".into(),
            bounds: OrientedBox {
                center,
                half: [20.; 3],
                heading: 0.,
                pitch: 0.,
                bank: 0.,
            },
            hit_points: 100,
            category: 0,
            radar_signature: 1.,
            infrared_signature: 1.,
            type_flags: 0,
            max_visible: 0,
            runway: false,
        }
    }

    /// A viewer with two ground objects: the fixture's building, destroyed
    /// at its second hit, and a site 50 nmi east of the player at `FAR_AT`.
    fn with_ground(name: &str) -> (TempDir, Viewer) {
        let dir = TempDir::new(name);
        let mut v = viewer(&dir, &Options::default());
        let far = far_site();
        v.world.airport_scene.objects = vec![
            site(f::SURFACE[0].1, "Hangar", [101_000., 0., 99_000.]),
            site(9_050, "", far),
        ];
        v.finish_tracks();
        v.clock.pause();
        (dir, v)
    }

    const FAR_AT: u64 = 200;
    fn far_site() -> [f64; 3] {
        let at = f::position(0, FAR_AT);
        [at[0] + 50. * NMI, at[1], at[2]]
    }

    #[test]
    fn cycling_missile_view_keeps_a_ground_target_centered() {
        let (_dir, mut v) = with_ground("viewer-missile-ground-target");
        v.clock.seek(FAR_AT as f64);
        press(&mut v, "F12");
        let mut picture = v.playback.picture(FAR_AT, 1.);
        let missile = picture
            .projectiles
            .iter_mut()
            .find(|p| p.id == f::MISSILE)
            .unwrap();
        missile.target = Some(9_050);
        // The target exists in the recorded world, not in aircraft snapshots.
        assert!(picture.target(9_050).is_none());
        let camera = v.frame_camera(&picture, FAR_AT, 0., false);
        let [x, y] = camera
            .project([960, 720], far_site())
            .expect("ground target in view");
        assert!((x - 480.).abs() < 0.01 && (y - 360.).abs() < 0.01);
        assert_eq!(v.selected_missile, Some(f::MISSILE));
    }

    #[test]
    fn o_steps_through_aircraft_ground_objects_and_weapons_in_flight() {
        let (_dir, mut v) = with_ground("viewer-object-cycle");
        v.clock.seek(FAR_AT as f64);
        let (a, g, m) = (Target::Aircraft, Target::Ground, Target::Missile);
        let order = [a(1), a(2), m(f::MISSILE), g(9_001), g(9_050)];
        assert_eq!(v.objects_now(), order);
        for expected in order.iter().chain(&order[..1]) {
            press(&mut v, "o");
            assert_eq!((v.look_at, v.view), (Some(*expected), OBJECT));
        }
        v.key("o", true, false, true);
        assert_eq!(v.look_at, Some(g(9_050)));
        v.key("o", true, false, true);
        v.key("o", true, false, true);
        assert_eq!(v.look_at, Some(m(f::MISSILE)));
        // The launch event names no weapon, so it is a missile.
        assert_eq!(v.camera_label(), "Object: You > Missile from Enemy 1-1");
        frame_at(&mut v, FAR_AT);
        assert!(
            v.readout
                .as_ref()
                .unwrap()
                .starts_with("You > Missile from Enemy 1-1   ")
        );
        // The building has fallen and the late aircraft has arrived; the
        // missile has hit. The object it starts from never appears.
        v.clock.seek(f::IMPACT as f64 + 20.);
        assert_eq!(v.objects_now(), [a(1), a(2), a(f::LATE), g(9_050)]);
        press(&mut v, "Tab");
        assert!(!v.objects_now().contains(&a(1)) && v.objects_now().contains(&a(0)));
        // A ground object is named, or else numbered.
        assert_eq!(v.object_name(g(9_001)), "Hangar");
        assert_eq!(v.object_name(g(9_050)), "Bunker");
        v.world.airport_scene.objects[1].object_type.clear();
        assert_eq!(v.object_name(g(9_050)), "Ground object 9050");
    }

    #[test]
    fn the_object_view_follows_a_surface_unit_that_drives_and_names_it() {
        use tore_world::surface::{SurfacePose, UnitId};
        let (_dir, mut v) = with_ground("viewer-object-column");
        v.clock.seek(FAR_AT as f64);
        // The column's unit started at the far site and has driven 3 nmi on.
        let drove = [
            far_site()[0] + 3. * NMI,
            far_site()[1],
            far_site()[2] + 500.,
        ];
        let mut picture = v.playback.picture(FAR_AT, 1.);
        picture.surface = vec![SurfacePose {
            id: UnitId(9_050),
            position: drove,
            attitude: [1.0, 0.0, 0.0],
            shape: None,
            wrecked: false,
        }];
        // Listed, looked at and picked where it is, not where it began.
        let at = |objects: Vec<(u32, [f64; 3])>| objects.iter().find(|o| o.0 == 9_050).unwrap().1;
        assert_eq!(at(v.ground_objects(FAR_AT, &picture)), drove);
        let plain = v.playback.picture(FAR_AT, 1.);
        assert_eq!(
            at(v.ground_objects(FAR_AT, &plain)),
            far_site(),
            "a unit with no recorded pose stands where the scene put it"
        );
        let body = v.body_of(&picture, FAR_AT, Target::Ground(9_050)).unwrap();
        assert_eq!(body.position(), drove);
        // The recording's registry names it; failing that, the scene does.
        assert_eq!(v.object_name(Target::Ground(9_050)), "Bunker");
        v.world.airport_scene.objects[1].name = "T-72".into();
        assert_eq!(v.object_name(Target::Ground(9_050)), "T-72");
    }

    #[test]
    fn launcher_rails_come_from_the_recorded_changes() {
        use tore_replay::SurfaceStock;
        let (_dir, mut v) = with_ground("viewer-rails");
        let rail = |mount, loaded| SurfaceStock {
            unit: 9_050,
            mount,
            loaded,
            reserve: None,
        };
        v.tracks.stock = vec![(150, rail(0, 1)), (150, rail(1, 3)), (190, rail(0, 3))];
        let loads = |tick| -> Vec<u32> {
            v.rails(tick)
                .unit(tore_world::surface::UnitId(9_050))
                .map(|unit| unit.mounts.iter().map(|m| m.loaded).collect())
                .unwrap_or_default()
        };
        assert!(loads(149).is_empty(), "as new until a rail changes");
        assert_eq!(loads(150), [1, 3]);
        assert_eq!(loads(200), [3, 3]);
        assert!(
            v.rails(200)
                .unit(tore_world::surface::UnitId(9_050))
                .unwrap()
                .armed
        );
    }

    #[test]
    fn object_view_faces_what_it_looks_at_at_any_range() {
        let (_dir, mut v) = with_ground("viewer-object-far");
        v.perform(Action::LookAt(Target::Ground(9_050)));
        assert_eq!(v.view, OBJECT);
        let c = frame_at(&mut v, FAR_AT);
        let far = far_site();
        assert!(tore_sim::attitude::dot(forward(&c), toward(c.position, far)) > 0.99999);
        assert!(tore_sim::attitude::dot(forward(&c), toward(f::position(0, FAR_AT), far)) > 0.9999);
        assert_eq!(v.readout.as_deref(), Some("You > Bunker   50.0 nmi"));
        // Looking at the object it starts from is refused.
        v.perform(Action::LookAt(Target::Aircraft(0)));
        assert_eq!(v.look_at, Some(Target::Ground(9_050)));
        assert!(toast(&v).contains("already starts from there"));
        // From the ground object: it was the look-at, so the view turns to
        // the nearest aircraft; then back at the player.
        v.perform(Action::ViewFrom(Target::Ground(9_050)));
        assert_eq!(v.from(), Target::Ground(9_050));
        assert_eq!(v.look_at, Some(Target::Aircraft(2)));
        v.perform(Action::LookAt(Target::Aircraft(0)));
        let c = frame_at(&mut v, FAR_AT);
        let at = v.playback.picture(FAR_AT, 1.).player.position;
        assert!(tore_sim::attitude::dot(forward(&c), toward(c.position, at)) > 0.99999);
        assert!(feet(c.position, far) < 250.);
        // The camera button reaches it after F12, then the drones.
        press(&mut v, "F12");
        v.left_owner = None;
        v.next_camera();
        assert_eq!((v.view, v.drone.is_none()), (OBJECT, true));
        v.next_camera();
        assert!(v.drone.is_some());
        // Tab selects an aircraft, which the view then starts from.
        press(&mut v, "Tab");
        assert_eq!(v.from(), Target::Aircraft(1));
    }

    #[test]
    fn object_view_falls_back_while_an_end_is_missing_and_recovers() {
        let (_dir, mut v) = with_ground("viewer-object-missing");
        // Nothing chosen yet.
        v.set_view(OBJECT);
        frame_at(&mut v, FAR_AT);
        assert!(toast(&v).starts_with("Nothing to look at"));
        v.perform(Action::LookAt(Target::Missile(f::MISSILE)));
        let good = frame_at(&mut v, FAR_AT);
        assert_eq!(v.camera_error, None);
        // The missile has hit: the player from outside, and the view keeps
        // waiting for it rather than moving on.
        v.toast = None;
        let outside = frame_at(&mut v, f::IMPACT + 20);
        assert_eq!(
            v.camera_error,
            Some("The object to look at is not in the recording now")
        );
        assert!(!toast(&v).is_empty());
        let offset = feet(outside.position, f::position(0, f::IMPACT + 20));
        assert!(offset > 100. && offset < 250., "{offset}");
        assert_eq!(v.look_at, Some(Target::Missile(f::MISSILE)));
        // Said once.
        v.toast = None;
        frame_at(&mut v, f::IMPACT + 30);
        assert!(v.toast.is_none());
        // Back before the hit, it recovers.
        let again = frame_at(&mut v, FAR_AT);
        assert_eq!((v.camera_error, again.position), (None, good.position));
        // From the missile: it looks at the missile's target, the player,
        // from 30 feet behind; once the missile is gone the last camera stays.
        v.perform(Action::ViewFrom(Target::Missile(f::MISSILE)));
        assert_eq!(v.look_at, Some(Target::Aircraft(0)));
        let from_missile = frame_at(&mut v, FAR_AT);
        let picture = v.playback.picture(FAR_AT, 1.);
        let missile = picture
            .projectiles
            .iter()
            .find(|p| p.id == f::MISSILE)
            .unwrap();
        let behind = feet(from_missile.position, missile.position);
        assert!((20. ..45.).contains(&behind), "{behind}");
        let player = picture.player.position;
        assert!(
            tore_sim::attitude::dot(
                forward(&from_missile),
                toward(from_missile.position, player)
            ) > 0.99999
        );
        let kept = frame_at(&mut v, f::IMPACT + 20);
        assert_eq!(kept.position, from_missile.position);
        assert_eq!(
            v.camera_error,
            Some("The object the view starts from is not in the recording now")
        );
        // A destroyed ground object is missing too.
        v.perform(Action::ViewFrom(Target::Ground(9_001)));
        frame_at(&mut v, 310);
        assert_eq!(v.camera_error, None);
        frame_at(&mut v, f::SURFACE[1].0 + 1);
        assert!(v.camera_error.is_some());
    }

    #[test]
    fn right_click_on_a_ground_object_offers_the_object_view() {
        let (_dir, mut v) = with_ground("viewer-object-menu");
        let size = [1280, 960];
        let hangar = [101_000., 0., 99_000.];
        let camera = Drone::looking(
            Mode::Free,
            [hangar[0], 3_000., hangar[2] - 6_000.],
            hangar,
            None,
        )
        .camera(None, |_, _| 0.);
        shown(&mut v, FAR_AT, camera, size);
        right_click(&mut v, [640., 480.], size);
        let menu = v.menu.take().unwrap();
        assert_eq!(menu.target, Target::Ground(9_001));
        assert_eq!(menu.title, "Hangar");
        let actions: Vec<_> = menu.items.iter().filter_map(|i| i.action).collect();
        assert_eq!(
            actions[..2],
            [
                Action::ViewFrom(Target::Ground(9_001)),
                Action::LookAt(Target::Ground(9_001))
            ]
        );
        // Once it has fallen it is not there to pick.
        let same = copy(&v.camera);
        shown(&mut v, 400, same, size);
        right_click(&mut v, [640., 480.], size);
        assert_eq!(v.menu.take().unwrap().target, Target::Nothing);
    }

    #[test]
    fn panel_keys_open_follow_and_close() {
        let dir = TempDir::new("viewer-panel-keys");
        let mut v = viewer(&dir, &Options::default());
        v.clock.pause();
        v.clock.seek((f::LAUNCH + 40) as f64);
        press(&mut v, "i");
        press(&mut v, "f");
        let kinds = |v: &Viewer| {
            [panels::Side::Left, panels::Side::Right]
                .map(|side| v.panels.slot(side).map(|p| (p.kind, p.subject)))
        };
        assert_eq!(
            kinds(&v),
            [Some((Kind::Thought, 0)), Some((Kind::Telemetry, 0))]
        );
        // Unpinned panels follow the selected aircraft.
        press(&mut v, "Tab");
        assert_eq!(
            kinds(&v),
            [Some((Kind::Thought, 1)), Some((Kind::Telemetry, 1))]
        );
        // Pressing the key again closes that panel.
        press(&mut v, "f");
        assert_eq!(kinds(&v)[1], None);
        // G: the selected aircraft's missile in flight, or why not.
        press(&mut v, "g");
        assert_eq!(kinds(&v)[1], Some((Kind::Guidance, f::MISSILE)));
        press(&mut v, "Tab");
        press(&mut v, "g");
        assert!(
            v.toast
                .as_ref()
                .unwrap()
                .0
                .contains("has no missile in flight")
        );
        // The wheel over a panel scrolls it rather than zooming.
        let size = [1280, 960];
        v.pointer(Some([100., 400.]), [100., 400.], size, [0.; 2]);
        let zoom = v.zoom;
        v.wheel(-1);
        assert_eq!(v.zoom, zoom);
        v.pointer(Some([640., 400.]), [640., 400.], size, [0.; 2]);
        v.wheel(-1);
        assert!(v.zoom < zoom);
        // M opens the menu on the selected aircraft; X closes every panel.
        press(&mut v, "m");
        assert_eq!(v.menu.as_ref().unwrap().target, Target::Aircraft(2));
        press(&mut v, "Escape");
        press(&mut v, "c");
        press(&mut v, "x");
        assert!(v.panels.is_empty() && !v.ui.comms);
        // Clicking a panel's close button closes it.
        press(&mut v, "i");
        let placement = Placement::new(size);
        let close = [
            (6. + 254. - 9.) * placement.scale(),
            (28. + 6.) * placement.scale(),
        ];
        v.left(true, Some(close), size);
        v.left(false, Some(close), size);
        assert!(v.panels.is_empty());
    }

    #[test]
    fn hud_subtitles_are_what_the_hud_showed() {
        use vocab::{field, kind, outcome, source};
        let hud = |result: &str| Event::new(kind::COMMS_HUD).with(field::OUTCOME, result);
        assert!(hud_shown(&hud(outcome::DELIVERED)));
        assert!(hud_shown(&hud(outcome::QUEUED)));
        assert!(hud_shown(&Event::new(kind::COMMS_HUD)));
        assert!(!hud_shown(&hud(outcome::SUPPRESSED)));
        assert!(!hud_shown(&hud(outcome::DROPPED)));
        // The journal's copy of an AI text line waiting for the HUD.
        assert!(!hud_shown(
            &hud(outcome::QUEUED).with(field::SOURCE, source::HUD)
        ));
    }

    #[test]
    fn command_line_options_set_the_start() {
        let dir = TempDir::new("viewer-options");
        let options = Options {
            tick: Some(300),
            view: Some(3),
            aircraft: Some(2),
            drone: true,
            speed: Some(-4.),
            ui: Ui::parse("trails,comms").unwrap(),
            panels: Request::parse("thought, telemetry,menu").unwrap(),
            menu: None,
            look_at: None,
        };
        let mut v = viewer(&dir, &options);
        assert!(v.panels.comms_open());
        v.open_requests(&camera_on(2, 300));
        assert_eq!(v.panels.find(Kind::Thought, 2), Some(panels::Side::Left));
        assert_eq!(v.panels.find(Kind::Telemetry, 2), Some(panels::Side::Right));
        assert_eq!(v.menu.as_ref().unwrap().target, Target::Aircraft(2));
        assert!(Request::parse("thought,sparkles").is_err());
        // The marks are on unless the list leaves them out.
        assert!(!v.ui.marks);
        assert!(Ui::parse("marks").unwrap().marks);
        assert!(Ui::default().marks);
        assert_eq!(Request::parse("").unwrap(), []);
        assert_eq!(v.clock.position(), 300.);
        assert_eq!(
            (v.clock.direction(), v.clock.speed(), v.clock.paused()),
            (Direction::Reverse, 4., false)
        );
        assert_eq!((v.view, v.selected), (3, 2));
        assert_eq!(v.drone.as_ref().map(|d| d.mode), Some(Mode::Follow));
        assert!(v.ui.trails && v.ui.comms && !v.ui.labels && !v.ui.timer);
        // A tick alone starts paused there.
        let v = viewer(
            &dir,
            &Options {
                tick: Some(10),
                ..Default::default()
            },
        );
        assert!(v.clock.paused());
        assert!(Ui::parse("labels,sparkles").is_err());
        // Starting in the object view.
        assert_eq!(parse_object("weapon:7"), Ok(Target::Missile(7)));
        assert_eq!(parse_object("ground: 12"), Ok(Target::Ground(12)));
        assert_eq!(parse_object("aircraft:2"), Ok(Target::Aircraft(2)));
        for bad in ["tank:3", "weapon", "weapon:x"] {
            assert!(parse_object(bad).is_err(), "{bad}");
        }
        let v = viewer(
            &dir,
            &Options {
                look_at: Some(Target::Aircraft(2)),
                ..Default::default()
            },
        );
        assert_eq!((v.view, v.look_at), (OBJECT, Some(Target::Aircraft(2))));
        let bad = Options {
            speed: Some(3.),
            ..Default::default()
        };
        let recording = Arc::new(f::recording(dir.path(), "bad-speed"));
        let art = CombatArt::synthetic(BTreeMap::new());
        assert!(
            Viewer::assemble(
                Path::new("x"),
                recording,
                (
                    tore_world::test_support::terrain(),
                    crate::scenery::tests::scenery(),
                ),
                (
                    crate::combat_view::render_hash_tests::hornet_airframe(true),
                    Vec::new(),
                    art,
                ),
                &bad,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn screenshots_are_named_after_the_recording_and_tick() {
        let dir = crate::replay::tests::TempDir::new("viewer-shots");
        let recording = Path::new("/x/2026-09-26_1540_UKR_F18.tore-replay");
        let first = screenshot_path(dir.path(), recording, 86_808);
        assert_eq!(
            first.file_name().unwrap(),
            "2026-09-26_1540_UKR_F18-tick0086808.png"
        );
        std::fs::write(&first, b"x").unwrap();
        let second = screenshot_path(dir.path(), recording, 86_808);
        assert_eq!(
            second.file_name().unwrap(),
            "2026-09-26_1540_UKR_F18-tick0086808-2.png"
        );
        let partial = Path::new("/x/flight.tore-replay.partial");
        assert_eq!(
            screenshot_path(dir.path(), partial, 5).file_name().unwrap(),
            "flight-tick0000005.png"
        );
    }

    #[test]
    fn roll_rate_is_the_turn_about_the_nose() {
        let rate = 90f64.to_radians();
        for [yaw, pitch] in [[0., 0.], [1., 0.3], [-2., -0.5]] {
            let before = [yaw, pitch, 0.2];
            let after = [yaw, pitch, 0.2 + rate / 120.];
            assert!(
                (roll_rate(before, after) - rate).abs() < 1e-3,
                "{yaw} {pitch}"
            );
            assert!((roll_rate(after, before) + rate).abs() < 1e-3);
        }
        // A level turn is not a roll.
        assert!(roll_rate([0., 0., 0.], [0.01, 0., 0.]).abs() < 1e-9);
    }

    #[test]
    fn markers_and_targets_come_from_events() {
        let dir = crate::replay::tests::TempDir::new("viewer-events");
        let recording = crate::replay::fixture::recording(dir.path(), "events");
        use crate::replay::fixture as f;
        let marks = markers(recording.events());
        assert_eq!(
            marks,
            [
                Marker {
                    tick: f::ORDER,
                    kind: MarkerKind::Order
                },
                Marker {
                    tick: f::LAUNCH,
                    kind: MarkerKind::Launch
                },
                Marker {
                    tick: f::BOOKMARK,
                    kind: MarkerKind::Bookmark
                },
                Marker {
                    tick: f::KILL,
                    kind: MarkerKind::Kill
                },
            ]
        );
        let found = targets(recording.events(), 0);
        assert_eq!(found[&1], [(20, Some(0))]);
        assert_eq!(found[&0], [(100, Some(2))]);
        // The player's designation rides on its commands.
        let command = |tick, target: Option<u32>| TimedEvent {
            tick,
            event: {
                let event = tore_replay::Event::new(vocab::kind::PLAYER_COMMAND).with_subject(0);
                match target {
                    Some(id) => event.with_object(id),
                    None => event,
                }
            },
        };
        let player = targets(&[command(5, Some(3)), command(9, None)], 0);
        assert_eq!(player[&0], [(5, Some(3)), (9, None)]);
        // A recording with view targets follows them alone for the player,
        // sight hold and drop included; AI aircraft keep their own.
        let view = |tick, target: Option<u32>, held: bool| TimedEvent {
            tick,
            event: {
                let event = tore_replay::Event::new(vocab::kind::PLAYER_VIEW_TARGET)
                    .with_subject(0)
                    .with(vocab::field::HELD, held);
                match target {
                    Some(id) => event.with_object(id),
                    None => event,
                }
            },
        };
        let ai = TimedEvent {
            tick: 6,
            event: tore_replay::Event::new(vocab::kind::AI_TARGET)
                .with_subject(1)
                .with_object(0),
        };
        let viewed = targets(
            &[
                view(0, None, false),
                command(5, Some(3)),
                view(5, Some(3), false),
                ai,
                command(8, None),
                view(8, Some(3), true),
                view(12, None, false),
            ],
            0,
        );
        assert_eq!(
            viewed[&0],
            [(0, None), (5, Some(3)), (8, Some(3)), (12, None)]
        );
        assert_eq!(viewed[&1], [(6, Some(0))]);
    }

    /// A viewer on a recording made for the seat that flew plane 2: the
    /// plane's own pose is the picture's player, and everything that was
    /// plane 0's in single player's recordings (the first selection, the
    /// weather's player, the target the views follow, the telemetry) is
    /// plane 2's.
    #[test]
    fn a_recording_for_another_seat_follows_that_plane() {
        let dir = TempDir::new("viewer-seat");
        let recording = Arc::new(f::recording_seat(dir.path(), "seat", 2));
        let mut v = viewer_recording(Arc::clone(&recording), &Options::default());
        assert_eq!((v.player, v.selected), (2, 2));
        assert_eq!(v.label(2), "You");
        assert_ne!(v.label(0), "You");
        // Plane 2 is an enemy in the fixture, and the player is shown as
        // what its roster entry says; plane 0 is no longer the exception.
        assert_eq!(v.side(2), Side::Enemy);
        assert_eq!(v.side(0), Side::Friendly);
        let picture = v.playback.picture(300, 1.);
        assert_eq!(picture.player.id, 2);
        assert!(picture.targets.iter().all(|t| t.id != 2));
        assert!(picture.targets.iter().any(|t| t.id == 0));
        // The camera starts on the player: its anchor, and its flight state
        // (the load factor the fixture gives plane 0 only is not borrowed).
        assert_eq!(v.anchor(&picture), Some(picture.player.position));
        let state = v.player_state(&picture, 300);
        assert_eq!(state.position, picture.player.position);
        assert_eq!(state.g, 2.);
        // The object view's list and the labels count the player once.
        v.look_at = None;
        let objects = v.objects_now();
        assert!(objects.contains(&Target::Aircraft(0)));
        assert!(
            !objects.contains(&Target::Aircraft(2)),
            "it is where the view starts from"
        );
        assert_eq!(
            v.body_of(&picture, 300, Target::Aircraft(2))
                .map(|b| b.position()),
            Some(picture.player.position)
        );
        let outside = camera_on(2, 300);
        let labels = v.labels(&picture, &outside, [1280, 960]);
        assert_eq!(labels.iter().filter(|l| l.id == 2).count(), 1);
        // The weather follows the player's path, which the tracks read from
        // plane 2's frames.
        v.finish_tracks();
        let at = v.playback.aircraft(300, 2).unwrap().position;
        let view = v.tracks.view(300).unwrap().position;
        assert!((0..3).all(|i| (at[i] - view[i]).abs() < 1e-9));
        // The menus list plane 2 as a person flying, not as an AI.
        assert!(!v.menu_entry(2).ai);
        assert!(v.menu_entry(0).ai || v.info[&0].human);
    }

    /// An observer's recording has no player: no pose is the player's, no
    /// aircraft is drawn as the ownship, and the first aircraft is watched.
    #[test]
    fn an_observers_recording_has_no_player_pose_to_mistake_for_an_aircraft() {
        let dir = TempDir::new("viewer-observer");
        let mut growing = f::Growing::watching(dir.path(), "observer", convert::NO_PLAYER);
        growing.push(5, 300);
        let mut v = viewer_growing(
            growing.open(),
            &Options::default(),
            Some(Growth {
                resources: Arc::new(BTreeMap::new()),
                shapes: BTreeSet::new(),
                events: 0,
            }),
        );
        assert_eq!(v.player, convert::NO_PLAYER);
        assert_eq!(v.selected, 0, "the first aircraft");
        let tick = v.clock.tick();
        let picture = v.playback.picture(tick, 1.);
        assert_eq!(picture.player.id, convert::NO_PLAYER);
        let everyone: Vec<u32> = everyone(&picture).map(|pose| pose.id).collect();
        assert_eq!(everyone, [0, 1, 2]);
        // Labels, the object list and what a click picks name real planes.
        let outside = camera_on(1, tick);
        let labels = v.labels(&picture, &outside, [1280, 960]);
        assert!(labels.iter().all(|l| l.id != convert::NO_PLAYER));
        let objects = v.objects_now();
        assert!(
            objects
                .iter()
                .all(|t| *t != Target::Aircraft(convert::NO_PLAYER))
        );
        let picks = Viewer::pickables_for(&picture, &[], &[]);
        assert!(
            picks
                .iter()
                .all(|p| p.target != Target::Aircraft(convert::NO_PLAYER))
        );
        // The camera's subject is the aircraft watched, or where the free
        // drone is.
        assert_eq!(v.camera_subject(), Subject::Aircraft(0));
        v.drone_mode(Some(Mode::Free));
        v.shown = true;
        v.camera.position = [1_000.7, 2_000., -3_000.2];
        assert_eq!(v.camera_subject(), Subject::Point([1_000, 2_000, -3_000]));
        v.drone_mode(None);
        v.select(9);
        assert!(
            matches!(v.camera_subject(), Subject::Point(_)),
            "9 is not in the mission"
        );
    }

    /// The live view: it opens a second behind the newest frame, grows with
    /// the recording, keeps ten minutes, leaves live when scrubbed and
    /// returns to it with End; its menu stops the watch.
    #[test]
    fn the_live_view_follows_a_growing_recording() {
        let dir = TempDir::new("viewer-live");
        let mut growing = f::Growing::watching(dir.path(), "live", convert::NO_PLAYER);
        growing.push(5, 300);
        let first = growing.open();
        let mut v = viewer_growing(
            Arc::clone(&first),
            &Options::default(),
            Some(Growth {
                resources: Arc::new(BTreeMap::new()),
                shapes: BTreeSet::new(),
                events: 0,
            }),
        );
        assert!(v.live() && v.clock.following());
        let last = first.last_tick().unwrap();
        assert_eq!(v.clock.position(), last as f64 - clock::LIVE_LAG);
        assert_eq!(v.model(v.clock.tick()).speed, "LIVE");
        assert_eq!(v.title(), "T.O.R.E-Fighters - Observing");
        // The recording grows by a read of it with more frames, a new
        // aircraft and a new marker.
        growing.push(301, 700);
        growing.register(&tore_replay::AircraftInfo {
            id: 9,
            pt: "MIG29.PT".into(),
            label: "Newcomer".into(),
            ..Default::default()
        });
        growing.push(701, 760);
        let later = growing.open();
        let newest = later.last_tick().unwrap();
        v.grow(Arc::clone(&later));
        assert_eq!(v.clock.last(), newest);
        // Frames that came late and all at once: a following playhead is
        // brought back to the edge.
        assert_eq!(v.clock.position(), newest as f64 - clock::LIVE_LAG);
        assert!(v.info.contains_key(&9));
        assert_eq!(v.label(9), "Newcomer");
        assert!(v.clock.following());
        // Time passing moves the playhead on, and never past the newest.
        for _ in 0..200 {
            v.clock.advance(0.1);
        }
        assert_eq!(v.clock.position(), newest as f64);
        assert!(!v.clock.paused() && v.clock.following());
        // The same read again, or an older one, changes nothing.
        v.grow(Arc::clone(&later));
        v.grow(Arc::clone(&first));
        assert_eq!(v.clock.last(), newest);
        // Scrubbing leaves live; End comes back.
        press(&mut v, "Home");
        assert!(!v.clock.following());
        assert_eq!(v.model(v.clock.tick()).speed, "1x");
        press(&mut v, "End");
        assert!(v.clock.following());
        assert_eq!(v.clock.position(), newest as f64 - clock::LIVE_LAG);
        // Space pauses, and plays on from where it was, not from the start.
        press(&mut v, "Space");
        assert!(v.clock.paused());
        press(&mut v, "Space");
        assert!(!v.clock.paused() && v.clock.position() > 100.);
        // The Escape menu stops the watch where a replay's ends the replay.
        press(&mut v, "Escape");
        assert!(v.pause.is_open());
        let rows = &v.pause.tree[0].children;
        assert_eq!(rows[0].label, pause::STOP_WATCHING);
        assert_eq!(click(&mut v, menu_row(0)), Command::Leave);
        // A recording that is not live has no such row.
        let still = viewer(&dir, &Options::default());
        assert!(!still.live());
        assert_eq!(still.pause.tree[0].children[0].label, pause::END_REPLAY);
    }

    /// Ten minutes can be scrubbed: the oldest frames are not reachable.
    #[test]
    fn only_the_last_ten_minutes_of_a_live_recording_can_be_scrubbed() {
        let dir = TempDir::new("viewer-window");
        let mut growing = f::Growing::watching(dir.path(), "window", convert::NO_PLAYER);
        growing.push(5, 300);
        let mut v = viewer_growing(
            growing.open(),
            &Options::default(),
            Some(Growth {
                resources: Arc::new(BTreeMap::new()),
                shapes: BTreeSet::new(),
                events: 0,
            }),
        );
        assert_eq!(v.clock.first(), 5);
        // Frames far later (the fixture has none past tick 900, so the
        // window is tested on the clock the view drives).
        let far = LIVE_WINDOW_TICKS + 5_000;
        v.clock.grow(far.saturating_sub(LIVE_WINDOW_TICKS), far);
        assert_eq!(v.clock.first(), 5_000);
        v.clock.start();
        assert_eq!(v.clock.position(), 5_000.);
    }
}
