//! The input configuration screen, shared by Pref > Controls on the main menu
//! and Escape > Control in flight. Devices are listed on the left; the right
//! pane shows the selected device's settings above its action mappings,
//! grouped as in `input_catalog`. It edits a draft profile that is applied
//! only by Apply. Layout and colours are an opinionated agent design after
//! John's 2026-09-22 mockup, drawn with the imported raster font. A search
//! row above the mappings (requested by John on 2026-09-28) narrows them to
//! matching actions or to the actions an input is already bound to.
use crate::input_catalog::{self as catalog, ENTRIES, Entry, Group, Kind as Row};
use crate::{hud::Paint, menu::Canvas};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tore_formats::font::Font;
use tore_formats::text::GlyphCodes;
use tore_input::profile_text::action_name;
use tore_input::{Action, Axis, Binding, Calibration, Event, Mode, Profile};
use tore_input_native::{Device, Kind};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResultAction {
    None,
    Changed,
    Save,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Gamepad,
    Stick,
    Throttle,
    Pedals,
    HeadTracker,
    Other,
}
impl DeviceClass {
    pub fn title(self) -> &'static str {
        match self {
            DeviceClass::Gamepad => "Controller",
            DeviceClass::Stick => "Flight stick",
            DeviceClass::Throttle => "Throttle",
            DeviceClass::Pedals => "Pedals",
            DeviceClass::HeadTracker => "Head tracker",
            DeviceClass::Other => "Device",
        }
    }
}
/// Fitted device classification from the name and exposed controls; it only
/// chooses a tab title and which settings are offered.
pub fn classify(device: &Device) -> DeviceClass {
    let name = device.name.to_ascii_lowercase();
    let has = |id: &str| device.controls.iter().any(|c| c.id == id);
    if name.contains("pedal") || name.contains("rudder") {
        DeviceClass::Pedals
    } else if name.contains("headpose")
        || name.contains("opentrack")
        || name.contains("trackir")
        || name.contains("head track")
    {
        DeviceClass::HeadTracker
    } else if name.contains("throttle") {
        DeviceClass::Throttle
    } else if device.id.starts_with("macos-gc-session-")
        || (has("button:304") && has("axis:0") && has("axis:1"))
        || ["gamepad", "controller", "xbox", "dualsense", "dualshock"]
            .iter()
            .any(|n| name.contains(n))
    {
        DeviceClass::Gamepad
    } else if device.controls.iter().any(|c| matches!(c.kind, Kind::Axis)) {
        DeviceClass::Stick
    } else {
        DeviceClass::Other
    }
}

/// A standard Xbox-layout gamepad as Linux reports it, for snapshots and tests.
pub fn preview_device() -> Device {
    let axes = [
        ("axis:0", -32768., 32767.),
        ("axis:1", -32768., 32767.),
        ("axis:2", 0., 1023.),
        ("axis:3", -32768., 32767.),
        ("axis:4", -32768., 32767.),
        ("axis:5", 0., 1023.),
        ("axis:16", -1., 1.),
        ("axis:17", -1., 1.),
    ];
    let buttons = [304, 305, 307, 308, 310, 311, 314, 315, 316, 317, 318];
    Device {
        id: "linux-045e-0b12-preview".into(),
        name: "Xbox Wireless Controller".into(),
        rumble: true,
        controls: axes
            .iter()
            .map(|(id, min, max)| tore_input_native::Control {
                id: (*id).into(),
                kind: Kind::Axis,
                min: *min,
                max: *max,
                value: if *min == 0. { 0. } else { (min + max) / 2. },
            })
            .chain(buttons.iter().map(|b| tore_input_native::Control {
                id: format!("button:{b}"),
                kind: Kind::Button,
                min: 0.,
                max: 1.,
                value: 0.,
            }))
            .collect(),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tab {
    Keyboard,
    Mouse,
    Head,
    /// A native device identity, connected or referenced by the profile.
    Device(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Setting {
    Info(&'static str),
    Status,
    Rumble,
    GamepadDefaults,
    Modifiers,
    Deadzone,
    StickSensitivity,
    TriggerSensitivity,
    MouseLook,
    MouseSensitivity,
    MouseInvert,
    HeadSource,
    HeadYaw,
    HeadPitch,
    HeadInvertYaw,
    HeadInvertPitch,
}
impl Setting {
    fn focusable(self) -> bool {
        !matches!(self, Setting::Info(_) | Setting::Status)
    }
    fn toggle(self) -> bool {
        matches!(
            self,
            Setting::Rumble
                | Setting::GamepadDefaults
                | Setting::MouseLook
                | Setting::MouseInvert
                | Setting::HeadSource
                | Setting::HeadInvertYaw
                | Setting::HeadInvertPitch
        )
    }
    /// Stepped values show arrows and take left/right. Toggles flip on any
    /// activation; Modifiers starts a capture only when activated, so the
    /// D-pad can move past it without starting one.
    fn arrows(self) -> bool {
        !self.toggle() && self != Setting::Modifiers
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Line {
    /// `None` is the group of profile bindings the catalog does not list.
    Header(Option<Group>),
    Entry(usize),
    Other(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    Tab(usize),
    Setting(usize),
    /// Visible mapping line and column: 0 primary, 1 secondary, 2 invert,
    /// 3 curve, 4 clear.
    Cell(usize, usize),
    /// Search row control, indexed as `SEARCH`.
    Search(usize),
    Footer(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Tab(usize),
    Setting(usize, i32),
    Cell(usize, usize),
    Search(usize),
    Footer(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Source {
    Stock(&'static str),
    Binding(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Slot(usize, usize),
    Modifier,
    /// The next input pressed becomes the search.
    Find,
}

/// What the search row looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Find {
    /// Actions whose name or group contains the text: commands to bind.
    Actions,
    /// Actions whose inputs on this device match: keys already bound.
    Inputs,
}

#[derive(Clone, Debug, PartialEq)]
struct Search {
    find: Find,
    text: String,
    /// The raw control pressed to find it, in profile naming, for an exact
    /// match on this device; `text` then holds its label.
    pressed: Option<String>,
}

struct Capture {
    target: Target,
    /// Control values when capture began, so small drifts are ignored.
    rest: BTreeMap<String, f64>,
}

/// Where the pointer rests and since when. A tooltip shows once the pointer
/// has stayed on one control for [`TOOLTIP_DELAY`].
#[derive(Clone, Copy, Debug, Default)]
struct Hover {
    point: Option<(f64, f64)>,
    hit: Option<Hit>,
    since: Option<Instant>,
}

/// How long the pointer rests on a control before its tooltip shows.
const TOOLTIP_DELAY: Duration = Duration::from_millis(400);
/// The invert box's tooltip (John, 2026-10-23): the box has no room for a label.
const INVERT_TIP: &str = "INV: invert this axis";
/// The response curve button's values, which a click turns through.
const CURVES: [f64; 4] = [1., 1.5, 2., 3.];

/// The response curve after (or before) `curve`, wrapping at both ends. A
/// value off the list (an edited profile) counts as the listed value it is
/// nearest to, rounding up at the halfway points of 1.25, 1.75 and 2.5.
pub(crate) fn turn_curve(curve: f64, forward: bool) -> f64 {
    let at = match curve {
        x if x < 1.25 => 0,
        x if x < 1.75 => 1,
        x if x < 2.5 => 2,
        _ => 3,
    };
    CURVES[(at + if forward { 1 } else { CURVES.len() - 1 }) % CURVES.len()]
}

pub(crate) type Rect = (i32, i32, i32, i32);
const LEFT: Rect = (6, 24, 150, 412);
const TAB_TOP: i32 = 42;
const TAB_HEIGHT: i32 = 34;
const RIGHT_X: i32 = 162;
const RIGHT_W: i32 = 472;
const SETTINGS_TOP: i32 = 42;
const SETTING_HEIGHT: i32 = 15;
const MAX_SETTINGS: usize = 6;
const MAP_TOP: i32 = 144;
const SEARCH_TOP: i32 = 162;
const LIST_TOP: i32 = 194;
const LINE_HEIGHT: i32 = 14;
const LIST_LINES: usize = 17;
/// Search row controls, as (x from `RIGHT_X`, width): Actions and Keys
/// choices, the text field, Press to find an input, and Clear. The field and
/// buttons line up with the mapping columns below.
const SEARCH: [(i32, i32); 5] = [(40, 54), (96, 54), (158, 222), (386, 38), (428, 40)];
const SEARCH_FIELD: usize = 2;
const SEARCH_CHARS: usize = 32;
const FOOTER_Y: i32 = 456;
const FOOTER: [&str; 3] = ["Apply", "Reset device", "Back"];
const COLUMNS: [(i32, i32); 5] = [(158, 104), (264, 104), (372, 16), (390, 34), (428, 40)];

pub(crate) const INK: [u8; 4] = [20, 39, 65, 255];
pub(crate) const PAPER: [u8; 4] = [24, 34, 45, 255];
pub(crate) const PANEL: [u8; 4] = [36, 52, 72, 255];
pub(crate) const TITLE: [u8; 4] = [240, 233, 194, 255];
pub(crate) const PALE: [u8; 4] = [201, 210, 222, 255];
pub(crate) const PALE_ALT: [u8; 4] = [190, 200, 214, 255];
/// The tooltip's colours: the menu bar's hint strip and its outline.
const TIP_FILL: [u8; 4] = [22, 31, 43, 255];
const TIP_EDGE: [u8; 4] = [145, 161, 180, 255];
const TIP_TEXT: [u8; 4] = [235, 239, 243, 255];
const GROUP: [u8; 4] = [150, 166, 188, 255];
pub(crate) const FOCUS: [u8; 4] = [62, 86, 118, 255];
pub(crate) const WHITE: [u8; 4] = [247, 250, 255, 255];
pub(crate) const MUTED: [u8; 4] = [150, 162, 178, 255];
pub(crate) const GOOD: [u8; 4] = [120, 214, 140, 255];
pub(crate) const BUTTON: [u8; 4] = [98, 118, 146, 255];
pub(crate) const HEADER: [u8; 4] = [44, 64, 88, 255];

pub struct Editor {
    pub profile: Profile,
    pub devices: Vec<Device>,
    pub message: String,
    /// Shown at the top right: where the screen was opened from.
    pub context: &'static str,
    /// Head-tracker receiver status, refreshed by the host each poll.
    pub head_status: String,
    capture: Option<Capture>,
    tab: usize,
    focus: Focus,
    scroll: usize,
    collapsed: BTreeSet<Option<Group>>,
    pressed: Option<Hit>,
    /// The control a right press went down on, which turns it back on release.
    right_pressed: Option<Hit>,
    /// What the pointer rests on, for the hover tooltip.
    hover: Hover,
    /// Latest raw value of every native control seen, for held modifiers.
    held: BTreeMap<(String, String), f64>,
    search: Search,
    dirty: bool,
}

fn norm(control: &tore_input_native::Control, value: f64) -> f64 {
    if matches!(control.kind, Kind::Axis) && control.max > control.min {
        ((value - control.min) / (control.max - control.min) * 2. - 1.).clamp(-1., 1.)
    } else {
        value
    }
}
/// Linux hats are integer axes from -1 to 1.
fn is_hat(control: &tore_input_native::Control) -> bool {
    matches!(control.kind, Kind::Position)
        || (matches!(control.kind, Kind::Axis) && control.min == -1. && control.max == 1.)
}
pub(crate) fn text_width(font: &Font, text: &str) -> i32 {
    text.glyph_codes()
        .map(|c| font.glyphs[c as usize].advance as i32)
        .sum()
}
pub(crate) fn fit(font: &Font, text: &str, width: i32) -> String {
    if text_width(font, text) <= width {
        return text.into();
    }
    let advance = |c: u8| font.glyphs[c as usize].advance as i32;
    let mut out = String::new();
    let mut used = 2 * advance(b'.');
    for c in text.chars() {
        used += advance(tore_formats::text::cp437_code(c));
        if used > width {
            break;
        }
        out.push(c);
    }
    out + ".."
}
pub(crate) fn inside(p: (f64, f64), r: Rect) -> bool {
    p.0 >= r.0 as f64 && p.1 >= r.1 as f64 && p.0 < (r.0 + r.2) as f64 && p.1 < (r.1 + r.3) as f64
}
/// Removal order that keeps the remaining binding indices valid.
fn highest_first(sources: &mut [Source]) {
    sources.sort_by_key(|s| match s {
        Source::Binding(i) => std::cmp::Reverse(*i),
        Source::Stock(_) => std::cmp::Reverse(usize::MAX),
    });
}

/// Clears the canvas and draws the title strip: the screen name on the left,
/// where it was opened from on the right. Shared with the Graphics screen.
pub(crate) fn title_bar(pixels: &mut [u8], font: &Font, title: &str, context: &str) {
    let full = (0, 0, 640, 480);
    Canvas(pixels).rect(full, PAPER);
    Canvas(pixels).rect((0, 0, 640, 20), PANEL);
    Editor::text(pixels, font, full, TITLE, title, (8, 5));
    let context = context.to_ascii_uppercase();
    Editor::text(
        pixels,
        font,
        full,
        TITLE,
        &context,
        (632 - text_width(font, &context), 5),
    );
}
/// The bottom strip: a message line above the navigation hint. Footer
/// buttons sit to the right of the hint.
pub(crate) fn status_bar(pixels: &mut [u8], font: &Font, message: &str) {
    message_bar(pixels, font, message);
    Editor::text(
        pixels,
        font,
        (8, 456, 300, 20),
        MUTED,
        "Arrows move, Enter selects, Esc backs out",
        (8, 461),
    );
}
/// The bottom strip with only its message line, for a screen whose footer
/// buttons span the whole width (the Replays screen).
pub(crate) fn message_bar(pixels: &mut [u8], font: &Font, message: &str) {
    Canvas(pixels).rect((0, 438, 640, 42), PANEL);
    Editor::text(
        pixels,
        font,
        (8, 440, 624, 14),
        TITLE,
        &fit(font, message, 620),
        (8, 442),
    );
}
pub(crate) fn footer_button(pixels: &mut [u8], font: &Font, r: Rect, label: &str, focused: bool) {
    Canvas(pixels).rect(r, if focused { FOCUS } else { PALE });
    let color = if focused { WHITE } else { INK };
    let x = r.0 + (r.2 - text_width(font, label)) / 2;
    Editor::text(pixels, font, r, color, label, (x, r.1 + 4));
}

impl Editor {
    pub fn new(profile: Profile, devices: Vec<Device>, context: &'static str) -> Self {
        let mut editor = Self {
            profile,
            devices,
            message: "Select a device, then click an input to change it.".into(),
            context,
            head_status: String::new(),
            capture: None,
            tab: 0,
            focus: Focus::Tab(0),
            scroll: 0,
            collapsed: BTreeSet::new(),
            pressed: None,
            right_pressed: None,
            hover: Hover::default(),
            held: BTreeMap::new(),
            search: Search {
                find: Find::Actions,
                text: String::new(),
                pressed: None,
            },
            dirty: false,
        };
        // Older chord profiles name their modifiers only inside bindings.
        let implied: Vec<(String, String)> = editor
            .profile
            .bindings
            .iter()
            .flat_map(|b| {
                let (mods, _) = tore_input::chord_parts(&b.control);
                mods.into_iter()
                    .map(|m| (b.device.clone(), m.to_owned()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for m in implied {
            if !editor.profile.modifiers.contains(&m) {
                editor.profile.modifiers.push(m);
            }
        }
        // Open on the first connected device: it is what players come to set up.
        if !editor.devices.is_empty() {
            editor.tab = 2;
        }
        editor.focus = Focus::Tab(editor.tab);
        editor
    }
    /// Snapshot helper: `""` keeps the opening tab; `keyboard`, `mouse` and
    /// `head` select those tabs. `search` searches the opening tab's actions
    /// for "fire"; `search-keys` finds the keyboard actions on Shift keys;
    /// `inv` rests the pointer on the first axis row's invert box long enough
    /// for its tooltip to show.
    pub fn preview(&mut self, tab: &str) -> Result<(), String> {
        let tabs = self.tabs();
        self.tab = match tab {
            "" | "search" | "inv" => self.tab,
            "keyboard" | "search-keys" => 0,
            "mouse" => 1,
            "head" => tabs.len() - 1,
            _ => {
                return Err(
                    "controls snapshot tab must be keyboard, mouse, head, search, search-keys or inv"
                        .into(),
                );
            }
        };
        self.focus = Focus::Tab(self.tab);
        if tab == "inv" {
            let lines = self.lines();
            let row = (0..LIST_LINES.min(lines.len()))
                .find(|row| self.columns(self.scroll + row).contains(&2))
                .ok_or("the controls snapshot has no axis row for its invert box")?;
            let point = (
                (RIGHT_X + COLUMNS[2].0 + 8) as f64,
                (Self::line_rect(row).1 + 6) as f64,
            );
            self.hover = Hover {
                point: Some(point),
                hit: self.hit(point),
                since: Instant::now().checked_sub(TOOLTIP_DELAY * 2),
            };
        }
        if let Some(text) = match tab {
            "search" => Some("fire"),
            "search-keys" => Some("shift"),
            _ => None,
        } {
            if tab == "search-keys" {
                self.search.find = Find::Inputs;
            }
            self.focus = Focus::Search(SEARCH_FIELD);
            self.text_input(text);
        }
        Ok(())
    }
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }
    /// Whether typed text goes to the search field. The host sends printable
    /// characters to `text_input` instead of `key` while this holds.
    pub fn typing(&self) -> bool {
        self.capture.is_none() && self.focus == Focus::Search(SEARCH_FIELD) && self.search_shown()
    }
    /// Printable text typed into the search field. Typing redraws without
    /// the click sound buttons make.
    pub fn text_input(&mut self, text: &str) -> ResultAction {
        if !self.typing() {
            return ResultAction::None;
        }
        if self.search.pressed.take().is_some() {
            self.search.text.clear();
        }
        for c in text.chars().filter(|c| c.is_ascii_graphic() || *c == ' ') {
            if self.search.text.len() < SEARCH_CHARS {
                self.search.text.push(c);
            }
        }
        self.searched();
        ResultAction::None
    }
    pub fn cancel_capture(&mut self) {
        self.capture = None;
        self.pressed = None;
    }
    pub fn saved(&mut self) {
        self.dirty = false;
    }

    // ---- model ----------------------------------------------------------

    fn tabs(&self) -> Vec<Tab> {
        let mut tabs = vec![Tab::Keyboard, Tab::Mouse];
        let mut ids: Vec<String> = self.devices.iter().map(|d| d.id.clone()).collect();
        for b in &self.profile.bindings {
            let id = self.profile.aliases.get(&b.device).unwrap_or(&b.device);
            if !matches!(
                id.as_str(),
                "keyboard" | "mouse" | "replay-keyboard" | "replay-mouse" | "*"
            ) && !ids.contains(id)
            {
                ids.push(id.clone());
            }
        }
        tabs.extend(ids.into_iter().map(Tab::Device));
        tabs.push(Tab::Head);
        tabs
    }
    fn current(&self) -> Tab {
        let tabs = self.tabs();
        tabs[self.tab.min(tabs.len() - 1)].clone()
    }
    fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }
    fn class(&self, id: &str) -> DeviceClass {
        self.device(id).map_or(DeviceClass::Other, classify)
    }
    /// The device reference new bindings use: an existing alias, `*` for
    /// session-only Apple gamepads, otherwise the identity.
    fn reference(&self, id: &str) -> String {
        if let Some((alias, _)) =
            self.profile.aliases.iter().find(|(alias, v)| {
                !alias.starts_with(crate::input::AUTO_GAMEPAD_PREFIX) && *v == id
            })
        {
            return alias.clone();
        }
        if id.starts_with("macos-gc-session-") {
            return "*".into();
        }
        id.into()
    }
    fn names_device(&self, reference: &str, id: &str) -> bool {
        reference == id
            || self.profile.aliases.get(reference).is_some_and(|v| v == id)
            || (reference == "*"
                && (id.starts_with("macos-gc-session-") || self.class(id) == DeviceClass::Gamepad))
    }
    fn on_tab(&self, binding: &Binding, tab: &Tab) -> bool {
        match tab {
            Tab::Keyboard => matches!(binding.device.as_str(), "keyboard" | "replay-keyboard"),
            Tab::Mouse => matches!(binding.device.as_str(), "mouse" | "replay-mouse"),
            Tab::Head => false,
            Tab::Device(id) => self.names_device(&binding.device, id),
        }
    }
    /// Xbox control names apply to gamepads and to disconnected Linux pads.
    fn gamepad_tab(&self, tab: &Tab) -> bool {
        matches!(tab, Tab::Device(id) if self.class(id) == DeviceClass::Gamepad
            || (id.starts_with("linux-") && self.device(id).is_none()))
    }
    fn stock(&self, entry: &Entry, tab: &Tab) -> Vec<&'static str> {
        let (device, stock) = match tab {
            Tab::Keyboard => ("keyboard", entry.keys),
            Tab::Mouse => ("mouse", entry.mouse),
            _ => return vec![],
        };
        stock
            .iter()
            .copied()
            .filter(|k| {
                !self
                    .profile
                    .disabled
                    .contains(&(entry.device(device).to_owned(), (*k).to_owned()))
            })
            .collect()
    }
    fn slots(&self, index: usize, tab: &Tab) -> Vec<Source> {
        let entry = &ENTRIES[index];
        let mut slots: Vec<Source> = self
            .stock(entry, tab)
            .into_iter()
            .map(Source::Stock)
            .collect();
        if entry.bindable() {
            slots.extend(
                self.profile
                    .bindings
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| self.on_tab(b, tab) && entry.matches(b))
                    .map(|(i, _)| Source::Binding(i)),
            );
        }
        slots
    }
    fn source_label(&self, source: &Source, tab: &Tab) -> String {
        match source {
            Source::Stock(key) => match tab {
                Tab::Mouse => catalog::mouse_label(key),
                _ => catalog::key_label(key),
            },
            Source::Binding(i) => {
                let b = &self.profile.bindings[*i];
                catalog::control_label(&b.device, &b.control, b.mode, self.gamepad_tab(tab))
            }
        }
    }
    fn shown(&self, entry: &Entry, tab: &Tab) -> bool {
        match tab {
            Tab::Keyboard => entry.digital() && !catalog::menu_only(entry),
            Tab::Mouse => entry.bindable() && entry.digital(),
            Tab::Head => false,
            Tab::Device(_) => entry.bindable(),
        }
    }
    fn lines(&self) -> Vec<Line> {
        let tab = self.current();
        let searching = self.searching();
        let mut lines = vec![];
        for group in Group::ALL {
            let entries: Vec<usize> = ENTRIES
                .iter()
                .enumerate()
                .filter(|(i, e)| {
                    e.group == group
                        && self.shown(e, &tab)
                        && (!searching || self.entry_found(*i, &tab))
                })
                .map(|(i, _)| i)
                .collect();
            if entries.is_empty() {
                continue;
            }
            lines.push(Line::Header(Some(group)));
            // Folded groups open while searching, so nothing found is hidden.
            if searching || !self.collapsed.contains(&Some(group)) {
                lines.extend(entries.into_iter().map(Line::Entry));
            }
        }
        let others: Vec<usize> = self
            .profile
            .bindings
            .iter()
            .enumerate()
            .filter(|(i, b)| {
                self.on_tab(b, &tab)
                    && !ENTRIES.iter().any(|e| self.shown(e, &tab) && e.matches(b))
                    && (!searching || self.other_found(*i, &tab))
            })
            .map(|(i, _)| i)
            .collect();
        if !others.is_empty() {
            lines.push(Line::Header(None));
            if searching || !self.collapsed.contains(&None) {
                lines.extend(others.into_iter().map(Line::Other));
            }
        }
        lines
    }

    // ---- search ---------------------------------------------------------

    fn search_shown(&self) -> bool {
        self.current() != Tab::Head
    }
    fn searching(&self) -> bool {
        self.search_shown()
            && (self.search.pressed.is_some() || !self.search.text.trim().is_empty())
    }
    fn query(&self) -> String {
        self.search.text.trim().to_ascii_lowercase()
    }
    /// Action text matches anywhere in the name or the group title.
    fn action_found(&self, text: &str) -> bool {
        text.to_ascii_lowercase().contains(&self.query())
    }
    /// Input labels match a whole word ("F" finds F and Ctrl+F, not F1 or
    /// Left); three or more characters also match anywhere ("pag" finds
    /// Page Up).
    fn label_found(&self, label: &str) -> bool {
        let query = self.query();
        let label = label.to_ascii_lowercase();
        label.split([' ', '+']).any(|word| word == query)
            || (query.len() >= 3 && label.contains(&query))
    }
    /// Whether an input is the one pressed to find it. Keyboard and mouse
    /// inputs match exactly, modifiers included; a device input matches any
    /// part of a chord, so finding a modifier button lists every combination
    /// that uses it.
    fn source_found(&self, source: &Source, tab: &Tab) -> bool {
        let Some(found) = &self.search.pressed else {
            return self.label_found(&self.source_label(source, tab));
        };
        let b = match source {
            Source::Stock(key) => return key == found,
            Source::Binding(i) => &self.profile.bindings[*i],
        };
        if !matches!(tab, Tab::Device(_)) {
            return &b.control == found;
        }
        let (mods, base) = tore_input::chord_parts(&b.control);
        let base = match b.mode {
            Mode::Position(n) => format!("{base}={n}"),
            _ => base.to_owned(),
        };
        let same = |bound: &str| {
            // Hat directions must agree; otherwise one physical control is one input.
            if bound.contains('=') && found.contains('=') {
                bound == found
            } else {
                tore_input::token_base(bound) == tore_input::token_base(found)
            }
        };
        mods.iter().any(|m| same(m)) || same(&base)
    }
    fn entry_found(&self, index: usize, tab: &Tab) -> bool {
        let entry = &ENTRIES[index];
        match self.search.find {
            Find::Actions => {
                self.action_found(entry.label) || self.action_found(entry.group.title())
            }
            Find::Inputs => self
                .slots(index, tab)
                .iter()
                .any(|s| self.source_found(s, tab)),
        }
    }
    fn other_found(&self, binding: usize, tab: &Tab) -> bool {
        match self.search.find {
            Find::Actions => {
                self.action_found(&action_name(&self.profile.bindings[binding].action))
            }
            Find::Inputs => self.source_found(&Source::Binding(binding), tab),
        }
    }
    /// After the search changes: back to the top of what it found.
    fn searched(&mut self) {
        self.scroll = 0;
        self.keep_visible();
    }
    fn clear_search(&mut self) -> ResultAction {
        self.search.text.clear();
        self.search.pressed = None;
        if matches!(
            self.capture,
            Some(Capture {
                target: Target::Find,
                ..
            })
        ) {
            self.capture = None;
        }
        self.searched();
        ResultAction::Changed
    }
    /// Label of an input pressed to find it, as its rows show it.
    fn found_label(&self, token: &str, tab: &Tab) -> String {
        match tab {
            Tab::Keyboard => catalog::key_label(token),
            Tab::Mouse => catalog::mouse_label(token),
            _ => catalog::control_label("pad", token, Mode::Press, self.gamepad_tab(tab)),
        }
    }
    fn found(&mut self, token: String) {
        let tab = self.current();
        let label = self.found_label(&token, &tab);
        self.search = Search {
            find: Find::Inputs,
            text: label.clone(),
            pressed: Some(token),
        };
        self.capture = None;
        self.searched();
        let count = self
            .lines()
            .iter()
            .filter(|l| !matches!(l, Line::Header(_)))
            .count();
        self.message = match count {
            0 => format!("{label} is not bound on this device"),
            1 => format!("{label} is bound to 1 action"),
            n => format!("{label} is bound to {n} actions"),
        };
    }
    fn settings(&self) -> Vec<Setting> {
        match self.current() {
            Tab::Keyboard => vec![
                Setting::Info("Stock keys are listed first. Change or clear any of them."),
                Setting::Info("Hold Ctrl, Alt or Shift, in any mix, while capturing a key."),
                Setting::Info("A key taken for one action is removed from its old action."),
                Setting::Info("Esc, Alt+Enter and Alt+F4 always keep their stock meaning."),
            ],
            Tab::Mouse => vec![
                Setting::MouseLook,
                Setting::MouseSensitivity,
                Setting::MouseInvert,
                Setting::Info("Left click always operates instruments and the HUD."),
            ],
            Tab::Head => vec![
                Setting::HeadSource,
                Setting::HeadYaw,
                Setting::HeadPitch,
                Setting::HeadInvertYaw,
                Setting::HeadInvertPitch,
                Setting::Status,
            ],
            tab @ Tab::Device(_) => {
                let mut s = vec![];
                if let Tab::Device(id) = &tab
                    && self.device(id).is_some_and(|d| d.rumble)
                {
                    s.push(Setting::Rumble);
                }
                s.push(Setting::Modifiers);
                s.push(Setting::Deadzone);
                s.push(Setting::StickSensitivity);
                if self.gamepad_tab(&tab) {
                    s.push(Setting::TriggerSensitivity);
                    s.push(Setting::GamepadDefaults);
                }
                s.truncate(MAX_SETTINGS);
                s
            }
        }
    }
    fn modifiers(&self, tab: &Tab) -> Vec<usize> {
        self.profile
            .modifiers
            .iter()
            .enumerate()
            .filter(|(_, (d, _))| matches!(tab, Tab::Device(id) if self.names_device(d, id)))
            .map(|(i, _)| i)
            .collect()
    }
    /// Bindings on this device whose calibration a device-wide setting drives.
    fn calibrated(&self, tab: &Tab, trigger: bool) -> Vec<usize> {
        self.profile
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                self.on_tab(b, tab)
                    && if trigger {
                        matches!(b.mode, Mode::Trigger(_))
                    } else {
                        b.mode == Mode::Axis
                            && !matches!(b.action, Action::Axis(Axis::HeadYaw | Axis::HeadPitch))
                    }
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn deadzone(&self, tab: &Tab) -> f64 {
        let mut all = self.calibrated(tab, false);
        all.extend(self.calibrated(tab, true));
        all.first()
            .map_or(0.1, |i| self.profile.bindings[*i].calibration.deadzone)
    }
    fn sensitivity(&self, tab: &Tab, trigger: bool) -> f64 {
        self.calibrated(tab, trigger)
            .first()
            .map_or(1., |i| self.profile.bindings[*i].calibration.scale.abs())
    }
    fn setting_text(&self, setting: Setting) -> (String, String) {
        let tab = self.current();
        let on = |v: bool| if v { "On" } else { "Off" }.to_owned();
        match setting {
            Setting::Info(text) => (text.into(), String::new()),
            Setting::Status => ("Status".into(), self.head_status.clone()),
            Setting::Rumble => ("Rumble / vibration".into(), on(self.profile.rumble)),
            Setting::GamepadDefaults => (
                "Defaults for new gamepads".into(),
                on(self.profile.gamepad_defaults),
            ),
            Setting::Modifiers => {
                let names: Vec<String> = self
                    .modifiers(&tab)
                    .into_iter()
                    .map(|i| {
                        let (_, m) = &self.profile.modifiers[i];
                        catalog::control_label("pad", m, Mode::Press, self.gamepad_tab(&tab))
                    })
                    .collect();
                (
                    "Modifier buttons".into(),
                    if names.is_empty() {
                        "None".into()
                    } else {
                        names.join(", ")
                    },
                )
            }
            Setting::Deadzone => ("Deadzone".into(), format!("{:.2}", self.deadzone(&tab))),
            Setting::StickSensitivity => (
                "Stick sensitivity".into(),
                format!("{:.2}", self.sensitivity(&tab, false)),
            ),
            Setting::TriggerSensitivity => (
                "Trigger sensitivity".into(),
                format!("{:.2}", self.sensitivity(&tab, true)),
            ),
            Setting::MouseLook => (
                "Mouse look (hold right button)".into(),
                on(self.profile.mouse_look),
            ),
            Setting::MouseSensitivity => (
                "Mouse look sensitivity".into(),
                format!("{:.1}", self.profile.mouse_sensitivity),
            ),
            Setting::MouseInvert => (
                "Invert mouse look up/down".into(),
                on(self.profile.mouse_invert),
            ),
            Setting::HeadSource => (
                "opentrack UDP receiver".into(),
                self.profile
                    .head_port
                    .map_or("Off".into(), |p| format!("On, port {p}")),
            ),
            Setting::HeadYaw => (
                "Yaw sensitivity".into(),
                format!("{:.1}", self.profile.head_scale[0].abs()),
            ),
            Setting::HeadPitch => (
                "Pitch sensitivity".into(),
                format!("{:.1}", self.profile.head_scale[1].abs()),
            ),
            Setting::HeadInvertYaw => ("Invert yaw".into(), on(self.profile.head_scale[0] < 0.)),
            Setting::HeadInvertPitch => {
                ("Invert pitch".into(), on(self.profile.head_scale[1] < 0.))
            }
        }
    }

    // ---- editing --------------------------------------------------------

    fn changed(&mut self) -> ResultAction {
        self.dirty = true;
        ResultAction::Changed
    }
    fn adjust(&mut self, setting: Setting, delta: i32) -> ResultAction {
        let tab = self.current();
        let step = f64::from(delta.signum());
        match setting {
            Setting::Info(_) | Setting::Status => return ResultAction::None,
            Setting::Rumble => self.profile.rumble = !self.profile.rumble,
            Setting::GamepadDefaults => {
                self.profile.gamepad_defaults = !self.profile.gamepad_defaults
            }
            Setting::Modifiers => {
                // Adding and removing both happen in the capture that follows.
                self.begin(Target::Modifier);
                return ResultAction::Changed;
            }
            Setting::Deadzone => {
                let v = ((self.deadzone(&tab) + 0.01 * step) * 100.).round() / 100.;
                let mut all = self.calibrated(&tab, false);
                all.extend(self.calibrated(&tab, true));
                for i in all {
                    self.profile.bindings[i].calibration.deadzone = v.clamp(0., 0.5);
                }
            }
            Setting::StickSensitivity | Setting::TriggerSensitivity => {
                let trigger = setting == Setting::TriggerSensitivity;
                let v = ((self.sensitivity(&tab, trigger) + 0.05 * step) * 20.).round() / 20.;
                for i in self.calibrated(&tab, trigger) {
                    let c = &mut self.profile.bindings[i].calibration;
                    c.scale = v.clamp(0.1, 1.).copysign(c.scale);
                }
            }
            Setting::MouseLook => self.profile.mouse_look = !self.profile.mouse_look,
            Setting::MouseSensitivity => {
                self.profile.mouse_sensitivity = ((self.profile.mouse_sensitivity + 0.1 * step)
                    * 10.)
                    .round()
                    .clamp(1., 50.)
                    / 10.
            }
            Setting::MouseInvert => self.profile.mouse_invert = !self.profile.mouse_invert,
            Setting::HeadSource => {
                self.profile.head_port = match self.profile.head_port {
                    Some(_) => None,
                    None => Some(4242),
                }
            }
            Setting::HeadYaw | Setting::HeadPitch => {
                let i = usize::from(setting == Setting::HeadPitch);
                let s = self.profile.head_scale[i];
                self.profile.head_scale[i] =
                    ((s.abs() + 0.1 * step) * 10.).round().clamp(1., 30.) / 10. * s.signum();
            }
            Setting::HeadInvertYaw => self.profile.head_scale[0] = -self.profile.head_scale[0],
            Setting::HeadInvertPitch => self.profile.head_scale[1] = -self.profile.head_scale[1],
        }
        self.changed()
    }
    fn begin(&mut self, target: Target) {
        let tab = self.current();
        let mut rest = BTreeMap::new();
        if let Tab::Device(id) = &tab
            && let Some(d) = self.device(id)
        {
            for c in &d.controls {
                let v = self
                    .held
                    .get(&(id.clone(), c.id.clone()))
                    .copied()
                    .unwrap_or(c.value);
                rest.insert(c.id.clone(), v);
            }
        }
        self.message = match (&target, &tab) {
            (Target::Modifier, _) => {
                "Press a button or D-pad direction to add it as a modifier, or a modifier to remove it. Esc cancels.".into()
            }
            (Target::Find, Tab::Keyboard) => {
                "Press the key to find, with any Ctrl/Alt/Shift. Esc cancels.".into()
            }
            (Target::Find, Tab::Mouse) => {
                "Click the middle, side or right button, or turn the wheel, to find it.".into()
            }
            (Target::Find, _) => {
                "Press the button or move the axis to find. Esc cancels.".into()
            }
            (Target::Slot(i, _), Tab::Keyboard) => format!(
                "Press a key for {}. Hold Ctrl/Alt/Shift to combine. Esc cancels.",
                ENTRIES[*i].label
            ),
            (Target::Slot(i, _), Tab::Mouse) => format!(
                "Click the middle, side or right button, or turn the wheel, for {}.",
                ENTRIES[*i].label
            ),
            (Target::Slot(i, _), _) => format!(
                "Press a button or move an axis for {}. Hold modifiers first. Esc cancels.",
                ENTRIES[*i].label
            ),
        };
        self.capture = Some(Capture { target, rest });
    }
    fn remove_source(&mut self, source: &Source, tab: &Tab, entry: &Entry) {
        match source {
            Source::Stock(key) => {
                let device = if *tab == Tab::Mouse {
                    "mouse"
                } else {
                    "keyboard"
                };
                self.profile
                    .disabled
                    .insert((entry.device(device).into(), (*key).into()));
            }
            Source::Binding(i) => {
                self.profile.bindings.remove(*i);
            }
        }
    }
    fn clear_slot(&mut self, index: usize, slot: Option<usize>) -> ResultAction {
        let tab = self.current();
        let entry = &ENTRIES[index];
        if entry.fixed {
            self.message = format!("{} is built in and cannot be cleared", entry.label);
            return ResultAction::Changed;
        }
        let mut slots = self.slots(index, &tab);
        if let Some(slot) = slot {
            if slot >= slots.len() {
                return ResultAction::None;
            }
            slots = vec![slots.remove(slot)];
        }
        highest_first(&mut slots);
        for source in slots {
            self.remove_source(&source, &tab, entry);
        }
        self.message = format!("{} cleared on this device", entry.label);
        self.keep_visible();
        self.changed()
    }
    /// The invert box (a toggle, so either direction flips it) or the response
    /// curve button, which turns to its next value forward or its previous
    /// one back, wrapping at both ends.
    fn axis_option(&mut self, index: usize, invert: bool, forward: bool) -> ResultAction {
        let tab = self.current();
        let bindings: Vec<usize> = self
            .slots(index, &tab)
            .into_iter()
            .filter_map(|s| match s {
                Source::Binding(i) => Some(i),
                Source::Stock(_) => None,
            })
            .collect();
        if bindings.is_empty() {
            self.message = "Assign an axis first".into();
            return ResultAction::Changed;
        }
        for i in bindings {
            let c = &mut self.profile.bindings[i].calibration;
            if invert {
                c.scale = -c.scale;
            } else {
                c.curve = turn_curve(c.curve, forward);
            }
        }
        self.changed()
    }
    fn reset_device(&mut self) -> ResultAction {
        let tab = self.current();
        match &tab {
            Tab::Keyboard | Tab::Mouse => {
                let device = if tab == Tab::Mouse {
                    "mouse"
                } else {
                    "keyboard"
                };
                let replay = format!("replay-{device}");
                self.profile
                    .disabled
                    .retain(|(d, _)| d != device && *d != replay);
                self.profile
                    .bindings
                    .retain(|b| b.device != device && b.device != replay);
                if tab == Tab::Mouse {
                    let d = Profile::default();
                    self.profile.mouse_look = d.mouse_look;
                    self.profile.mouse_sensitivity = d.mouse_sensitivity;
                    self.profile.mouse_invert = d.mouse_invert;
                }
            }
            Tab::Head => {
                let d = Profile::default();
                self.profile.head_port = d.head_port;
                self.profile.head_scale = d.head_scale;
            }
            Tab::Device(id) => {
                for i in self.modifiers(&tab).into_iter().rev() {
                    self.profile.modifiers.remove(i);
                }
                let keep: Vec<Binding> = self
                    .profile
                    .bindings
                    .iter()
                    .filter(|b| !self.on_tab(b, &tab))
                    .cloned()
                    .collect();
                self.profile.bindings = keep;
                if let Some(d) = self.device(id) {
                    let defaults = crate::input::gamepad_defaults(d);
                    self.profile.bindings.extend(defaults.bindings);
                    self.profile.aliases.extend(defaults.aliases);
                    self.profile.modifiers.extend(defaults.modifiers);
                }
            }
        }
        self.message = "Device restored to its defaults. Apply to keep it.".into();
        self.scroll = 0;
        self.changed()
    }
    /// Stores a captured input in the target slot, replacing what was there.
    fn assign(&mut self, index: usize, slot: usize, mut binding: Binding) {
        let tab = self.current();
        let entry = &ENTRIES[index];
        let digital = matches!(tab, Tab::Keyboard | Tab::Mouse);
        let mut notes = vec![];
        // On a keyboard or mouse one control does one thing, so it is taken
        // from any other action. Devices keep shared assignments (A is gear
        // in flight and select in menus); those are only reported.
        let mut remove: Vec<Source> = vec![];
        for (other, e) in ENTRIES.iter().enumerate() {
            if other == index
                || catalog::menu_only(e)
                || e.fixed
                || e.replay_only() != entry.replay_only()
            {
                continue;
            }
            for source in self.slots(other, &tab) {
                let same = match &source {
                    Source::Stock(k) => *k == binding.control,
                    Source::Binding(i) => {
                        let b = &self.profile.bindings[*i];
                        b.control == binding.control && b.mode == binding.mode
                    }
                };
                if same {
                    notes.push(e.label);
                    if digital {
                        remove.push(source);
                    }
                }
            }
        }
        let previous = self.slots(index, &tab).get(slot).cloned();
        if let Some(previous) = &previous {
            remove.push(previous.clone());
        }
        // Giving an entry back its own stock key needs no custom binding.
        let own_stock = match tab {
            Tab::Keyboard => entry.keys,
            Tab::Mouse => entry.mouse,
            _ => &[],
        };
        let restores_stock = own_stock.contains(&binding.control.as_str());
        highest_first(&mut remove);
        remove.dedup();
        // A replaced binding keeps its place, so primary stays primary.
        let insert_at = match previous {
            Some(Source::Binding(at)) => Some(
                at - remove
                    .iter()
                    .filter(|s| matches!(s, Source::Binding(i) if *i < at))
                    .count(),
            ),
            _ => None,
        };
        for source in &remove {
            self.remove_source(source, &tab, entry);
        }
        let shown = match tab {
            Tab::Keyboard => catalog::key_label(&binding.control),
            Tab::Mouse => catalog::mouse_label(&binding.control),
            _ => catalog::control_label(
                &binding.device,
                &binding.control,
                binding.mode,
                self.gamepad_tab(&tab),
            ),
        };
        if restores_stock {
            let device = if tab == Tab::Mouse {
                "mouse"
            } else {
                "keyboard"
            };
            self.profile
                .disabled
                .remove(&(entry.device(device).into(), binding.control.clone()));
        } else {
            if let Some(action) = entry.parsed() {
                binding.action = action;
            }
            match insert_at {
                Some(at) => self.profile.bindings.insert(at, binding),
                None => self.profile.bindings.push(binding),
            }
        }
        self.message = if notes.is_empty() {
            format!("{} set to {shown}. Apply to keep it.", entry.label)
        } else if digital {
            format!(
                "{} set to {shown}; removed from {}.",
                entry.label,
                notes.join(", ")
            )
        } else {
            format!(
                "{} set to {shown}; also used for {}.",
                entry.label,
                notes.join(", ")
            )
        };
        self.capture = None;
        self.dirty = true;
    }
    fn digital_binding(&self, index: usize, device: &str, control: String) -> Binding {
        let entry = &ENTRIES[index];
        // Keyboard directions match the stock keyboard axes' priority.
        let (mode, priority) = match entry.kind {
            Row::Direction(sign) => (Mode::Hold(sign), 100),
            Row::Hold => (Mode::HoldState, 10),
            _ => (Mode::Press, 10),
        };
        Binding {
            device: entry.device(device).into(),
            control,
            action: entry.parsed().unwrap_or(Action::Ui("pause".into())),
            mode,
            calibration: Calibration {
                deadzone: 0.,
                ..Calibration::default()
            },
            priority,
        }
    }

    // ---- navigation -----------------------------------------------------

    fn columns(&self, line: usize) -> Vec<usize> {
        match self.lines().get(line) {
            Some(Line::Entry(i)) => {
                if matches!(ENTRIES[*i].kind, Row::Axis | Row::Lever | Row::Head)
                    && matches!(self.current(), Tab::Device(_))
                {
                    vec![0, 1, 2, 3, 4]
                } else {
                    vec![0, 1, 4]
                }
            }
            Some(Line::Other(_)) => vec![4],
            _ => vec![0],
        }
    }
    fn keep_visible(&mut self) {
        let total = self.lines().len();
        if let Focus::Cell(line, _) = self.focus
            && line >= total
            && total > 0
        {
            self.focus = Focus::Cell(total - 1, 0);
        }
        if let Focus::Cell(line, _) = self.focus {
            if line < self.scroll {
                self.scroll = line;
            } else if line >= self.scroll + LIST_LINES {
                self.scroll = line + 1 - LIST_LINES;
            }
        }
        self.scroll = self.scroll.min(total.saturating_sub(LIST_LINES));
    }
    fn select_tab(&mut self, tab: usize) {
        if tab != self.tab {
            self.tab = tab;
            self.scroll = 0;
            self.capture = None;
            // Typed searches carry across devices; a pressed input names a
            // control on the old device only.
            if self.search.pressed.take().is_some() {
                self.search.text.clear();
            }
        }
    }
    fn nearest(&self, line: usize, column: usize) -> usize {
        *self
            .columns(line)
            .iter()
            .min_by_key(|c| (**c as i32 - column as i32).abs())
            .unwrap_or(&0)
    }
    fn move_focus(&mut self, key: &str) -> ResultAction {
        let tabs = self.tabs().len();
        let lines = self.lines().len();
        let focusable: Vec<usize> = self
            .settings()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.focusable())
            .map(|(i, _)| i)
            .collect();
        let into_list = if lines > 0 {
            Focus::Cell(self.scroll, 0)
        } else {
            Focus::Footer(0)
        };
        let settings_end = focusable
            .last()
            .map_or(Focus::Tab(self.tab), |s| Focus::Setting(*s));
        let (below_settings, above_list) = if self.search_shown() {
            (Focus::Search(SEARCH_FIELD), Focus::Search(SEARCH_FIELD))
        } else {
            (into_list, settings_end)
        };
        self.focus = match (self.focus, key) {
            (Focus::Tab(i), "ArrowUp") => Focus::Tab((i + tabs - 1) % tabs),
            (Focus::Tab(i), "ArrowDown") => Focus::Tab((i + 1) % tabs),
            (Focus::Tab(_), "ArrowRight") => focusable
                .first()
                .map_or(below_settings, |s| Focus::Setting(*s)),
            (Focus::Setting(s), "ArrowUp") => focusable
                .iter()
                .rev()
                .find(|i| **i < s)
                .map_or(Focus::Tab(self.tab), |i| Focus::Setting(*i)),
            (Focus::Setting(s), "ArrowDown") => focusable
                .iter()
                .find(|i| **i > s)
                .map_or(below_settings, |i| Focus::Setting(*i)),
            (Focus::Search(_), "ArrowUp") => settings_end,
            (Focus::Search(_), "ArrowDown") => into_list,
            (Focus::Search(0), "ArrowLeft") => Focus::Tab(self.tab),
            (Focus::Search(i), "ArrowLeft") => Focus::Search(i - 1),
            (Focus::Search(i), "ArrowRight") => Focus::Search((i + 1).min(SEARCH.len() - 1)),
            (Focus::Cell(l, c), "ArrowUp") if l > 0 => Focus::Cell(l - 1, self.nearest(l - 1, c)),
            (Focus::Cell(_, _), "ArrowUp") => above_list,
            (Focus::Cell(l, c), "ArrowDown") if l + 1 < lines => {
                Focus::Cell(l + 1, self.nearest(l + 1, c))
            }
            (Focus::Cell(_, _), "ArrowDown") => Focus::Footer(0),
            (Focus::Cell(l, c), "ArrowLeft") => self
                .columns(l)
                .iter()
                .rev()
                .find(|x| **x < c)
                .map_or(Focus::Tab(self.tab), |x| Focus::Cell(l, *x)),
            (Focus::Cell(l, c), "ArrowRight") => {
                Focus::Cell(l, *self.columns(l).iter().find(|x| **x > c).unwrap_or(&c))
            }
            (Focus::Footer(_), "ArrowUp") if lines > 0 => Focus::Cell(lines - 1, 0),
            (Focus::Footer(_), "ArrowUp") => above_list,
            (Focus::Footer(i), "ArrowLeft") => Focus::Footer(i.saturating_sub(1)),
            (Focus::Footer(i), "ArrowRight") => Focus::Footer((i + 1).min(FOOTER.len() - 1)),
            (focus, _) => focus,
        };
        if let Focus::Tab(i) = self.focus {
            self.select_tab(i);
        }
        self.keep_visible();
        // Moving focus redraws without the click sound activation makes.
        ResultAction::None
    }
    fn activate(&mut self, hit: Hit) -> ResultAction {
        match hit {
            Hit::Tab(i) => {
                self.select_tab(i);
                self.focus = Focus::Tab(i);
                ResultAction::Changed
            }
            Hit::Setting(i, delta) => {
                self.focus = Focus::Setting(i);
                match self.settings().get(i) {
                    Some(s) => self.adjust(*s, delta),
                    None => ResultAction::None,
                }
            }
            Hit::Search(i) => {
                self.focus = Focus::Search(i);
                match i {
                    0 | 1 => {
                        let find = if i == 0 { Find::Actions } else { Find::Inputs };
                        if find != self.search.find {
                            self.search.find = find;
                            // A pressed input only means something as an input.
                            if self.search.pressed.take().is_some() {
                                self.search.text.clear();
                            }
                            self.searched();
                        }
                    }
                    SEARCH_FIELD => {}
                    3 => self.begin(Target::Find),
                    _ => return self.clear_search(),
                }
                ResultAction::Changed
            }
            Hit::Footer(0) => ResultAction::Save,
            Hit::Footer(1) => self.reset_device(),
            Hit::Footer(_) => ResultAction::Close,
            Hit::Cell(line, column) => {
                self.focus = Focus::Cell(line, column);
                match self.lines().get(line).copied() {
                    Some(Line::Header(group)) => {
                        if !self.collapsed.remove(&group) {
                            self.collapsed.insert(group);
                        }
                        self.keep_visible();
                        ResultAction::Changed
                    }
                    Some(Line::Other(i)) => {
                        let name = action_name(&self.profile.bindings[i].action);
                        self.profile.bindings.remove(i);
                        self.message = format!("Removed the {name} binding");
                        self.keep_visible();
                        self.changed()
                    }
                    Some(Line::Entry(i)) => match column {
                        0 | 1 => {
                            let entry = &ENTRIES[i];
                            if entry.fixed || !entry.bindable() {
                                self.message =
                                    format!("{} is built in and cannot be changed", entry.label);
                                return ResultAction::Changed;
                            }
                            // An empty primary is filled before the secondary.
                            let filled = self.slots(i, &self.current()).len();
                            self.begin(Target::Slot(i, column.min(filled)));
                            ResultAction::Changed
                        }
                        2 => self.axis_option(i, true, true),
                        3 => self.axis_option(i, false, true),
                        _ => self.clear_slot(i, None),
                    },
                    None => ResultAction::None,
                }
            }
        }
    }
    fn focused_hit(&self, delta: i32) -> Option<Hit> {
        Some(match self.focus {
            Focus::Tab(_) => return None,
            Focus::Setting(i) => Hit::Setting(i, delta),
            Focus::Cell(l, c) => Hit::Cell(l, c),
            Focus::Search(i) => Hit::Search(i),
            Focus::Footer(i) => Hit::Footer(i),
        })
    }
    /// Keyboard input, and controller menu actions mapped to arrow keys,
    /// Enter and Escape.
    pub fn key(&mut self, key: &str, shift: bool, ctrl: bool, alt: bool) -> ResultAction {
        if let Some(capture) = &self.capture {
            if key == "Escape" {
                self.capture = None;
                self.message = "Capture cancelled".into();
                return ResultAction::Changed;
            }
            let target = capture.target.clone();
            if matches!(target, Target::Modifier) {
                return ResultAction::None;
            }
            let capture_shift = key == "Shift"
                && matches!(target, Target::Slot(i, _) if ENTRIES[i].action == "drone-boost");
            if self.current() != Tab::Keyboard
                || key.is_empty()
                || (!capture_shift && matches!(key, "Shift" | "Control" | "Alt" | "Super"))
            {
                return ResultAction::None;
            }
            let control = format!(
                "{}{}{}{}",
                if ctrl { "Ctrl-" } else { "" },
                if alt { "Alt-" } else { "" },
                if shift && !capture_shift {
                    "Shift-"
                } else {
                    ""
                },
                key
            );
            let Target::Slot(index, slot) = target else {
                self.found(control);
                return ResultAction::Changed;
            };
            if catalog::PROTECTED_KEYS.contains(&control.as_str()) {
                self.message = format!(
                    "{} is reserved; press another key",
                    catalog::key_label(&control)
                );
                return ResultAction::Changed;
            }
            let binding = self.digital_binding(index, "keyboard", control);
            self.assign(index, slot, binding);
            return ResultAction::Changed;
        }
        if ctrl && key == "f" && self.search_shown() {
            self.focus = Focus::Search(SEARCH_FIELD);
            return ResultAction::None;
        }
        if self.typing() {
            match key {
                "Backspace" => {
                    if self.search.pressed.take().is_some() {
                        self.search.text.clear();
                    } else {
                        self.search.text.pop();
                    }
                    self.searched();
                    return ResultAction::None;
                }
                "Delete" => {
                    self.clear_search();
                    return ResultAction::None;
                }
                // Esc empties the search first, then backs out as usual.
                "Escape" if self.searching() => return self.clear_search(),
                "Enter" => return self.move_focus("ArrowDown"),
                _ => {}
            }
        }
        match key {
            "Escape" => ResultAction::Close,
            "ArrowLeft" | "ArrowRight" if matches!(self.focus, Focus::Setting(_)) => {
                if let Focus::Setting(i) = self.focus
                    && self
                        .settings()
                        .get(i)
                        .is_some_and(|s| *s == Setting::Modifiers)
                {
                    return ResultAction::None;
                }
                let delta = if key == "ArrowLeft" { -1 } else { 1 };
                match self.focused_hit(delta) {
                    Some(hit) => self.activate(hit),
                    None => ResultAction::None,
                }
            }
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => self.move_focus(key),
            "Tab" => {
                let n = self.tabs().len();
                let next = if shift {
                    (self.tab + n - 1) % n
                } else {
                    (self.tab + 1) % n
                };
                self.select_tab(next);
                self.focus = Focus::Tab(next);
                ResultAction::None
            }
            "PageUp" | "PageDown" => {
                let lines = self.lines().len();
                if lines == 0 {
                    return ResultAction::None;
                }
                let at = match self.focus {
                    Focus::Cell(l, _) => l,
                    _ => self.scroll,
                };
                let target = if key == "PageUp" {
                    at.saturating_sub(LIST_LINES)
                } else {
                    (at + LIST_LINES).min(lines - 1)
                };
                self.focus = Focus::Cell(target, 0);
                self.keep_visible();
                ResultAction::None
            }
            "Delete" | "Backspace" => match self.focus {
                Focus::Cell(l, c) if c < 2 => match self.lines().get(l) {
                    Some(Line::Entry(i)) => self.clear_slot(*i, Some(c)),
                    _ => ResultAction::None,
                },
                _ => ResultAction::None,
            },
            "Enter" | "Space" => match self.focus {
                Focus::Tab(_) => self.move_focus("ArrowRight"),
                _ => match self.focused_hit(1) {
                    Some(hit) => self.activate(hit),
                    None => ResultAction::None,
                },
            },
            _ => ResultAction::None,
        }
    }
    /// A mouse button other than the left one, or a wheel notch, while the
    /// screen captures on the Mouse tab.
    pub fn mouse(&mut self, control: &str) -> ResultAction {
        if self.current() != Tab::Mouse {
            return ResultAction::None;
        }
        let (index, slot) = match &self.capture {
            Some(Capture {
                target: Target::Slot(index, slot),
                ..
            }) => (*index, *slot),
            Some(Capture {
                target: Target::Find,
                ..
            }) => {
                self.found(control.into());
                return ResultAction::Changed;
            }
            _ => return ResultAction::None,
        };
        if control == "button:right" && self.profile.mouse_look && !ENTRIES[index].replay_only() {
            self.message = "The right button is mouse look; turn mouse look off to bind it".into();
            return ResultAction::Changed;
        }
        let binding = self.digital_binding(index, "mouse", control.into());
        self.assign(index, slot, binding);
        ResultAction::Changed
    }
    pub fn wheel(&mut self, notches: i32) -> ResultAction {
        if self.capture.is_some() {
            return self.mouse(if notches > 0 {
                "wheel:up"
            } else {
                "wheel:down"
            });
        }
        let max = self.lines().len().saturating_sub(LIST_LINES);
        self.scroll = (self.scroll as i32 - notches * 3).clamp(0, max as i32) as usize;
        // Another control may now rest under the pointer.
        self.moved(self.hover.point);
        ResultAction::None
    }
    /// Native samples: tracks held modifiers and completes a device capture.
    /// Returns true when the draft changed.
    pub fn observe(&mut self, event: &Event) -> bool {
        let previous = self
            .held
            .insert((event.device.clone(), event.control.clone()), event.value);
        let Some(capture) = &self.capture else {
            return false;
        };
        let Tab::Device(id) = self.current() else {
            return false;
        };
        if event.baseline || event.device != id {
            return false;
        }
        let Some(device) = self.device(&id).cloned() else {
            return false;
        };
        let Some(control) = device.controls.iter().find(|c| c.id == event.control) else {
            return false;
        };
        let rest = capture
            .rest
            .get(&control.id)
            .copied()
            .unwrap_or(control.value);
        let target = capture.target.clone();
        let hat = is_hat(control);
        // What this sample presses, as a button token, if anything.
        let pressed: Option<String> = match control.kind {
            Kind::Button if event.value > 0.5 && previous.is_none_or(|v| v <= 0.5) => {
                Some(control.id.clone())
            }
            _ if hat && event.value != 0. && event.value != rest => {
                Some(format!("{}={}", control.id, event.value as i32))
            }
            _ => None,
        };
        let tab = self.current();
        let reference = self.reference(&id);
        let declared: Vec<String> = self
            .modifiers(&tab)
            .into_iter()
            .map(|i| self.profile.modifiers[i].1.clone())
            .collect();
        if target == Target::Find {
            let span = control.max - control.min;
            let moved = matches!(control.kind, Kind::Axis)
                && !hat
                && (event.value - rest).abs() > span * 0.3;
            let Some(token) = pressed.or_else(|| moved.then(|| control.id.clone())) else {
                return false;
            };
            self.found(token);
            return true;
        }
        if target == Target::Modifier {
            let Some(token) = pressed else {
                return false;
            };
            let name = catalog::control_label("pad", &token, Mode::Press, self.gamepad_tab(&tab));
            if let Some(i) = self
                .modifiers(&tab)
                .into_iter()
                .find(|i| self.profile.modifiers[*i].1 == token)
            {
                self.profile.modifiers.remove(i);
                self.message = format!("{name} is an ordinary button again.");
            } else if declared.len() >= 4 {
                self.message =
                    "A device can have four modifier buttons; press one to remove it".into();
                return true;
            } else {
                self.profile.modifiers.push((reference, token));
                self.message =
                    format!("{name} is now a modifier. Hold it while capturing to combine.");
            }
            self.capture = None;
            self.dirty = true;
            return true;
        }
        let Target::Slot(index, slot) = target else {
            return false;
        };
        if pressed.as_ref().is_some_and(|t| declared.contains(t)) {
            return false; // A modifier going down; wait for the control itself.
        }
        let held_mods: Vec<String> = declared
            .iter()
            .filter(|m| {
                let base = tore_input::token_base(m);
                let Some(c) = device.controls.iter().find(|c| c.id == base) else {
                    return false;
                };
                let raw = self
                    .held
                    .get(&(id.clone(), base.to_owned()))
                    .copied()
                    .unwrap_or(c.value);
                let value = norm(c, raw);
                tore_input::token_value(m, value).unwrap_or(value) != 0.
            })
            .take(2)
            .cloned()
            .collect();
        let chord = |base: &str| {
            let mut parts = held_mods.clone();
            parts.push(base.to_owned());
            parts.join("+")
        };
        let entry = &ENTRIES[index];
        let span = control.max - control.min;
        let moved =
            matches!(control.kind, Kind::Axis) && !hat && (event.value - rest).abs() > span * 0.3;
        let relative = matches!(control.kind, Kind::Relative) && event.value != 0.;
        // Triggers rest at one end of their travel; sticks rest centered.
        let trigger = matches!(control.kind, Kind::Axis) && norm(control, rest) < -0.8;
        let direction = (event.value - rest).signum();
        let mut calibration = Calibration {
            deadzone: self.deadzone(&tab),
            ..Calibration::default()
        };
        let sensitivity = self.sensitivity(&tab, false);
        let mut binding = Binding {
            device: reference,
            control: String::new(),
            action: entry.parsed().unwrap_or(Action::Ui("pause".into())),
            mode: Mode::Press,
            calibration,
            priority: 10,
        };
        match entry.kind {
            Row::Axis | Row::Lever | Row::Head => {
                if relative && entry.action == "throttle-rate" {
                    binding.mode = Mode::Delta;
                    binding.control = chord(&control.id);
                } else if moved {
                    binding.control = chord(&control.id);
                    binding.mode = if entry.kind == Row::Lever {
                        Mode::Unit
                    } else {
                        Mode::Axis
                    };
                    if entry.kind == Row::Head {
                        calibration.deadzone = 0.;
                    } else if entry.kind == Row::Axis {
                        // Pushing a stick forward to look or slew up reads negative.
                        let sign = if matches!(entry.action, "look-y" | "sight-y") {
                            -1.
                        } else {
                            1.
                        };
                        calibration.scale = sensitivity * sign;
                    }
                    binding.calibration = calibration;
                } else if pressed.is_some() || relative {
                    self.message = format!(
                        "{} needs an axis; put buttons on its direction rows",
                        entry.label
                    );
                    return true;
                } else {
                    return false;
                }
            }
            Row::Direction(sign) => {
                if let Some(token) = &pressed {
                    binding.control = chord(token);
                    binding.mode = Mode::Hold(sign);
                } else if moved && trigger {
                    binding.control = chord(&control.id);
                    binding.mode = Mode::Trigger(sign);
                    calibration.scale = self.sensitivity(&tab, true);
                    binding.calibration = calibration;
                } else if moved {
                    // A centered stick drives the whole axis row instead.
                    let Some(axis_row) = ENTRIES
                        .iter()
                        .position(|e| e.action == entry.action && e.kind == Row::Axis)
                    else {
                        return false;
                    };
                    binding.control = chord(&control.id);
                    binding.mode = Mode::Axis;
                    calibration.scale = sensitivity * sign * direction;
                    binding.calibration = calibration;
                    let slot = self.slots(axis_row, &tab).len().min(1);
                    self.assign(axis_row, slot, binding);
                    return true;
                } else {
                    return false;
                }
            }
            Row::Command | Row::Hold => {
                let mode = if entry.kind == Row::Hold {
                    Mode::HoldState
                } else {
                    Mode::Press
                };
                if let Some(token) = &pressed {
                    if hat && entry.kind == Row::Command {
                        binding.control = chord(&control.id);
                        binding.mode = Mode::Position(event.value as i32);
                    } else {
                        binding.control = chord(token);
                        binding.mode = mode;
                    }
                } else if relative && entry.kind == Row::Command {
                    binding.control = chord(&control.id);
                    binding.mode = Mode::Delta;
                    binding.calibration.scale = direction;
                } else if moved {
                    let threshold = if trigger {
                        format!("{}>0", control.id)
                    } else if direction > 0. {
                        format!("{}>0.5", control.id)
                    } else {
                        format!("{}<-0.5", control.id)
                    };
                    binding.control = chord(&threshold);
                    binding.mode = mode;
                } else {
                    return false;
                }
            }
        }
        self.assign(index, slot, binding);
        true
    }

    // ---- pointer --------------------------------------------------------

    fn tab_rect(i: usize) -> Rect {
        (
            LEFT.0 + 2,
            TAB_TOP + i as i32 * TAB_HEIGHT,
            LEFT.2 - 4,
            TAB_HEIGHT - 2,
        )
    }
    fn setting_rect(i: usize) -> Rect {
        (
            RIGHT_X + 4,
            SETTINGS_TOP + i as i32 * SETTING_HEIGHT,
            RIGHT_W - 8,
            SETTING_HEIGHT - 1,
        )
    }
    fn line_rect(row: usize) -> Rect {
        (
            RIGHT_X + 2,
            LIST_TOP + row as i32 * LINE_HEIGHT,
            RIGHT_W - 6,
            LINE_HEIGHT - 1,
        )
    }
    fn cell_rect(row: usize, column: usize) -> Rect {
        let (x, w) = COLUMNS[column];
        (
            RIGHT_X + x,
            LIST_TOP + row as i32 * LINE_HEIGHT,
            w,
            LINE_HEIGHT - 1,
        )
    }
    fn search_rect(i: usize) -> Rect {
        let (x, w) = SEARCH[i];
        (RIGHT_X + x, SEARCH_TOP, w, LINE_HEIGHT)
    }
    fn value_rect(r: Rect) -> Rect {
        (r.0 + 234, r.1 + 1, 220, r.3 - 2)
    }
    fn footer_rect(i: usize) -> Rect {
        (RIGHT_X + 150 + i as i32 * 108, FOOTER_Y, 100, 18)
    }
    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        for i in 0..self.tabs().len() {
            if inside(p, Self::tab_rect(i)) {
                return Some(Hit::Tab(i));
            }
        }
        for (i, s) in self.settings().iter().enumerate() {
            let r = Self::setting_rect(i);
            if s.focusable() && inside(p, r) {
                // The left third of the value box steps down; the rest steps up.
                let v = Self::value_rect(r);
                let delta = if s.arrows() && p.0 < (v.0 + v.2 / 3) as f64 {
                    -1
                } else {
                    1
                };
                return Some(Hit::Setting(i, delta));
            }
        }
        if self.search_shown()
            && let Some(i) = (0..SEARCH.len()).find(|i| inside(p, Self::search_rect(*i)))
        {
            return Some(Hit::Search(i));
        }
        let lines = self.lines();
        for row in 0..LIST_LINES {
            let line = self.scroll + row;
            let Some(kind) = lines.get(line) else {
                break;
            };
            if !inside(p, Self::line_rect(row)) {
                continue;
            }
            if matches!(kind, Line::Header(_)) {
                return Some(Hit::Cell(line, 0));
            }
            return self
                .columns(line)
                .into_iter()
                .find(|c| inside(p, Self::cell_rect(row, *c)))
                .map(|c| Hit::Cell(line, c));
        }
        (0..FOOTER.len())
            .find(|i| inside(p, Self::footer_rect(*i)))
            .map(Hit::Footer)
    }
    pub fn pointer(&mut self, point: Option<(f64, f64)>, down: bool) -> ResultAction {
        // A click starts the tooltip's wait over.
        self.hover.since = Some(Instant::now());
        let hit = point.and_then(|p| self.hit(p));
        if down {
            self.pressed = hit;
            return ResultAction::None;
        }
        let pressed = self.pressed.take();
        if pressed != hit || self.capture.is_some() {
            return ResultAction::None;
        }
        hit.map_or(ResultAction::None, |h| self.activate(h))
    }
    /// The right button: it turns a value the left button turns forward back
    /// one (a click and a release on the same control, as the left button
    /// needs), wrapping at both ends. It does nothing on a control that does
    /// not cycle, and while an input is being captured the host sends it to
    /// [`Self::mouse`] instead.
    pub fn right_pointer(&mut self, point: Option<(f64, f64)>, down: bool) -> ResultAction {
        self.hover.since = Some(Instant::now());
        let hit = point.and_then(|p| self.hit(p));
        if down {
            self.pressed = None;
            self.right_pressed = hit;
            return ResultAction::None;
        }
        let pressed = self.right_pressed.take();
        if pressed != hit || self.capture.is_some() {
            return ResultAction::None;
        }
        hit.map_or(ResultAction::None, |h| self.turn_back(h))
    }
    /// What a right click on `hit` does: the previous value of a setting row
    /// (a toggle flips), of the response curve, or the invert box flipped.
    fn turn_back(&mut self, hit: Hit) -> ResultAction {
        match hit {
            Hit::Setting(i, _) => {
                let Some(setting) = self.settings().get(i).copied() else {
                    return ResultAction::None;
                };
                // Modifiers starts a capture, which has no direction.
                if setting == Setting::Modifiers {
                    return ResultAction::None;
                }
                self.focus = Focus::Setting(i);
                self.adjust(setting, -1)
            }
            Hit::Cell(line, column @ (2 | 3)) => {
                let Some(Line::Entry(i)) = self.lines().get(line).copied() else {
                    return ResultAction::None;
                };
                self.focus = Focus::Cell(line, column);
                self.axis_option(i, column == 2, false)
            }
            _ => ResultAction::None,
        }
    }

    // ---- tooltips -------------------------------------------------------

    /// The tooltip for the control under the pointer, if it has one.
    fn tip(&self, hit: Hit) -> Option<&'static str> {
        match hit {
            Hit::Cell(line, 2) => {
                Some(INVERT_TIP).filter(|_| matches!(self.lines().get(line), Some(Line::Entry(_))))
            }
            _ => None,
        }
    }
    /// The pointer moved to `point` (the 640x480 layer) or left the window.
    /// True when the picture changes: a tooltip that was showing goes away, or
    /// the pointer came to rest on a control that has one (whose delay the
    /// host must keep redrawing for, see [`Self::tooltip_pending`]).
    pub fn moved(&mut self, point: Option<(f64, f64)>) -> bool {
        self.moved_at(point, Instant::now())
    }
    fn moved_at(&mut self, point: Option<(f64, f64)>, now: Instant) -> bool {
        let before = self.tooltip_shown(now);
        let hit = point.and_then(|p| self.hit(p));
        self.hover.point = point;
        if hit != self.hover.hit {
            self.hover.hit = hit;
            self.hover.since = Some(now);
            return before.is_some() || hit.and_then(|h| self.tip(h)).is_some();
        }
        false
    }
    /// Whether the pointer rests on a control with a tooltip that is not
    /// showing yet, so the host redraws until it does.
    pub fn tooltip_pending(&self) -> bool {
        self.tooltip_shown(Instant::now()).is_none()
            && self.hover.hit.and_then(|h| self.tip(h)).is_some()
            && self.pressed.is_none()
    }
    /// The tooltip showing at `now` and the control it belongs to.
    fn tooltip_shown(&self, now: Instant) -> Option<(&'static str, Hit)> {
        let hit = self.hover.hit?;
        let since = self.hover.since?;
        // A press, a capture or a scroll since the pointer last moved hides it.
        if self.pressed.is_some() || self.capture.is_some() {
            return None;
        }
        if self.hit(self.hover.point?) != Some(hit) {
            return None;
        }
        let tip = self.tip(hit)?;
        (now.saturating_duration_since(since) >= TOOLTIP_DELAY).then_some((tip, hit))
    }
    /// The tooltip's box: below its control and as far right as the screen
    /// allows, or above when there is no room below, never over the control.
    fn tooltip_rect(font: &Font, text: &str, control: Rect) -> Rect {
        let w = text_width(font, text) + 8;
        let h = 14;
        let x = (control.0 + control.2 - w).clamp(2, 638 - w);
        let below = control.1 + control.3 + 2;
        let y = if below + h <= 436 {
            below
        } else {
            control.1 - 2 - h
        };
        (x, y, w, h)
    }
    fn draw_tooltip(&self, pixels: &mut [u8], font: &Font, now: Instant) {
        let Some((text, hit)) = self.tooltip_shown(now) else {
            return;
        };
        let Hit::Cell(line, column) = hit else {
            return;
        };
        let Some(row) = line.checked_sub(self.scroll).filter(|r| *r < LIST_LINES) else {
            return;
        };
        let r = Self::tooltip_rect(font, text, Self::cell_rect(row, column));
        Canvas(pixels).rect(r, TIP_FILL);
        Canvas(pixels).outline(r, TIP_EDGE);
        Self::text(pixels, font, r, TIP_TEXT, text, (r.0 + 4, r.1 + 3));
    }

    // ---- drawing --------------------------------------------------------

    pub(crate) fn text(
        pixels: &mut [u8],
        font: &Font,
        clip: Rect,
        color: [u8; 4],
        text: &str,
        at: (i32, i32),
    ) {
        Paint {
            pixels,
            clip,
            color,
        }
        .text(font, text, at.0, at.1);
    }
    fn button(pixels: &mut [u8], font: &Font, r: Rect, label: &str, focused: bool) {
        Canvas(pixels).rect(r, if focused { FOCUS } else { BUTTON });
        let x = r.0 + (r.2 - text_width(font, label)) / 2;
        Self::text(pixels, font, r, WHITE, label, (x, r.1 + 2));
    }
    pub fn draw(&self, pixels: &mut [u8], font: &Font) {
        title_bar(
            pixels,
            font,
            "INPUT CONFIGURATION   |   Control mappings",
            self.context,
        );
        self.draw_tabs(pixels, font);
        self.draw_settings(pixels, font);
        self.draw_mappings(pixels, font);
        let message = if self.dirty && self.capture.is_none() {
            format!("{}  (not applied)", self.message)
        } else {
            self.message.clone()
        };
        status_bar(pixels, font, &message);
        for (i, label) in FOOTER.iter().enumerate() {
            footer_button(
                pixels,
                font,
                Self::footer_rect(i),
                label,
                self.focus == Focus::Footer(i),
            );
        }
        self.draw_tooltip(pixels, font, Instant::now());
    }
    fn draw_tabs(&self, pixels: &mut [u8], font: &Font) {
        Canvas(pixels).rect(LEFT, PANEL);
        Self::text(
            pixels,
            font,
            LEFT,
            TITLE,
            "INPUT DEVICES",
            (LEFT.0 + 6, LEFT.1 + 5),
        );
        for (i, tab) in self.tabs().iter().enumerate() {
            let r = Self::tab_rect(i);
            if r.1 + r.3 > LEFT.1 + LEFT.3 {
                break;
            }
            let fill = if self.focus == Focus::Tab(i) {
                FOCUS
            } else if i == self.tab {
                [52, 74, 100, 255]
            } else {
                PAPER
            };
            Canvas(pixels).rect(r, fill);
            if i == self.tab {
                Canvas(pixels).rect((r.0, r.1, 3, r.3), GOOD);
            }
            let (title, subtitle, live) = match tab {
                Tab::Keyboard => ("KEYBOARD".to_owned(), "Standard keyboard".to_owned(), true),
                Tab::Mouse => ("MOUSE".into(), "Look and buttons".into(), true),
                Tab::Head => (
                    "HEAD TRACKER".into(),
                    if self.head_status.starts_with("Receiving") {
                        "Receiving".into()
                    } else {
                        self.profile
                            .head_port
                            .map_or("Off".into(), |p| format!("opentrack UDP {p}"))
                    },
                    self.head_status.starts_with("Receiving"),
                ),
                Tab::Device(id) => match self.device(id) {
                    Some(d) => (
                        classify(d).title().to_ascii_uppercase(),
                        d.name.clone(),
                        true,
                    ),
                    None => ("DEVICE".into(), "Not connected".into(), false),
                },
            };
            let color = if live { WHITE } else { MUTED };
            Self::text(pixels, font, r, color, &title, (r.0 + 8, r.1 + 5));
            Self::text(
                pixels,
                font,
                r,
                MUTED,
                &fit(font, &subtitle, r.2 - 12),
                (r.0 + 8, r.1 + 18),
            );
        }
    }
    fn draw_settings(&self, pixels: &mut [u8], font: &Font) {
        let tab = self.current();
        let panel = (RIGHT_X, 24, RIGHT_W, 116);
        Canvas(pixels).rect(panel, PANEL);
        let (title, device, live) = match &tab {
            Tab::Keyboard => ("KEYBOARD SETTINGS".to_owned(), String::new(), true),
            Tab::Mouse => ("MOUSE SETTINGS".into(), String::new(), true),
            Tab::Head => (
                "HEAD TRACKER SETTINGS".into(),
                if self.head_status.starts_with("Receiving") {
                    "CONNECTED".into()
                } else {
                    "NOT DETECTED".into()
                },
                self.head_status.starts_with("Receiving"),
            ),
            Tab::Device(id) => match self.device(id) {
                Some(d) => (
                    format!("{} SETTINGS", classify(d).title().to_ascii_uppercase()),
                    format!("{}   CONNECTED", d.name.to_ascii_uppercase()),
                    true,
                ),
                None => ("DEVICE SETTINGS".into(), "NOT CONNECTED".into(), false),
            },
        };
        Self::text(pixels, font, panel, TITLE, &title, (RIGHT_X + 6, 29));
        let device = fit(font, &device, 280);
        let x = RIGHT_X + RIGHT_W - 6 - text_width(font, &device);
        Self::text(
            pixels,
            font,
            panel,
            if live { GOOD } else { MUTED },
            &device,
            (x, 29),
        );
        for (i, setting) in self.settings().iter().enumerate() {
            let r = Self::setting_rect(i);
            if self.focus == Focus::Setting(i) {
                Canvas(pixels).rect(r, FOCUS);
            }
            let (label, value) = self.setting_text(*setting);
            if !setting.focusable() {
                let text = if value.is_empty() {
                    label
                } else {
                    format!("{label}: {value}")
                };
                Self::text(
                    pixels,
                    font,
                    r,
                    MUTED,
                    &fit(font, &text, r.2 - 12),
                    (r.0 + 6, r.1 + 3),
                );
                continue;
            }
            Self::text(pixels, font, r, WHITE, &label, (r.0 + 6, r.1 + 3));
            let v = Self::value_rect(r);
            Canvas(pixels).outline(v, [110, 130, 156, 255]);
            let value = if !setting.arrows() {
                fit(font, &value, 200)
            } else {
                format!("<   {}   >", fit(font, &value, 170))
            };
            let x = v.0 + (v.2 - text_width(font, &value)) / 2;
            Self::text(pixels, font, v, WHITE, &value, (x, r.1 + 3));
        }
    }
    fn draw_mappings(&self, pixels: &mut [u8], font: &Font) {
        let tab = self.current();
        let panel = (RIGHT_X, MAP_TOP, RIGHT_W, 292);
        Canvas(pixels).rect(panel, PANEL);
        Self::text(
            pixels,
            font,
            panel,
            TITLE,
            "CONTROL MAPPINGS",
            (RIGHT_X + 6, MAP_TOP + 4),
        );
        if tab == Tab::Head {
            for (i, text) in [
                "Head tracking turns the view like the right stick or mouse look, and",
                "keeps the cockpit limit: you cannot look below the forward eye line.",
                "Center view (Shift+/) makes your current head position straight ahead.",
                "",
                "TrackIR's own software only feeds games registered with NaturalPoint.",
                "Use opentrack instead: input TrackIR (or a webcam or phone tracker),",
                "output 'UDP over network' to 127.0.0.1 port 4242.",
                "",
                "Trackers that appear as a joystick can bind Head tracker yaw and",
                "pitch on that device's tab instead.",
            ]
            .iter()
            .enumerate()
            {
                Self::text(
                    pixels,
                    font,
                    panel,
                    WHITE,
                    text,
                    (RIGHT_X + 10, MAP_TOP + 24 + i as i32 * 15),
                );
            }
            return;
        }
        let lines = self.lines();
        let count = self
            .profile
            .bindings
            .iter()
            .filter(|b| self.on_tab(b, &tab))
            .count();
        let summary = if self.searching() {
            match lines
                .iter()
                .filter(|l| !matches!(l, Line::Header(_)))
                .count()
            {
                1 => "1 match".to_owned(),
                n => format!("{n} matches"),
            }
        } else {
            match tab {
                Tab::Keyboard | Tab::Mouse => format!("{count} custom"),
                _ => format!("{count} bindings"),
            }
        };
        let x = RIGHT_X + RIGHT_W - 6 - text_width(font, &summary);
        Self::text(pixels, font, panel, MUTED, &summary, (x, MAP_TOP + 4));
        self.draw_search(pixels, font, &tab);
        if lines.is_empty() && self.searching() {
            let what = match self.search.find {
                Find::Actions => "No actions match",
                Find::Inputs => "Nothing on this device is bound to",
            };
            let text = format!("{what} \"{}\"", self.search.text.trim());
            Self::text(
                pixels,
                font,
                panel,
                MUTED,
                &fit(font, &text, RIGHT_W - 20),
                (RIGHT_X + 10, LIST_TOP + 4),
            );
        }
        let header = (RIGHT_X + 2, LIST_TOP - 16, RIGHT_W - 4, 14);
        Canvas(pixels).rect(header, HEADER);
        let (first, second) = if tab == Tab::Keyboard {
            ("PRIMARY KEY", "SECONDARY KEY")
        } else {
            ("PRIMARY INPUT", "SECONDARY INPUT")
        };
        for (text, x) in [
            ("ACTION", 6),
            (first, COLUMNS[0].0),
            (second, COLUMNS[1].0),
            ("OPTIONS", COLUMNS[2].0),
        ] {
            Self::text(
                pixels,
                font,
                header,
                TITLE,
                text,
                (RIGHT_X + x, header.1 + 3),
            );
        }
        for row in 0..LIST_LINES {
            let line = self.scroll + row;
            let Some(kind) = lines.get(line) else {
                break;
            };
            let r = Self::line_rect(row);
            match kind {
                Line::Header(group) => {
                    Canvas(pixels).rect(r, GROUP);
                    if matches!(self.focus, Focus::Cell(l, _) if l == line) {
                        Canvas(pixels).outline(r, WHITE);
                    }
                    let open = !self.collapsed.contains(group);
                    let count = lines
                        .iter()
                        .skip(line + 1)
                        .take_while(|l| !matches!(l, Line::Header(_)))
                        .count();
                    let title = group
                        .map_or("Other bindings", Group::title)
                        .to_ascii_uppercase();
                    // A small drawn triangle: down when open, right when collapsed.
                    for i in 0..4 {
                        let rect = if open {
                            (r.0 + 5 + i, r.1 + 4 + i, 7 - 2 * i, 1)
                        } else {
                            (r.0 + 6 + i, r.1 + 3 + i, 1, 7 - 2 * i)
                        };
                        Canvas(pixels).rect(rect, INK);
                    }
                    let text = if open {
                        format!("{title} ({count})")
                    } else {
                        title
                    };
                    Self::text(pixels, font, r, INK, &text, (r.0 + 18, r.1 + 2));
                }
                Line::Entry(i) => self.draw_entry(pixels, font, &tab, row, line, *i),
                Line::Other(i) => {
                    let b = &self.profile.bindings[*i];
                    Canvas(pixels).rect(
                        r,
                        if row.is_multiple_of(2) {
                            PALE
                        } else {
                            PALE_ALT
                        },
                    );
                    let name = fit(font, &action_name(&b.action), COLUMNS[0].0 - 10);
                    Self::text(pixels, font, r, INK, &name, (r.0 + 4, r.1 + 2));
                    let label = catalog::control_label(
                        &b.device,
                        &b.control,
                        b.mode,
                        self.gamepad_tab(&tab),
                    );
                    let cell = Self::cell_rect(row, 0);
                    let width = COLUMNS[1].0 + COLUMNS[1].1 - COLUMNS[0].0;
                    Self::text(
                        pixels,
                        font,
                        r,
                        INK,
                        &fit(font, &label, width),
                        (cell.0 + 2, cell.1 + 2),
                    );
                    let focused = self.focus == Focus::Cell(line, 4);
                    Self::button(pixels, font, Self::cell_rect(row, 4), "Clear", focused);
                }
            }
        }
        let total = lines.len();
        if total > LIST_LINES {
            let track = (
                RIGHT_X + RIGHT_W - 3,
                LIST_TOP,
                2,
                LIST_LINES as i32 * LINE_HEIGHT,
            );
            Canvas(pixels).rect(track, HEADER);
            let h = (track.3 * LIST_LINES as i32 / total as i32).max(8);
            let y = track.1 + (track.3 - h) * self.scroll as i32 / (total - LIST_LINES) as i32;
            Canvas(pixels).rect((track.0, y, 2, h), TITLE);
        }
    }
    fn draw_search(&self, pixels: &mut [u8], font: &Font, tab: &Tab) {
        let row = (RIGHT_X + 2, SEARCH_TOP, RIGHT_W - 4, LINE_HEIGHT);
        Self::text(
            pixels,
            font,
            row,
            TITLE,
            "FIND",
            (RIGHT_X + 6, SEARCH_TOP + 3),
        );
        let inputs = if *tab == Tab::Keyboard {
            "Keys"
        } else {
            "Inputs"
        };
        for (i, (label, find)) in [("Actions", Find::Actions), (inputs, Find::Inputs)]
            .into_iter()
            .enumerate()
        {
            let r = Self::search_rect(i);
            Self::button(pixels, font, r, label, self.focus == Focus::Search(i));
            if self.search.find == find {
                Canvas(pixels).rect((r.0, r.1 + r.3 - 2, r.2, 2), GOOD);
            }
        }
        let field = Self::search_rect(SEARCH_FIELD);
        let finding = matches!(
            self.capture,
            Some(Capture {
                target: Target::Find,
                ..
            })
        );
        let focused = self.focus == Focus::Search(SEARCH_FIELD);
        Canvas(pixels).rect(field, PALE);
        Canvas(pixels).outline(field, if focused { WHITE } else { BUTTON });
        let (text, color) = if finding {
            ("Press...".to_owned(), INK)
        } else if self.search.text.is_empty() {
            let hint = match (self.search.find, tab) {
                (Find::Actions, _) => "Type part of an action name",
                (Find::Inputs, Tab::Keyboard) => "Type a key name, or Press it",
                (Find::Inputs, _) => "Type an input name, or Press it",
            };
            (hint.to_owned(), [104, 116, 134, 255])
        } else {
            (self.search.text.clone(), INK)
        };
        let mut shown = fit(font, &text, field.2 - 12);
        if focused && !finding {
            if self.search.text.is_empty() {
                shown = format!("_ {shown}");
            } else {
                shown.push('_');
            }
        }
        Self::text(
            pixels,
            font,
            field,
            color,
            &shown,
            (field.0 + 4, field.1 + 3),
        );
        for (i, label) in [(3, "Press"), (4, "Clear")] {
            Self::button(
                pixels,
                font,
                Self::search_rect(i),
                label,
                self.focus == Focus::Search(i) || (i == 3 && finding),
            );
        }
    }
    fn draw_entry(
        &self,
        pixels: &mut [u8],
        font: &Font,
        tab: &Tab,
        row: usize,
        line: usize,
        index: usize,
    ) {
        let entry = &ENTRIES[index];
        let r = Self::line_rect(row);
        Canvas(pixels).rect(
            r,
            if row.is_multiple_of(2) {
                PALE
            } else {
                PALE_ALT
            },
        );
        let label = fit(font, entry.label, COLUMNS[0].0 - 10);
        Self::text(pixels, font, r, INK, &label, (r.0 + 4, r.1 + 2));
        let slots = self.slots(index, tab);
        let capturing = match &self.capture {
            Some(Capture {
                target: Target::Slot(i, s),
                ..
            }) if *i == index => Some(*s),
            _ => None,
        };
        let bindings: Vec<&Binding> = slots
            .iter()
            .filter_map(|s| match s {
                Source::Binding(i) => Some(&self.profile.bindings[*i]),
                Source::Stock(_) => None,
            })
            .collect();
        for column in self.columns(line) {
            let cell = Self::cell_rect(row, column);
            let focused = self.focus == Focus::Cell(line, column);
            match column {
                0 | 1 => {
                    let mut text = if capturing.is_some_and(|s| s.min(1) == column) {
                        "Press...".to_owned()
                    } else {
                        slots
                            .get(column)
                            .map_or("-".to_owned(), |s| self.source_label(s, tab))
                    };
                    if column == 1 && slots.len() > 2 {
                        text = format!("{text} +{}", slots.len() - 2);
                    }
                    let color = if focused {
                        Canvas(pixels).rect(cell, FOCUS);
                        WHITE
                    } else if entry.fixed {
                        MUTED
                    } else {
                        INK
                    };
                    Self::text(
                        pixels,
                        font,
                        cell,
                        color,
                        &fit(font, &text, cell.2 - 4),
                        (cell.0 + 2, cell.1 + 2),
                    );
                }
                2 => {
                    let inverted = bindings.iter().any(|b| b.calibration.scale < 0.);
                    let color = if focused { WHITE } else { INK };
                    if focused {
                        Canvas(pixels).rect(cell, FOCUS);
                    }
                    let tick = (cell.0 + 3, cell.1 + 2, 9, 9);
                    Canvas(pixels).outline(tick, color);
                    if inverted {
                        Canvas(pixels).rect((tick.0 + 2, tick.1 + 2, 5, 5), color);
                    }
                }
                3 => {
                    let curve = bindings.first().map(|b| b.calibration.curve);
                    let text = curve.map_or("curve".to_owned(), |c| format!("x{c:.1}"));
                    Self::button(pixels, font, cell, &text, focused);
                }
                _ => Self::button(pixels, font, cell, "Clear", focused),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pad() -> Device {
        let control = |id: &str, kind: Kind, min: f64, max: f64| tore_input_native::Control {
            id: id.into(),
            kind,
            min,
            max,
            value: if min == 0. && max > 1. {
                0.
            } else {
                (min + max) / 2.
            },
        };
        Device {
            id: "linux-pad".into(),
            name: "Test pad".into(),
            rumble: true,
            controls: vec![
                control("axis:0", Kind::Axis, -100., 100.),
                control("axis:1", Kind::Axis, -100., 100.),
                control("axis:5", Kind::Axis, 0., 255.),
                control("axis:16", Kind::Axis, -1., 1.),
                control("button:304", Kind::Button, 0., 1.),
                control("button:314", Kind::Button, 0., 1.),
            ],
        }
    }
    fn event(e: &mut Editor, control: &str, value: f64) -> bool {
        e.observe(&Event {
            device: "linux-pad".into(),
            control: control.into(),
            value,
            baseline: false,
        })
    }
    fn entry(label: &str) -> usize {
        ENTRIES.iter().position(|e| e.label == label).unwrap()
    }
    fn line(e: &Editor, label: &str) -> usize {
        let index = entry(label);
        e.lines()
            .iter()
            .position(|l| *l == Line::Entry(index))
            .unwrap()
    }
    #[test]
    fn opens_on_the_connected_device_with_grouped_rows() {
        let e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        assert_eq!(e.current(), Tab::Device("linux-pad".into()));
        assert_eq!(classify(&pad()), DeviceClass::Gamepad);
        assert!(matches!(e.lines()[0], Line::Header(Some(Group::Flight))));
        assert!(e.settings().contains(&Setting::Rumble));
        assert!(e.settings().contains(&Setting::Modifiers));
        assert_eq!(e.tabs().last(), Some(&Tab::Head));
    }
    #[test]
    fn drone_bindings_keep_flight_assignments_and_save_in_their_own_context() {
        let mut e = Editor::new(Profile::default(), vec![], "Replay paused");
        e.activate(Hit::Cell(line(&e, "Drone: move forward"), 0));
        e.key("g", false, false, false);
        assert!(
            e.profile
                .disabled
                .contains(&("replay-keyboard".into(), "w".into()))
        );
        assert!(!e.slots(entry("Landing gear"), &Tab::Keyboard).is_empty());
        assert_eq!(e.profile.bindings[0].device, "replay-keyboard");
        let saved = Profile::parse(&e.profile.to_text().unwrap()).unwrap();
        let mut controls = crate::replay::drone::Controls::new(&saved);
        assert!(!controls.key("w", true, Default::default()));
        assert!(controls.key("g", true, Default::default()));
        assert!(controls.held("drone-forward"));
        controls.key("g", false, Default::default());
        assert!(!controls.held("drone-forward"));
        // Reset restores both desktop contexts on this tab.
        e.reset_device();
        assert!(e.profile.bindings.is_empty() && e.profile.disabled.is_empty());
        e.activate(Hit::Cell(line(&e, "Drone: four times faster"), 0));
        e.key("Shift", true, false, false);
        assert!(!e.capturing());
        assert_eq!(
            e.slots(entry("Drone: four times faster"), &Tab::Keyboard)
                .len(),
            1
        );
    }

    #[test]
    fn keyboard_stock_key_moves_and_reports_the_previous_owner() {
        let mut e = Editor::new(Profile::default(), vec![], "Main menu");
        assert_eq!(e.current(), Tab::Keyboard);
        e.activate(Hit::Cell(line(&e, "Landing gear"), 0));
        assert!(e.capturing());
        e.key("f", false, false, false);
        // G is disabled for gear, F is taken from flaps and bound to gear.
        assert!(
            e.profile
                .disabled
                .contains(&("keyboard".into(), "g".into()))
        );
        assert!(
            e.profile
                .disabled
                .contains(&("keyboard".into(), "f".into()))
        );
        assert_eq!(e.profile.bindings.len(), 1);
        assert_eq!(e.profile.bindings[0].control, "f");
        assert!(e.message.contains("Flaps"), "{}", e.message);
        assert!(e.slots(entry("Flaps"), &Tab::Keyboard).is_empty());
        // Giving flaps back its own stock key re-enables it without a binding.
        e.activate(Hit::Cell(line(&e, "Flaps"), 0));
        e.key("f", false, false, false);
        assert!(
            !e.profile
                .disabled
                .contains(&("keyboard".into(), "f".into()))
        );
        assert!(e.profile.bindings.is_empty());
        assert!(e.profile.to_text().is_ok());
    }
    #[test]
    fn keyboard_combinations_and_protected_keys() {
        let mut e = Editor::new(Profile::default(), vec![], "Flight paused");
        e.activate(Hit::Cell(line(&e, "Pitch: nose down"), 1));
        e.key("w", true, true, false);
        let b = &e.profile.bindings[0];
        assert_eq!(
            (b.control.as_str(), b.mode, b.priority),
            ("Ctrl-Shift-w", Mode::Hold(-1.), 100)
        );
        // The stock Up arrow stays primary; the new key is secondary.
        assert_eq!(
            e.slots(entry("Pitch: nose down"), &Tab::Keyboard)[0],
            Source::Stock("ArrowUp")
        );
        e.activate(Hit::Cell(line(&e, "Landing gear"), 0));
        e.key("Enter", false, false, true);
        assert!(e.capturing(), "Alt+Enter is reserved");
        assert_eq!(e.key("Escape", false, false, false), ResultAction::Changed);
        assert!(!e.capturing());
        assert_eq!(e.key("Escape", false, false, false), ResultAction::Close);
    }
    #[test]
    fn gamepad_capture_uses_held_modifiers_and_dpad_directions() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        e.adjust(Setting::Modifiers, 1);
        event(&mut e, "button:314", 1.);
        event(&mut e, "button:314", 0.);
        e.adjust(Setting::Modifiers, 1);
        event(&mut e, "axis:16", -1.);
        event(&mut e, "axis:16", 0.);
        assert_eq!(e.profile.modifiers.len(), 2);
        e.activate(Hit::Cell(line(&e, "Flaps"), 0));
        event(&mut e, "button:314", 1.);
        event(&mut e, "axis:16", -1.);
        assert!(e.capturing(), "modifiers alone do not complete a capture");
        event(&mut e, "button:304", 1.);
        assert_eq!(
            e.profile.bindings[0].control,
            "button:314+axis:16=-1+button:304"
        );
        assert!(e.profile.to_text().is_ok());
        assert_eq!(
            e.source_label(&Source::Binding(0), &e.current()),
            "View + D-pad left + A"
        );
    }
    #[test]
    fn axes_triggers_and_device_settings() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        e.activate(Hit::Cell(line(&e, "Roll (bank left/right)"), 0));
        event(&mut e, "axis:0", 90.);
        e.activate(Hit::Cell(line(&e, "Rudder right"), 0));
        event(&mut e, "axis:5", 250.);
        event(&mut e, "axis:5", 0.);
        e.activate(Hit::Cell(line(&e, "Fire / release weapon"), 0));
        event(&mut e, "axis:5", 250.);
        let modes: Vec<_> = e
            .profile
            .bindings
            .iter()
            .map(|b| (b.control.clone(), b.mode))
            .collect();
        assert_eq!(
            modes,
            [
                ("axis:0".into(), Mode::Axis),
                ("axis:5".into(), Mode::Trigger(1.)),
                ("axis:5>0".into(), Mode::HoldState)
            ]
        );
        e.adjust(Setting::StickSensitivity, -1);
        assert!((e.profile.bindings[0].calibration.scale - 0.95).abs() < 1e-9);
        let roll = line(&e, "Roll (bank left/right)");
        e.activate(Hit::Cell(roll, 2));
        assert!(e.profile.bindings[0].calibration.scale < 0.);
        assert!(e.profile.to_text().is_ok());
        e.activate(Hit::Cell(roll, 4));
        assert_eq!(e.profile.bindings.len(), 2);
    }
    #[test]
    fn modifiers_start_only_on_activation_and_toggle_in_capture() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        let row = e
            .settings()
            .iter()
            .position(|s| *s == Setting::Modifiers)
            .unwrap();
        e.focus = Focus::Setting(row);
        // D-pad left/right arrive as arrow keys and must not start a capture.
        assert_eq!(e.key("ArrowRight", false, false, false), ResultAction::None);
        assert_eq!(e.key("ArrowLeft", false, false, false), ResultAction::None);
        assert!(!e.capturing());
        // A arrives as Enter.
        e.key("Enter", false, false, false);
        assert!(e.capturing());
        event(&mut e, "axis:16", 1.);
        event(&mut e, "axis:16", 0.);
        assert_eq!(
            e.profile.modifiers,
            [("linux-pad".into(), "axis:16=1".into())]
        );
        // Pressing an existing modifier in the next capture removes it.
        e.key("Enter", false, false, false);
        event(&mut e, "axis:16", 1.);
        assert!(e.profile.modifiers.is_empty());
        assert!(!e.capturing());
    }
    #[test]
    fn replacing_a_primary_keeps_it_primary() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        let flaps = line(&e, "Flaps");
        e.activate(Hit::Cell(flaps, 0));
        event(&mut e, "button:304", 1.);
        event(&mut e, "button:304", 0.);
        e.activate(Hit::Cell(flaps, 1));
        event(&mut e, "button:314", 1.);
        event(&mut e, "button:314", 0.);
        e.activate(Hit::Cell(flaps, 0));
        event(&mut e, "axis:16", -1.);
        let tab = e.current();
        let labels: Vec<_> = e
            .slots(entry("Flaps"), &tab)
            .iter()
            .map(|s| e.source_label(s, &tab))
            .collect();
        assert_eq!(labels, ["D-pad left", "View"]);
    }
    #[test]
    fn keyboard_navigation_reaches_every_region() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        assert_eq!(e.focus, Focus::Tab(2));
        e.key("ArrowRight", false, false, false);
        assert!(matches!(e.focus, Focus::Setting(_)));
        for _ in 0..8 {
            e.key("ArrowDown", false, false, false);
        }
        assert!(matches!(e.focus, Focus::Cell(_, _)));
        e.key("PageDown", false, false, false);
        e.key("PageDown", false, false, false);
        assert!(e.scroll > 0);
        for _ in 0..400 {
            e.key("ArrowDown", false, false, false);
        }
        assert_eq!(e.focus, Focus::Footer(0));
        assert_eq!(e.key("Enter", false, false, false), ResultAction::Save);
    }
    fn found(e: &Editor) -> Vec<&'static str> {
        e.lines()
            .iter()
            .filter_map(|l| match l {
                Line::Entry(i) => Some(ENTRIES[*i].label),
                _ => None,
            })
            .collect()
    }
    fn type_search(e: &mut Editor, text: &str) {
        e.focus = Focus::Search(SEARCH_FIELD);
        assert_eq!(e.text_input(text), ResultAction::None, "typing is silent");
    }
    #[test]
    fn searching_actions_opens_folded_groups_and_esc_clears_first() {
        let mut e = Editor::new(Profile::default(), vec![], "Main menu");
        let all = e.lines().len();
        let gear = line(&e, "Landing gear");
        let header = (0..gear)
            .rev()
            .find(|l| matches!(e.lines()[*l], Line::Header(_)))
            .unwrap();
        e.activate(Hit::Cell(header, 0));
        assert!(!found(&e).contains(&"Landing gear"), "folded");
        // Text typed without the field focused goes nowhere.
        assert_eq!(e.text_input("gear"), ResultAction::None);
        assert!(!e.searching());
        type_search(&mut e, "GEAR");
        assert!(found(&e).contains(&"Landing gear"), "{:?}", found(&e));
        assert!(
            found(&e)
                .iter()
                .all(|l| l.to_ascii_lowercase().contains("gear"))
        );
        // A group title finds its whole group.
        e.key("Delete", false, false, false);
        type_search(&mut e, "weapons");
        assert!(found(&e).contains(&"Fire / release weapon"));
        e.key("Backspace", false, false, false);
        assert_eq!(e.search.text, "weapon");
        type_search(&mut e, "zzz");
        assert!(e.lines().is_empty());
        assert_eq!(e.key("Escape", false, false, false), ResultAction::Changed);
        assert!(!e.searching());
        assert!(
            e.lines().len() < all,
            "the fold returns once the search ends"
        );
        assert_eq!(e.key("Escape", false, false, false), ResultAction::Close);
    }
    #[test]
    fn key_names_match_whole_words_and_pressed_keys_match_exactly() {
        let mut e = Editor::new(Profile::default(), vec![], "Main menu");
        e.activate(Hit::Search(1));
        type_search(&mut e, "f");
        let by_name = found(&e);
        assert!(by_name.contains(&"Flaps"));
        assert!(!by_name.contains(&"Front cockpit view"), "F1 is not F");
        e.key("Delete", false, false, false);
        type_search(&mut e, "page");
        assert!(found(&e).contains(&"Rudder right"), "Page Down");
        // Press finds the key itself, whatever its name.
        e.activate(Hit::Search(3));
        assert!(e.capturing());
        assert!(!e.typing());
        e.key("g", false, false, false);
        assert!(!e.capturing());
        assert_eq!(found(&e), ["Landing gear"]);
        assert_eq!(e.search.text, "G");
        assert!(e.message.contains("bound to 1 action"), "{}", e.message);
        e.activate(Hit::Search(3));
        e.key("g", false, true, false);
        assert!(found(&e).is_empty(), "Ctrl+G is a different key");
        assert!(e.message.contains("not bound"), "{}", e.message);
        // Typing replaces a pressed key; Actions drops it.
        e.activate(Hit::Search(3));
        e.key("g", false, false, false);
        e.activate(Hit::Search(0));
        assert!(!e.searching());
    }
    #[test]
    fn pressing_a_modifier_finds_every_chord_that_uses_it() {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        e.adjust(Setting::Modifiers, 1);
        event(&mut e, "button:314", 1.);
        event(&mut e, "button:314", 0.);
        e.activate(Hit::Cell(line(&e, "Flaps"), 0));
        event(&mut e, "button:314", 1.);
        event(&mut e, "button:304", 1.);
        event(&mut e, "button:304", 0.);
        event(&mut e, "button:314", 0.);
        e.activate(Hit::Cell(line(&e, "Landing gear"), 0));
        event(&mut e, "axis:16", -1.);
        event(&mut e, "axis:16", 0.);
        for (control, value, expected) in [
            ("button:314", 1., vec!["Flaps"]),
            ("button:304", 1., vec!["Flaps"]),
            ("axis:16", -1., vec!["Landing gear"]),
            ("axis:16", 1., vec![]),
            // An axis moved past a third of its travel is found too.
            ("axis:1", 90., vec![]),
        ] {
            e.activate(Hit::Search(3));
            event(&mut e, control, value);
            event(&mut e, control, 0.);
            assert!(!e.capturing(), "{control}={value}: {}", e.message);
            assert_eq!(found(&e), expected, "{control}={value}");
        }
        // Typed labels work too, and a new device drops a pressed input.
        e.activate(Hit::Search(3));
        event(&mut e, "button:314", 1.);
        e.select_tab(0);
        assert!(!e.searching());
        e.select_tab(2);
        type_search(&mut e, "a");
        assert_eq!(found(&e), ["Flaps"]);
        e.select_tab(0);
        assert_eq!(e.search.text, "a", "typed text carries across devices");
    }
    #[test]
    fn search_row_joins_keyboard_navigation_except_on_the_head_tab() {
        let mut e = Editor::new(Profile::default(), vec![], "Main menu");
        // Keyboard settings are information only.
        e.key("ArrowRight", false, false, false);
        assert_eq!(e.focus, Focus::Search(SEARCH_FIELD));
        assert!(e.typing());
        e.key("ArrowDown", false, false, false);
        assert_eq!(e.focus, Focus::Cell(0, 0));
        e.key("ArrowUp", false, false, false);
        assert_eq!(e.focus, Focus::Search(SEARCH_FIELD));
        e.key("ArrowRight", false, false, false);
        e.key("ArrowRight", false, false, false);
        e.key("ArrowRight", false, false, false);
        assert_eq!(e.focus, Focus::Search(4));
        for _ in 0..5 {
            e.key("ArrowLeft", false, false, false);
        }
        assert_eq!(e.focus, Focus::Tab(0));
        e.focus = Focus::Footer(1);
        e.key("ArrowUp", false, false, false);
        assert!(matches!(e.focus, Focus::Cell(_, _)));
        assert_eq!(e.key("f", false, true, false), ResultAction::None);
        assert_eq!(e.focus, Focus::Search(SEARCH_FIELD));
        let r = Editor::search_rect(1);
        let p = ((r.0 + 3) as f64, (r.1 + 3) as f64);
        e.pointer(Some(p), true);
        e.pointer(Some(p), false);
        assert_eq!(e.search.find, Find::Inputs);
        e.preview("head").unwrap();
        assert!(!e.search_shown());
        assert_eq!(e.hit(p), None);
        e.focus = Focus::Search(SEARCH_FIELD);
        assert!(!e.typing());
        e.focus = Focus::Tab(e.tab);
        e.key("f", false, true, false);
        assert_eq!(e.focus, Focus::Tab(e.tab));
    }
    /// A synthetic font: every byte a 5 pixel advance with one dot.
    fn font() -> Font {
        Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 5,
                    pixels: vec![(1, 3)],
                })
                .collect(),
        }
    }
    fn center(r: Rect) -> (f64, f64) {
        ((r.0 + r.2 / 2) as f64, (r.1 + r.3 / 2) as f64)
    }
    /// An editor with the roll axis bound, and the roll row's visible index.
    fn with_roll() -> (Editor, usize, usize) {
        let mut e = Editor::new(Profile::default(), vec![pad()], "Main menu");
        let roll = line(&e, "Roll (bank left/right)");
        e.activate(Hit::Cell(roll, 0));
        event(&mut e, "axis:0", 90.);
        let row = roll - e.scroll;
        (e, roll, row)
    }
    fn right_click(e: &mut Editor, at: (f64, f64)) -> ResultAction {
        e.right_pointer(Some(at), true);
        e.right_pointer(Some(at), false)
    }
    #[test]
    fn the_curve_turns_forward_and_back_and_wraps_at_both_ends() {
        let forward: Vec<f64> = std::iter::successors(Some(1.), |c| Some(turn_curve(*c, true)))
            .take(6)
            .collect();
        assert_eq!(forward, [1., 1.5, 2., 3., 1., 1.5]);
        let back: Vec<f64> = std::iter::successors(Some(1.), |c| Some(turn_curve(*c, false)))
            .take(6)
            .collect();
        assert_eq!(back, [1., 3., 2., 1.5, 1., 3.]);
        // A value an edited profile left between the listed ones counts as
        // the one it is nearest to.
        assert_eq!(turn_curve(1.2, true), 1.5);
        assert_eq!(turn_curve(1.2, false), 3.);
        assert_eq!(turn_curve(2.4, true), 3.);
        assert_eq!(turn_curve(2.4, false), 1.5);
        assert_eq!(turn_curve(9., true), 1.);
        assert_eq!(turn_curve(9., false), 2.);
    }
    #[test]
    fn left_click_turns_the_curve_forward_and_right_click_back() {
        let (mut e, _, row) = with_roll();
        let curve = center(Editor::cell_rect(row, 3));
        let shown = |e: &Editor| e.profile.bindings[0].calibration.curve;
        assert_eq!(shown(&e), 1.);
        for want in [1.5, 2., 3., 1.] {
            e.pointer(Some(curve), true);
            assert_eq!(e.pointer(Some(curve), false), ResultAction::Changed);
            assert_eq!(shown(&e), want);
        }
        for want in [3., 2., 1.5, 1., 3.] {
            assert_eq!(right_click(&mut e, curve), ResultAction::Changed);
            assert_eq!(shown(&e), want);
        }
        // Forward again from the wrapped value.
        e.pointer(Some(curve), true);
        e.pointer(Some(curve), false);
        assert_eq!(shown(&e), 1.);
        assert!(e.dirty);
    }
    #[test]
    fn a_right_click_flips_the_invert_box_and_needs_a_matching_release() {
        let (mut e, _, row) = with_roll();
        let invert = center(Editor::cell_rect(row, 2));
        assert!(e.profile.bindings[0].calibration.scale > 0.);
        right_click(&mut e, invert);
        assert!(e.profile.bindings[0].calibration.scale < 0.);
        right_click(&mut e, invert);
        assert!(e.profile.bindings[0].calibration.scale > 0.);
        // Pressed on the box and released elsewhere does nothing.
        e.right_pointer(Some(invert), true);
        e.right_pointer(Some((1., 470.)), false);
        assert!(e.profile.bindings[0].calibration.scale > 0.);
        // A right click on an unassigned axis row says so, as a left one does.
        let pitch = line(&e, "Pitch (nose up/down)") - e.scroll;
        right_click(&mut e, center(Editor::cell_rect(pitch, 3)));
        assert_eq!(e.message, "Assign an axis first");
    }
    #[test]
    fn a_right_click_steps_a_setting_back_or_flips_a_toggle_and_leaves_the_rest() {
        let (mut e, _, _) = with_roll();
        let at = |e: &Editor, setting: Setting| {
            let i = e.settings().iter().position(|s| *s == setting).unwrap();
            // The right end of the value box is a forward click for the left button.
            let r = Editor::value_rect(Editor::setting_rect(i));
            ((r.0 + r.2 - 4) as f64, (r.1 + 2) as f64)
        };
        let deadzone = e.deadzone(&e.current());
        let p = at(&e, Setting::Deadzone);
        right_click(&mut e, p);
        assert!((e.deadzone(&e.current()) - (deadzone - 0.01)).abs() < 1e-9);
        // The left click on the right of the box steps up again.
        let p = at(&e, Setting::Deadzone);
        e.pointer(Some(p), true);
        e.pointer(Some(p), false);
        assert!((e.deadzone(&e.current()) - deadzone).abs() < 1e-9);
        let rumble = e.profile.rumble;
        let p = at(&e, Setting::Rumble);
        right_click(&mut e, p);
        assert_ne!(e.profile.rumble, rumble);
        let p = at(&e, Setting::Rumble);
        right_click(&mut e, p);
        assert_eq!(e.profile.rumble, rumble);
        // The modifier row starts a capture on a left click only.
        let p = at(&e, Setting::Modifiers);
        right_click(&mut e, p);
        assert!(!e.capturing());
        // Nothing else is turned: a tab, or a mapping cell.
        let tab = e.current();
        right_click(&mut e, center(Editor::tab_rect(0)));
        assert_eq!(e.current(), tab);
    }
    #[test]
    fn a_right_click_while_capturing_is_left_to_the_mouse_capture() {
        let (mut e, _, row) = with_roll();
        e.activate(Hit::Cell(row + e.scroll, 0));
        assert!(e.capturing());
        let curve = center(Editor::cell_rect(row, 3));
        let before = e.profile.bindings[0].calibration.curve;
        right_click(&mut e, curve);
        assert_eq!(e.profile.bindings[0].calibration.curve, before);
        assert!(e.capturing());
    }
    #[test]
    fn the_invert_box_shows_its_tooltip_after_a_short_hover() {
        let (mut e, roll, row) = with_roll();
        let cell = Editor::cell_rect(row, 2);
        let at = center(cell);
        let now = Instant::now();
        assert!(e.tooltip_shown(now).is_none());
        // Coming to rest on the box is a change to redraw for, but nothing shows yet.
        assert!(e.moved_at(Some(at), now));
        assert!(e.tooltip_shown(now + Duration::from_millis(100)).is_none());
        assert_eq!(
            e.tooltip_shown(now + TOOLTIP_DELAY),
            Some((INVERT_TIP, Hit::Cell(roll, 2)))
        );
        // Moving within the box keeps it; leaving it takes it away.
        assert!(!e.moved_at(Some((at.0, at.1 - 1.)), now + TOOLTIP_DELAY));
        assert!(e.tooltip_shown(now + TOOLTIP_DELAY).is_some());
        assert!(e.moved_at(None, now + TOOLTIP_DELAY));
        assert!(e.tooltip_shown(now + TOOLTIP_DELAY * 2).is_none());
        // Other controls have none: the curve button, a mapping cell.
        for column in [0, 3, 4] {
            let p = center(Editor::cell_rect(row, column));
            assert!(!e.moved_at(Some(p), now));
            assert!(e.tooltip_shown(now + TOOLTIP_DELAY * 4).is_none());
        }
        // A press hides it and starts the wait again.
        e.moved_at(Some(at), now);
        e.pressed = Some(Hit::Cell(roll, 2));
        assert!(e.tooltip_shown(now + TOOLTIP_DELAY * 4).is_none());
        e.pressed = None;
        // A tooltip does not stay where a scroll moved another row's box.
        assert!(e.tooltip_shown(now + TOOLTIP_DELAY * 4).is_some());
        e.scroll += 1;
        assert!(e.tooltip_shown(now + TOOLTIP_DELAY * 4).is_none());
    }
    #[test]
    fn the_tooltip_sits_clear_of_its_box_and_inside_the_screen() {
        let font = font();
        let width = text_width(&font, INVERT_TIP) + 8;
        for row in 0..LIST_LINES {
            let cell = Editor::cell_rect(row, 2);
            let r = Editor::tooltip_rect(&font, INVERT_TIP, cell);
            assert_eq!(r.2, width);
            assert!(r.0 >= 0 && r.0 + r.2 <= 640, "row {row} is off the sides");
            assert!(
                r.1 >= 0 && r.1 + r.3 <= 438,
                "row {row} reaches the status bar"
            );
            let apart = r.1 >= cell.1 + cell.3 || r.1 + r.3 <= cell.1 || r.0 + r.2 <= cell.0;
            assert!(apart, "row {row}: {r:?} covers {cell:?}");
        }
        // A box near the right edge is clipped in, not pushed off.
        let r = Editor::tooltip_rect(&font, INVERT_TIP, (630, 200, 16, 13));
        assert!(r.0 + r.2 <= 638);
    }
    #[test]
    fn drawing_adds_the_tooltip_only_while_it_shows() {
        let (mut e, _, row) = with_roll();
        let font = font();
        let at = center(Editor::cell_rect(row, 2));
        let now = Instant::now();
        let frame = |e: &Editor, when: Instant| {
            let mut pixels = vec![0u8; 640 * 480 * 4];
            e.draw(&mut pixels, &font);
            e.draw_tooltip(&mut pixels, &font, when);
            pixels
        };
        e.moved_at(Some(at), now);
        let early = frame(&e, now);
        let late = frame(&e, now + TOOLTIP_DELAY);
        assert_ne!(early, late);
        // The box itself is untouched by it.
        let cell = Editor::cell_rect(row, 2);
        let px = |p: &[u8], x: i32, y: i32| p[((y * 640 + x) * 4) as usize..][..4].to_vec();
        for (x, y) in [(cell.0 + 3, cell.1 + 2), (cell.0 + 11, cell.1 + 10)] {
            assert_eq!(px(&early, x, y), px(&late, x, y));
        }
    }
    #[test]
    fn pointer_requires_matching_release() {
        let mut e = Editor::new(Profile::default(), vec![], "Main menu");
        let r = Editor::tab_rect(1);
        let p = ((r.0 + 5) as f64, (r.1 + 5) as f64);
        e.pointer(Some(p), true);
        e.pointer(Some((1., 470.)), false);
        assert_eq!(e.current(), Tab::Keyboard);
        e.pointer(Some(p), true);
        e.pointer(Some(p), false);
        assert_eq!(e.current(), Tab::Mouse);
    }
}
