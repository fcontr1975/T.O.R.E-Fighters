//! Locate Fighters Anthology screen: state, key handling and CPU-canvas drawing.
//! Behaviour: docs/spec/first-run-import.md. Work package E owns this file.
//!
//! The screen is renderer independent. It keeps the typed path, the detected
//! candidates and the import phase, turns key, pointer and drop input into
//! [`Event`]s, and paints itself into a 640x480 RGBA buffer. Nothing here reads
//! retail media: the panel is flat colour and the text uses the bundled
//! large menu font, because on a first run there is no imported art yet. When
//! a pack does exist the caller passes the Choose Activity frame as
//! `background`. The screen owns its own atlas so that every string on it is
//! drawn at the same readable size.
use crate::graphics::GraphicsApi;
use crate::menu::{Canvas, HEIGHT, LARGE_CELL, Sprite, WIDTH, flat_font_large, text_width};
use std::path::PathBuf;

/// How a candidate folder is offered to the player. Detection itself lives in
/// `tore-import`; this is only the label the screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Installed,
    Disc,
}
impl SourceKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SourceKind::Installed => "Installed folder",
            SourceKind::Disc => "Disc or mounted image",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub kind: SourceKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    /// The import has begun but no archive is being read yet: the worker is
    /// identifying the build and finding what to read, which can take tens of
    /// seconds from a disc. `step` names that work in plain words.
    Starting {
        step: String,
    },
    Importing {
        archive: String,
        resources_done: usize,
        resources_total: Option<usize>,
    },
    Done {
        summary: Vec<String>,
    },
    Failed {
        reason: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    PathField,
    Candidates,
    ImportButton,
    QuitButton,
    ContinueButton,
    /// The graphics API choice, on Windows.
    ApiChoice,
}
/// What the caller should do after an input. Anything else is [`Event::None`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    None,
    Import(PathBuf),
    Quit,
    Continue,
    /// The player picked another graphics API.
    ChooseApi(GraphicsApi),
}

// Layout. Every hit rectangle below is derived from these, so `click` and
// `draw` cannot drift apart. The numbers are sized for the 24 by 17 cells of
// the large bundled atlas: one text line is `CELL` tall and successive lines
// are `LINE` apart. The panel is nearly the whole canvas because the larger
// type needs the room.
const CELL: i32 = LARGE_CELL.1 as i32;
const PANEL: (i32, i32, i32, i32) = (20, 12, 600, 456);
const INNER_X: i32 = PANEL.0 + 20;
const INNER_W: i32 = PANEL.2 - 40;
const LINE: i32 = 18;
const TITLE_Y: i32 = PANEL.1 + 10;
const PARA_Y: i32 = PANEL.1 + 42;
const PARA_LINES: usize = 5;
const FIELD_LABEL_Y: i32 = PARA_Y + PARA_LINES as i32 * LINE + 6;
const FIELD: (i32, i32, i32, i32) = (INNER_X, FIELD_LABEL_Y + LINE + 1, INNER_W, 26);
const FIELD_PAD: i32 = 8;
const LIST_LABEL_Y: i32 = FIELD.1 + FIELD.3 + 8;
const LIST_Y: i32 = LIST_LABEL_Y + LINE + 1;
const ROW_H: i32 = 20;
const MAX_ROWS: usize = 4;
const STATUS_Y: i32 = LIST_Y + MAX_ROWS as i32 * ROW_H + 8;
const STATUS_LINES: usize = 4;
const HINT_Y: i32 = STATUS_Y + STATUS_LINES as i32 * LINE + 4;
const HINT_LINES: usize = 2;
const BUTTON_W: i32 = 150;
const BUTTON_H: i32 = 28;
const BUTTON_Y: i32 = PANEL.1 + PANEL.3 - 10 - BUTTON_H;
const IMPORT_RECT: (i32, i32, i32, i32) = (INNER_X, BUTTON_Y, BUTTON_W, BUTTON_H);
const QUIT_RECT: (i32, i32, i32, i32) =
    (INNER_X + INNER_W - BUTTON_W, BUTTON_Y, BUTTON_W, BUTTON_H);
const CONTINUE_RECT: (i32, i32, i32, i32) = IMPORT_RECT;
/// The graphics API choice sits on the title line, on Windows, so it is visible
/// whatever the import is doing and needs no room from the blocks below.
const API_W: i32 = 92;
const API_H: i32 = 24;
const API_Y: i32 = PANEL.1 + 5;
const API_X: i32 = INNER_X + INNER_W - 2 * API_W - 4;
const API_RECTS: [(i32, i32, i32, i32); 2] = [
    (API_X, API_Y, API_W, API_H),
    (API_X + API_W + 4, API_Y, API_W, API_H),
];
const API_LABEL: &str = "Graphics";
const MAX_PATH_BYTES: usize = 512;
/// Frames for the starting bar to cross once. The shell ticks about thirty
/// times a second, so one crossing takes about a second and a half.
const SWEEP_FRAMES: u32 = 45;
/// Compile-time layout guard. A block of `n` text lines starting at `start`
/// ends at `start + (n - 1) * LINE + CELL`. Nothing may reach into the block
/// below it, the buttons must sit inside the panel, and the panel with its
/// drop shadow must sit inside the 640x480 canvas.
const _: () = {
    const fn ends(start: i32, lines: i32) -> i32 {
        start + (lines - 1) * LINE + CELL
    }
    assert!(ends(TITLE_Y, 1) < PARA_Y);
    assert!(ends(PARA_Y, PARA_LINES as i32) <= FIELD_LABEL_Y);
    assert!(ends(FIELD_LABEL_Y, 1) <= FIELD.1);
    assert!(FIELD.1 + FIELD.3 <= LIST_LABEL_Y);
    assert!(ends(LIST_LABEL_Y, 1) <= LIST_Y);
    assert!(CELL + 2 <= ROW_H, "a candidate row cannot hold its text");
    assert!(LIST_Y + MAX_ROWS as i32 * ROW_H <= STATUS_Y);
    assert!(ends(STATUS_Y, STATUS_LINES as i32) < HINT_Y);
    assert!(ends(HINT_Y, HINT_LINES as i32) < BUTTON_Y);
    assert!(CELL < BUTTON_H);
    assert!(BUTTON_Y + BUTTON_H <= PANEL.1 + PANEL.3);
    assert!(PANEL.0 + PANEL.2 + 4 <= WIDTH as i32);
    assert!(PANEL.1 + PANEL.3 + 4 <= HEIGHT as i32);
    assert!(IMPORT_RECT.0 + IMPORT_RECT.2 < QUIT_RECT.0);
    assert!(QUIT_RECT.0 + QUIT_RECT.2 <= PANEL.0 + PANEL.2);
    assert!(API_Y + API_H <= TITLE_Y + CELL + 4, "above the title rule");
};

const TITLE: &str = "Locate Fighters Anthology";
const EXPLANATION: &str = "T.O.R.E ships with no game media. Point it at your own copy of Jane's Fighters Anthology, either the installed game folder or a mounted disc 1, and it reads what it needs once into its own data folder. Your copy is never changed, and nothing is taken from this program. You can also drop a folder onto this window.";

const BACKDROP: [u8; 4] = [16, 20, 26, 255];
const PANEL_FILL: [u8; 4] = [27, 36, 48, 255];
const PANEL_EDGE: [u8; 4] = [124, 144, 170, 255];
const PANEL_SHADOW: [u8; 4] = [10, 13, 17, 255];
const FIELD_FILL: [u8; 4] = [13, 17, 23, 255];
const EDGE_IDLE: [u8; 4] = [86, 101, 122, 255];
const EDGE_FOCUS: [u8; 4] = [156, 194, 238, 255];
const ROW_SELECTED: [u8; 4] = [50, 72, 100, 255];
const ROW_HOVER: [u8; 4] = [38, 52, 70, 255];
const BUTTON_FILL: [u8; 4] = [44, 60, 80, 255];
const BUTTON_ACTIVE: [u8; 4] = [68, 94, 126, 255];
const BUTTON_DISABLED: [u8; 4] = [32, 40, 51, 255];
const BAR_FILL: [u8; 4] = [96, 152, 210, 255];
const DIM: [u8; 3] = [206, 216, 232];
const FAIL: [u8; 3] = [255, 140, 130];
const GOOD: [u8; 3] = [160, 232, 172];

fn in_rect(x: f64, y: f64, (rx, ry, rw, rh): (i32, i32, i32, i32)) -> bool {
    x >= rx as f64 && y >= ry as f64 && x < (rx + rw) as f64 && y < (ry + rh) as f64
}
/// Greedy word wrap. No returned line is wider than `width`, unless a single
/// word is, in which case it stands alone on its own line.
pub(crate) fn wrap(font: &Sprite, text: &str, width: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if text_width(font, &format!("{line} {word}")) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}
/// Fit `text` into `width` by dropping leading characters, so the end of a long
/// path stays readable.
pub(crate) fn truncate_left(font: &Sprite, text: &str, width: i32) -> String {
    if text_width(font, text) <= width {
        return text.to_owned();
    }
    let ellipsis = "...";
    let room = width - text_width(font, ellipsis);
    let mut start = text.len();
    for (at, _) in text.char_indices().rev() {
        if text_width(font, &text[at..]) > room {
            break;
        }
        start = at;
    }
    format!("{ellipsis}{}", &text[start..])
}
fn fit_right(font: &Sprite, text: &str, width: i32) -> String {
    let mut end = 0;
    for (at, ch) in text.char_indices() {
        let next = at + ch.len_utf8();
        if text_width(font, &text[..next]) > width {
            break;
        }
        end = next;
    }
    text[..end].to_owned()
}

pub(crate) struct Locate {
    /// Full-white large atlas. Primary text is drawn straight from it and
    /// secondary text is tinted down, so both come from one allocation.
    font: Sprite,
    path: String,
    cursor: usize,
    candidates: Vec<Candidate>,
    selected: Option<usize>,
    focus: Focus,
    phase: Phase,
    hint: Option<String>,
    hover: Option<Focus>,
    hover_row: Option<usize>,
    scroll: usize,
    /// Advanced by [`Locate::tick`] while starting, so the sweeping bar moves.
    frame: u32,
    /// The graphics API shown as chosen; None hides the choice (off Windows).
    api: Option<GraphicsApi>,
    hover_api: Option<usize>,
}
impl Locate {
    pub(crate) fn new(prefill: Option<String>, candidates: Vec<Candidate>) -> Self {
        let mut locate = Self {
            font: flat_font_large([255, 255, 255]),
            path: String::new(),
            cursor: 0,
            candidates: Vec::new(),
            selected: None,
            focus: Focus::PathField,
            phase: Phase::Idle,
            hint: None,
            hover: None,
            hover_row: None,
            scroll: 0,
            frame: 0,
            api: None,
            hover_api: None,
        };
        if let Some(prefill) = prefill.filter(|p| !p.trim().is_empty()) {
            locate.set_path(prefill);
        }
        locate.set_candidates(candidates);
        locate
    }
    fn set_path(&mut self, path: String) {
        self.path = path;
        self.cursor = self.path.len();
    }
    /// Replace the detected sources. An empty field takes the first one, which
    /// is the spec's "prefilled with the first automatically detected source".
    pub(crate) fn set_candidates(&mut self, candidates: Vec<Candidate>) {
        self.candidates = candidates;
        self.selected = None;
        self.scroll = 0;
        if self.path.trim().is_empty()
            && let Some(first) = self.candidates.first()
        {
            self.selected = Some(0);
            let path = first.path.display().to_string();
            self.set_path(path);
        }
    }
    /// Offers the graphics API choice with `api` selected; None hides it.
    pub(crate) fn set_graphics_api(&mut self, api: Option<GraphicsApi>) {
        self.api = api;
    }
    fn api_at(&self, x: f64, y: f64) -> Option<usize> {
        self.api?;
        API_RECTS.iter().position(|rect| in_rect(x, y, *rect))
    }
    /// The event for picking `index`, if that changes anything.
    fn pick_api(&self, index: usize) -> Event {
        let api = GraphicsApi::CHOICES[index];
        if self.api == Some(api) {
            Event::None
        } else {
            Event::ChooseApi(api)
        }
    }
    /// The other API, from the keyboard.
    fn toggle_api(&self) -> Event {
        let selected = GraphicsApi::CHOICES
            .iter()
            .position(|api| self.api == Some(*api))
            .unwrap_or(0);
        self.pick_api(1 - selected)
    }
    pub(crate) fn set_phase(&mut self, phase: Phase) {
        if matches!(phase, Phase::Done { .. }) {
            self.focus = Focus::ContinueButton;
        } else if self.focus == Focus::ContinueButton {
            self.focus = Focus::PathField;
        }
        self.phase = phase;
    }
    pub(crate) fn set_hint(&mut self, hint: String) {
        self.hint = Some(hint);
    }
    pub(crate) fn clear_hint(&mut self) {
        self.hint = None;
    }
    pub(crate) fn phase(&self) -> &Phase {
        &self.phase
    }
    /// Advance the starting animation by one frame. Returns true when the
    /// screen needs redrawing, which is only while the import is starting.
    pub(crate) fn tick(&mut self) -> bool {
        if matches!(self.phase, Phase::Starting { .. }) {
            self.frame = self.frame.wrapping_add(1);
            true
        } else {
            false
        }
    }
    fn importing(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Importing { .. })
    }
    fn done(&self) -> bool {
        matches!(self.phase, Phase::Done { .. })
    }
    /// A file or folder dropped on the window fills the field verbatim. The
    /// shell decides whether a dropped file means its parent directory.
    pub(crate) fn dropped_path(&mut self, path: PathBuf) {
        if self.importing() {
            return;
        }
        let text = path.display().to_string();
        self.set_path(text);
        self.selected = None;
        self.focus = Focus::PathField;
        self.hint = None;
    }
    /// A printable character typed into the field, with its case preserved.
    pub(crate) fn text_input(&mut self, ch: char) {
        if self.importing() || self.done() || ch.is_control() {
            return;
        }
        // Space on the choice has already toggled it.
        if self.focus == Focus::ApiChoice && ch == ' ' {
            return;
        }
        if self.path.len() + ch.len_utf8() > MAX_PATH_BYTES {
            return;
        }
        self.focus = Focus::PathField;
        self.path.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.selected = None;
    }
    fn order(&self) -> Vec<Focus> {
        if self.done() {
            return vec![Focus::ContinueButton];
        }
        let mut order = vec![Focus::PathField];
        if !self.candidates.is_empty() {
            order.push(Focus::Candidates);
        }
        order.push(Focus::ImportButton);
        order.push(Focus::QuitButton);
        if self.api.is_some() {
            order.push(Focus::ApiChoice);
        }
        order
    }
    fn step_focus(&mut self, delta: i32) {
        let order = self.order();
        let at = order.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        self.focus = order[(at + delta).rem_euclid(order.len() as i32) as usize];
    }
    fn select(&mut self, index: usize) {
        let Some(candidate) = self.candidates.get(index) else {
            return;
        };
        let path = candidate.path.display().to_string();
        self.selected = Some(index);
        self.set_path(path);
        self.focus = Focus::Candidates;
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll + MAX_ROWS {
            self.scroll = index + 1 - MAX_ROWS;
        }
    }
    fn prev_boundary(&self) -> usize {
        self.path[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(at, _)| at)
    }
    fn next_boundary(&self) -> usize {
        self.path[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |ch| self.cursor + ch.len_utf8())
    }
    fn import_event(&self) -> Event {
        let path = self.path.trim();
        if path.is_empty() || self.importing() {
            Event::None
        } else {
            Event::Import(PathBuf::from(path))
        }
    }
    /// Key names arrive as winit-style strings. Printable characters come
    /// through [`Self::text_input`] instead, so their case survives.
    pub(crate) fn key(&mut self, name: &str) -> Event {
        if self.done() {
            return match name {
                "Enter" | " " | "Escape" => Event::Continue,
                _ => Event::None,
            };
        }
        if self.importing() {
            return Event::None;
        }
        if self.focus == Focus::ApiChoice
            && matches!(name, "Enter" | " " | "ArrowLeft" | "ArrowRight")
        {
            return self.toggle_api();
        }
        match name {
            "Escape" => return Event::Quit,
            "Enter" => {
                return if self.focus == Focus::QuitButton {
                    Event::Quit
                } else {
                    self.import_event()
                };
            }
            "Tab" => self.step_focus(1),
            "ArrowDown" => self.move_vertically(1),
            "ArrowUp" => self.move_vertically(-1),
            "Backspace" if self.focus == Focus::PathField && self.cursor > 0 => {
                let at = self.prev_boundary();
                self.path.replace_range(at..self.cursor, "");
                self.cursor = at;
                self.selected = None;
            }
            "Delete" if self.focus == Focus::PathField && self.cursor < self.path.len() => {
                let to = self.next_boundary();
                self.path.replace_range(self.cursor..to, "");
                self.selected = None;
            }
            "Home" => self.cursor = 0,
            "End" => self.cursor = self.path.len(),
            "ArrowLeft" if self.focus == Focus::PathField => self.cursor = self.prev_boundary(),
            "ArrowRight" if self.focus == Focus::PathField => self.cursor = self.next_boundary(),
            _ => {}
        }
        Event::None
    }
    fn move_vertically(&mut self, delta: i32) {
        match (self.focus, delta) {
            (Focus::PathField, 1) => {
                if self.candidates.is_empty() {
                    self.focus = Focus::ImportButton;
                } else {
                    self.select(self.selected.unwrap_or(0));
                }
            }
            (Focus::Candidates, _) => {
                let at = self.selected.unwrap_or(0) as i32 + delta;
                if at < 0 {
                    self.focus = Focus::PathField;
                } else if at as usize >= self.candidates.len() {
                    self.focus = Focus::ImportButton;
                } else {
                    self.select(at as usize);
                }
            }
            (Focus::ImportButton, 1) => self.focus = Focus::QuitButton,
            (Focus::ImportButton, _) => {
                if self.candidates.is_empty() {
                    self.focus = Focus::PathField;
                } else {
                    self.select(self.candidates.len() - 1);
                }
            }
            (Focus::QuitButton, -1) => self.focus = Focus::ImportButton,
            (_, _) => self.step_focus(delta),
        }
    }
    fn caret_from_click(&mut self, x: f64) {
        let font = &self.font;
        let (visible, start) = self.field_view(font);
        let target = x - (FIELD.0 + FIELD_PAD) as f64;
        let mut at = start;
        for (offset, ch) in visible.char_indices() {
            let advance = text_width(font, &visible[offset..offset + ch.len_utf8()]) as f64;
            let left = text_width(font, &visible[..offset]) as f64;
            if target < left + advance / 2.0 {
                self.cursor = start + offset;
                return;
            }
            at = start + offset + ch.len_utf8();
        }
        self.cursor = at.min(self.path.len());
    }
    pub(crate) fn hover(&mut self, x: f64, y: f64) {
        self.hover = None;
        self.hover_row = None;
        self.hover_api = self.api_at(x, y);
        if self.done() {
            if in_rect(x, y, CONTINUE_RECT) {
                self.hover = Some(Focus::ContinueButton);
            }
            return;
        }
        if in_rect(x, y, IMPORT_RECT) {
            self.hover = Some(Focus::ImportButton);
        } else if in_rect(x, y, QUIT_RECT) {
            self.hover = Some(Focus::QuitButton);
        } else if in_rect(x, y, FIELD) {
            self.hover = Some(Focus::PathField);
        } else if let Some(row) = self.row_at(x, y) {
            self.hover = Some(Focus::Candidates);
            self.hover_row = Some(row);
        }
    }
    fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        let rows = self
            .candidates
            .len()
            .saturating_sub(self.scroll)
            .min(MAX_ROWS);
        for row in 0..rows {
            if in_rect(x, y, (INNER_X, LIST_Y + row as i32 * ROW_H, INNER_W, ROW_H)) {
                return Some(self.scroll + row);
            }
        }
        None
    }
    pub(crate) fn click(&mut self, x: f64, y: f64) -> Event {
        // Usable before, during and after an import: it touches only the
        // graphics options, and it is read when the game starts.
        if let Some(index) = self.api_at(x, y) {
            if !self.done() && !self.importing() {
                self.focus = Focus::ApiChoice;
            }
            return self.pick_api(index);
        }
        if self.done() {
            return if in_rect(x, y, CONTINUE_RECT) {
                Event::Continue
            } else {
                Event::None
            };
        }
        if self.importing() {
            return Event::None;
        }
        if in_rect(x, y, IMPORT_RECT) {
            self.focus = Focus::ImportButton;
            return self.import_event();
        }
        if in_rect(x, y, QUIT_RECT) {
            self.focus = Focus::QuitButton;
            return Event::Quit;
        }
        if in_rect(x, y, FIELD) {
            self.focus = Focus::PathField;
            self.caret_from_click(x);
            return Event::None;
        }
        if let Some(row) = self.row_at(x, y) {
            self.select(row);
        }
        Event::None
    }
    /// The visible slice of the field text and the byte offset it starts at.
    /// A path longer than the field scrolls so that the caret stays in view.
    fn field_view(&self, font: &Sprite) -> (String, usize) {
        let room = FIELD.2 - FIELD_PAD * 2 - 2;
        let mut start = 0;
        if text_width(font, &self.path) > room {
            start = self.cursor;
            for (at, _) in self.path[..self.cursor].char_indices().rev() {
                if text_width(font, &self.path[at..self.cursor]) > room {
                    break;
                }
                start = at;
            }
        }
        (fit_right(font, &self.path[start..], room), start)
    }
    fn status_lines(&self, font: &Sprite) -> Vec<(String, Option<[u8; 3]>)> {
        let width = INNER_W;
        match &self.phase {
            Phase::Idle => Vec::new(),
            Phase::Starting { step } => {
                vec![
                    ("Starting import...".to_owned(), None),
                    (step.clone(), Some(DIM)),
                ]
            }
            Phase::Importing {
                archive,
                resources_done,
                resources_total,
            } => {
                let count = match resources_total {
                    Some(total) => format!("{resources_done} of {total} resources"),
                    None => format!("{resources_done} resources"),
                };
                vec![(format!("Reading {archive}"), None), (count, Some(DIM))]
            }
            Phase::Done { summary } => {
                let mut lines = vec![("Import complete".to_owned(), Some(GOOD))];
                for line in summary {
                    lines.extend(wrap(font, line, width).into_iter().map(|l| (l, Some(DIM))));
                }
                lines
            }
            Phase::Failed { reason } => {
                let mut lines = vec![("Import failed".to_owned(), Some(FAIL))];
                lines.extend(wrap(font, reason, width).into_iter().map(|l| (l, None)));
                lines
            }
        }
    }
    /// Paint the screen. The two font arguments are vestigial: the screen owns
    /// its atlas so that everything on it is one readable size, and they are
    /// kept only so the callers in `main.rs` need not change in this step.
    pub(crate) fn draw(
        &self,
        pixels: &mut [u8],
        _font: &Sprite,
        _small: &Sprite,
        background: Option<&[u8]>,
    ) {
        let (font, small) = (&self.font, &self.font);
        assert_eq!(
            pixels.len(),
            WIDTH * HEIGHT * 4,
            "locate needs a 640x480 RGBA buffer"
        );
        match background {
            Some(art) if art.len() == pixels.len() => pixels.copy_from_slice(art),
            _ => Canvas(pixels).rect((0, 0, WIDTH as i32, HEIGHT as i32), BACKDROP),
        }
        let mut canvas = Canvas(pixels);
        let (px, py, pw, ph) = PANEL;
        canvas.rect((px + 4, py + 4, pw, ph), PANEL_SHADOW);
        canvas.rect(PANEL, PANEL_FILL);
        canvas.outline(PANEL, PANEL_EDGE);
        canvas.text(font, TITLE, INNER_X, TITLE_Y, None);
        canvas.rect((INNER_X, TITLE_Y + CELL + 4, INNER_W, 1), PANEL_EDGE);
        if self.api.is_some() {
            self.draw_api_choice(&mut canvas, font);
        }
        for (row, line) in wrap(small, EXPLANATION, INNER_W)
            .iter()
            .take(PARA_LINES)
            .enumerate()
        {
            canvas.text(small, line, INNER_X, PARA_Y + row as i32 * LINE, Some(DIM));
        }
        // Path field.
        canvas.text(small, "Folder", INNER_X, FIELD_LABEL_Y, Some(DIM));
        canvas.rect(FIELD, FIELD_FILL);
        canvas.outline(
            FIELD,
            if self.focus == Focus::PathField {
                EDGE_FOCUS
            } else {
                EDGE_IDLE
            },
        );
        let (visible, start) = self.field_view(font);
        let text_y = FIELD.1 + (FIELD.3 - font.height as i32) / 2;
        canvas.text(font, &visible, FIELD.0 + FIELD_PAD, text_y, None);
        if self.focus == Focus::PathField && !self.importing() && !self.done() {
            let caret = text_width(font, &self.path[start..self.cursor.max(start)]);
            canvas.rect(
                (FIELD.0 + FIELD_PAD + caret, FIELD.1 + 4, 1, FIELD.3 - 8),
                EDGE_FOCUS,
            );
        }
        // Detected sources.
        let heading = if self.candidates.is_empty() {
            "No sources detected. Type or drop a folder above."
        } else {
            "Detected sources"
        };
        canvas.text(small, heading, INNER_X, LIST_LABEL_Y, Some(DIM));
        let shown = self
            .candidates
            .len()
            .saturating_sub(self.scroll)
            .min(MAX_ROWS);
        for row in 0..shown {
            let index = self.scroll + row;
            let rect = (INNER_X, LIST_Y + row as i32 * ROW_H, INNER_W, ROW_H);
            let chosen = self.selected == Some(index);
            if chosen {
                canvas.rect(rect, ROW_SELECTED);
                if self.focus == Focus::Candidates {
                    canvas.outline(rect, EDGE_FOCUS);
                }
            } else if self.hover_row == Some(index) {
                canvas.rect(rect, ROW_HOVER);
            }
            let candidate = &self.candidates[index];
            let kind = candidate.kind.label();
            let kind_w = text_width(small, kind) + 12;
            canvas.text(small, kind, rect.0 + 6, rect.1 + 2, Some(DIM));
            let path = candidate.path.display().to_string();
            canvas.text(
                small,
                &truncate_left(small, &path, rect.2 - kind_w - 12),
                rect.0 + 6 + kind_w,
                rect.1 + 2,
                None,
            );
        }
        if self.candidates.len() > self.scroll + shown {
            canvas.text(
                small,
                &format!("{} more", self.candidates.len() - self.scroll - shown),
                INNER_X,
                LIST_Y + MAX_ROWS as i32 * ROW_H - CELL,
                Some(DIM),
            );
        }
        // Progress, summary or failure.
        for (row, (line, tint)) in self
            .status_lines(small)
            .into_iter()
            .take(STATUS_LINES)
            .enumerate()
        {
            canvas.text(small, &line, INNER_X, STATUS_Y + row as i32 * LINE, tint);
        }
        if let Phase::Importing {
            resources_done,
            resources_total: Some(total),
            ..
        } = &self.phase
        {
            let bar = (INNER_X, STATUS_Y + 2 * LINE + 4, INNER_W, 10);
            canvas.rect(bar, FIELD_FILL);
            canvas.outline(bar, EDGE_IDLE);
            let filled = if *total == 0 {
                0
            } else {
                ((INNER_W - 2) as i64 * (*resources_done).min(*total) as i64 / *total as i64) as i32
            };
            canvas.rect((bar.0 + 1, bar.1 + 1, filled, bar.3 - 2), BAR_FILL);
        }
        if let Phase::Starting { .. } = &self.phase {
            // No count is known yet, so a block sweeps back and forth to show
            // the import is alive.
            let bar = (INNER_X, STATUS_Y + 2 * LINE + 4, INNER_W, 10);
            canvas.rect(bar, FIELD_FILL);
            canvas.outline(bar, EDGE_IDLE);
            let block = INNER_W / 5;
            let travel = INNER_W - 2 - block;
            let phase = (self.frame % (2 * SWEEP_FRAMES)) as i32;
            let step = if phase < SWEEP_FRAMES as i32 {
                phase
            } else {
                2 * SWEEP_FRAMES as i32 - phase
            };
            let x = travel * step / SWEEP_FRAMES as i32;
            canvas.rect((bar.0 + 1 + x, bar.1 + 1, block, bar.3 - 2), BAR_FILL);
        }
        if let Some(hint) = &self.hint {
            for (row, line) in wrap(small, hint, INNER_W)
                .iter()
                .take(HINT_LINES)
                .enumerate()
            {
                canvas.text(small, line, INNER_X, HINT_Y + row as i32 * LINE, Some(DIM));
            }
        }
        // Buttons.
        if self.done() {
            self.button(
                &mut canvas,
                font,
                "Continue",
                CONTINUE_RECT,
                Focus::ContinueButton,
                true,
            );
        } else {
            let ready = !self.path.trim().is_empty() && !self.importing();
            self.button(
                &mut canvas,
                font,
                "Import",
                IMPORT_RECT,
                Focus::ImportButton,
                ready,
            );
            self.button(
                &mut canvas,
                font,
                "Quit",
                QUIT_RECT,
                Focus::QuitButton,
                !self.importing(),
            );
        }
    }
    fn draw_api_choice(&self, canvas: &mut Canvas<'_>, font: &Sprite) {
        let label_x = API_X - 8 - text_width(font, API_LABEL);
        canvas.text(font, API_LABEL, label_x, TITLE_Y, Some(DIM));
        for (index, rect) in API_RECTS.into_iter().enumerate() {
            let api = GraphicsApi::CHOICES[index];
            let chosen = self.api == Some(api);
            canvas.rect(
                rect,
                if chosen {
                    ROW_SELECTED
                } else if self.hover_api == Some(index) {
                    ROW_HOVER
                } else {
                    BUTTON_DISABLED
                },
            );
            canvas.outline(
                rect,
                if chosen && self.focus == Focus::ApiChoice {
                    EDGE_FOCUS
                } else if chosen {
                    PANEL_EDGE
                } else {
                    EDGE_IDLE
                },
            );
            canvas.centered_text(font, api.label(), rect);
        }
    }
    fn button(
        &self,
        canvas: &mut Canvas<'_>,
        font: &Sprite,
        label: &str,
        rect: (i32, i32, i32, i32),
        focus: Focus,
        enabled: bool,
    ) {
        let active = enabled && (self.focus == focus || self.hover == Some(focus));
        canvas.rect(
            rect,
            match (enabled, active) {
                (false, _) => BUTTON_DISABLED,
                (true, true) => BUTTON_ACTIVE,
                (true, false) => BUTTON_FILL,
            },
        );
        canvas.outline(
            rect,
            if self.focus == focus {
                EDGE_FOCUS
            } else {
                EDGE_IDLE
            },
        );
        if enabled {
            canvas.centered_text(font, label, rect);
        } else {
            let x = rect.0 + (rect.2 - text_width(font, label)) / 2;
            canvas.text(
                font,
                label,
                x,
                rect.1 + (rect.3 - font.height as i32) / 2,
                Some(DIM),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn font() -> Sprite {
        flat_font_large([255, 255, 255])
    }
    fn candidates() -> Vec<Candidate> {
        vec![
            Candidate {
                path: PathBuf::from("/home/pilot/games/Fighters Anthology"),
                kind: SourceKind::Installed,
            },
            Candidate {
                path: PathBuf::from("/run/media/pilot/FA_DISC1"),
                kind: SourceKind::Disc,
            },
        ]
    }
    fn type_text(locate: &mut Locate, text: &str) {
        for ch in text.chars() {
            locate.text_input(ch);
        }
    }

    #[test]
    fn the_graphics_api_choice_is_hidden_unless_offered() {
        let mut l = Locate::new(Some("/tmp/fa".into()), Vec::new());
        let center = |r: (i32, i32, i32, i32)| ((r.0 + r.2 / 2) as f64, (r.1 + r.3 / 2) as f64);
        let (x, y) = center(API_RECTS[1]);
        assert_eq!(l.click(x, y), Event::None);
        assert!(!l.order().contains(&Focus::ApiChoice));
        l.set_graphics_api(Some(GraphicsApi::Vulkan));
        assert!(l.order().contains(&Focus::ApiChoice));
        assert_eq!(l.click(x, y), Event::ChooseApi(GraphicsApi::Dx12));
        l.set_graphics_api(Some(GraphicsApi::Dx12));
        assert_eq!(l.click(x, y), Event::None, "already chosen");
    }
    #[test]
    fn the_graphics_api_choice_works_from_the_keyboard_without_touching_the_path() {
        let mut l = Locate::new(Some("/tmp/fa".into()), Vec::new());
        l.set_graphics_api(Some(GraphicsApi::Vulkan));
        l.focus = Focus::QuitButton;
        l.key("Tab");
        assert_eq!(l.focus, Focus::ApiChoice);
        assert_eq!(l.key("Enter"), Event::ChooseApi(GraphicsApi::Dx12));
        assert_eq!(l.key(" "), Event::ChooseApi(GraphicsApi::Dx12));
        l.text_input(' ');
        assert_eq!(l.path, "/tmp/fa");
        l.set_graphics_api(Some(GraphicsApi::Dx12));
        assert_eq!(l.key("ArrowLeft"), Event::ChooseApi(GraphicsApi::Vulkan));
        l.key("Tab");
        assert_eq!(l.focus, Focus::PathField);
    }
    #[test]
    fn the_graphics_api_choice_stays_usable_during_and_after_an_import() {
        let mut l = Locate::new(Some("/tmp/fa".into()), Vec::new());
        l.set_graphics_api(Some(GraphicsApi::Vulkan));
        let (x, y) = ((API_RECTS[1].0 + 4) as f64, (API_RECTS[1].1 + 4) as f64);
        l.set_phase(Phase::Starting {
            step: "Opening".into(),
        });
        assert_eq!(l.click(x, y), Event::ChooseApi(GraphicsApi::Dx12));
        l.set_phase(Phase::Done {
            summary: Vec::new(),
        });
        assert_eq!(l.click(x, y), Event::ChooseApi(GraphicsApi::Dx12));
        assert_eq!(l.focus, Focus::ContinueButton);
    }
    #[test]
    fn the_title_and_the_graphics_api_label_do_not_collide() {
        let font = font();
        let title_end = INNER_X + text_width(&font, TITLE);
        let label_x = API_X - 8 - text_width(&font, API_LABEL);
        assert!(title_end + 8 <= label_x, "{title_end} and {label_x}");
        for api in GraphicsApi::CHOICES {
            assert!(text_width(&font, api.label()) + 4 <= API_W, "{api:?}");
        }
    }
    #[test]
    fn typing_preserves_case_and_edits_at_the_caret() {
        let mut l = Locate::new(None, Vec::new());
        type_text(&mut l, "D:/Janes/FA");
        assert_eq!(l.path, "D:/Janes/FA");
        assert_eq!(l.key("Backspace"), Event::None);
        assert_eq!(l.path, "D:/Janes/F");
        l.key("Home");
        assert_eq!(l.cursor, 0);
        // Backspace at the start of the field does nothing.
        l.key("Backspace");
        assert_eq!(l.path, "D:/Janes/F");
        l.key("Delete");
        assert_eq!(l.path, ":/Janes/F");
        l.key("ArrowRight");
        type_text(&mut l, "C");
        assert_eq!(l.path, ":C/Janes/F");
        l.key("End");
        type_text(&mut l, "A");
        assert_eq!(l.path, ":C/Janes/FA");
        l.key("ArrowLeft");
        l.key("ArrowLeft");
        assert_eq!(l.cursor, l.path.len() - 2);
    }
    #[test]
    fn the_field_length_is_capped() {
        let mut l = Locate::new(None, Vec::new());
        type_text(&mut l, &"a".repeat(MAX_PATH_BYTES + 40));
        assert_eq!(l.path.len(), MAX_PATH_BYTES);
    }
    #[test]
    fn tab_cycles_focus_and_skips_an_empty_candidate_list() {
        let mut l = Locate::new(Some("/tmp/fa".into()), candidates());
        assert_eq!(l.focus, Focus::PathField);
        for expected in [
            Focus::Candidates,
            Focus::ImportButton,
            Focus::QuitButton,
            Focus::PathField,
        ] {
            l.key("Tab");
            assert_eq!(l.focus, expected);
        }
        let mut empty = Locate::new(Some("/tmp/fa".into()), Vec::new());
        for expected in [Focus::ImportButton, Focus::QuitButton, Focus::PathField] {
            empty.key("Tab");
            assert_eq!(empty.focus, expected);
        }
    }
    #[test]
    fn selecting_a_candidate_copies_its_path_into_the_field() {
        let mut l = Locate::new(Some("/typed".into()), candidates());
        assert_eq!(l.path, "/typed");
        assert_eq!(l.selected, None);
        l.key("ArrowDown");
        assert_eq!(l.focus, Focus::Candidates);
        assert_eq!(l.path, "/home/pilot/games/Fighters Anthology");
        l.key("ArrowDown");
        assert_eq!(l.selected, Some(1));
        assert_eq!(l.path, "/run/media/pilot/FA_DISC1");
        // Past the last row the focus moves on to the buttons.
        l.key("ArrowDown");
        assert_eq!(l.focus, Focus::ImportButton);
        // Clicking a row selects it too.
        assert_eq!(
            l.click(INNER_X as f64 + 4.0, LIST_Y as f64 + 2.0),
            Event::None
        );
        assert_eq!(l.selected, Some(0));
        assert_eq!(l.path, "/home/pilot/games/Fighters Anthology");
    }
    #[test]
    fn an_empty_field_takes_the_first_detected_source() {
        let l = Locate::new(None, candidates());
        assert_eq!(l.path, "/home/pilot/games/Fighters Anthology");
        assert_eq!(l.selected, Some(0));
        let kept = Locate::new(Some("/kept".into()), candidates());
        assert_eq!(kept.path, "/kept");
    }
    #[test]
    fn enter_imports_the_field_path_and_continues_when_done() {
        let mut l = Locate::new(None, Vec::new());
        assert_eq!(l.key("Enter"), Event::None);
        type_text(&mut l, "  /mnt/disc1  ");
        assert_eq!(l.key("Enter"), Event::Import(PathBuf::from("/mnt/disc1")));
        l.set_phase(Phase::Done {
            summary: vec!["FA.EXE: 1.02F".into()],
        });
        assert_eq!(l.focus, Focus::ContinueButton);
        assert_eq!(l.key("Enter"), Event::Continue);
        assert_eq!(l.key("Escape"), Event::Continue);
        // Typing cannot change the field once the import has finished.
        type_text(&mut l, "x");
        assert_eq!(l.path, "  /mnt/disc1  ");
    }
    #[test]
    fn escape_quits_when_idle_or_failed_and_is_ignored_while_importing() {
        let mut l = Locate::new(Some("/mnt/disc1".into()), Vec::new());
        assert_eq!(l.key("Escape"), Event::Quit);
        l.set_phase(Phase::Importing {
            archive: "FA_1.LIB".into(),
            resources_done: 10,
            resources_total: Some(100),
        });
        assert_eq!(l.key("Escape"), Event::None);
        assert_eq!(l.key("Enter"), Event::None);
        assert_eq!(
            l.click(IMPORT_RECT.0 as f64 + 2.0, IMPORT_RECT.1 as f64 + 2.0),
            Event::None
        );
        assert_eq!(
            l.click(QUIT_RECT.0 as f64 + 2.0, QUIT_RECT.1 as f64 + 2.0),
            Event::None
        );
        l.set_phase(Phase::Failed {
            reason: "FA_2.LIB is truncated".into(),
        });
        assert_eq!(l.key("Escape"), Event::Quit);
        assert_eq!(l.focus, Focus::PathField);
    }
    #[test]
    fn starting_locks_the_controls_and_animates_until_reading_begins() {
        let mut l = Locate::new(Some("/mnt/disc1".into()), Vec::new());
        assert!(!l.tick(), "an idle screen has nothing to animate");
        l.set_phase(Phase::Starting {
            step: "Identifying the game build".into(),
        });
        assert_eq!(l.key("Escape"), Event::None);
        assert_eq!(l.key("Enter"), Event::None);
        type_text(&mut l, "x");
        assert_eq!(l.path, "/mnt/disc1");
        let font = font();
        let lines = l.status_lines(&font);
        assert_eq!(lines[0].0, "Starting import...");
        assert_eq!(lines[1].0, "Identifying the game build");
        let mut before = vec![0u8; WIDTH * HEIGHT * 4];
        l.draw(&mut before, &font, &font, None);
        for _ in 0..10 {
            assert!(l.tick());
        }
        let mut after = vec![0u8; WIDTH * HEIGHT * 4];
        l.draw(&mut after, &font, &font, None);
        assert_ne!(before, after, "the starting bar did not move");
        l.set_phase(Phase::Importing {
            archive: "FA_1.LIB".into(),
            resources_done: 0,
            resources_total: Some(100),
        });
        assert!(!l.tick(), "a counted import needs no animation");
    }
    #[test]
    fn clicking_each_button_reports_its_event() {
        let mut l = Locate::new(Some("/mnt/disc1".into()), Vec::new());
        let centre = |r: (i32, i32, i32, i32)| ((r.0 + r.2 / 2) as f64, (r.1 + r.3 / 2) as f64);
        let (x, y) = centre(QUIT_RECT);
        assert_eq!(l.click(x, y), Event::Quit);
        let (x, y) = centre(IMPORT_RECT);
        assert_eq!(l.click(x, y), Event::Import(PathBuf::from("/mnt/disc1")));
        assert_eq!(l.click(5.0, 5.0), Event::None);
        // The caret follows a click in the field.
        let (x, y) = centre(FIELD);
        assert_eq!(l.click((FIELD.0 + FIELD_PAD) as f64, y), Event::None);
        assert_eq!(l.focus, Focus::PathField);
        assert_eq!(l.cursor, 0);
        l.click(x, y);
        assert_eq!(l.cursor, l.path.len());
        l.set_phase(Phase::Done {
            summary: vec!["archives read: 2".into()],
        });
        let (x, y) = centre(CONTINUE_RECT);
        assert_eq!(l.click(x, y), Event::Continue);
        assert_eq!(l.click(5.0, 5.0), Event::None);
    }
    #[test]
    fn hover_reports_the_control_under_the_pointer() {
        let mut l = Locate::new(None, candidates());
        l.hover((IMPORT_RECT.0 + 2) as f64, (IMPORT_RECT.1 + 2) as f64);
        assert_eq!(l.hover, Some(Focus::ImportButton));
        l.hover(INNER_X as f64 + 2.0, LIST_Y as f64 + ROW_H as f64 + 2.0);
        assert_eq!(l.hover_row, Some(1));
        l.hover(2.0, 2.0);
        assert_eq!(l.hover, None);
    }
    #[test]
    fn a_dropped_path_fills_the_field_verbatim() {
        let mut l = Locate::new(None, candidates());
        l.set_hint("Mount the image and choose the mounted folder".into());
        l.dropped_path(PathBuf::from("/run/media/pilot/FA_DISC1/SETUP.ESA"));
        assert_eq!(l.path, "/run/media/pilot/FA_DISC1/SETUP.ESA");
        assert_eq!(l.selected, None);
        assert!(l.hint.is_none());
    }
    #[test]
    fn long_paths_truncate_from_the_left_with_an_ellipsis() {
        let f = font();
        let long = format!(
            "/run/media/pilot/{}/disc1",
            "very-long-volume-name".repeat(6)
        );
        let fitted = truncate_left(&f, &long, 300);
        assert!(fitted.starts_with("..."));
        assert!(fitted.ends_with("/disc1"));
        assert!(text_width(&f, &fitted) <= 300);
        assert_eq!(truncate_left(&f, "/mnt/d", 300), "/mnt/d");
    }
    #[test]
    fn the_explanation_wraps_inside_the_panel() {
        let f = font();
        let lines = wrap(&f, EXPLANATION, INNER_W);
        assert!(!lines.is_empty());
        assert!(
            lines.len() <= PARA_LINES,
            "the explanation needs {} lines, the panel reserves {PARA_LINES}",
            lines.len()
        );
        for line in &lines {
            assert!(text_width(&f, line) <= INNER_W, "line too wide: {line}");
        }
        // Words are not lost or joined.
        assert_eq!(lines.join(" "), EXPLANATION);
    }
    #[test]
    fn the_screen_and_its_layout_agree_on_the_cell_height() {
        // `LAYOUT` checks the block chain at compile time; the atlas is the
        // one thing it cannot see.
        assert_eq!(CELL, font().height as i32);
        assert_eq!(font().glyphs[b'M' as usize][2] as i32, CELL);
    }
    #[test]
    fn button_labels_fit_their_buttons() {
        let f = font();
        for label in ["Import", "Quit", "Continue"] {
            assert!(
                text_width(&f, label) + 16 <= BUTTON_W,
                "{label} does not fit a {BUTTON_W} pixel button"
            );
        }
        assert!(text_width(&f, TITLE) <= INNER_W);
        assert!(text_width(&f, "No sources detected. Type or drop a folder above.") <= INNER_W);
        // A row label plus its truncated path has to leave room for both.
        let kind = text_width(&f, SourceKind::Disc.label()) + 12;
        assert!(INNER_W - kind - 12 > 200, "no room left for the path");
    }
    #[test]
    fn the_field_scrolls_so_the_caret_stays_visible() {
        let f = font();
        let mut l = Locate::new(None, Vec::new());
        type_text(&mut l, &"/some/very/long/path/segment".repeat(6));
        let (visible, start) = l.field_view(&f);
        assert!(start > 0);
        assert!(text_width(&f, &visible) <= FIELD.2 - FIELD_PAD * 2 - 2);
        assert!(l.path.ends_with(&visible));
        l.key("Home");
        let (_, start) = l.field_view(&f);
        assert_eq!(start, 0);
    }
    #[test]
    fn every_phase_draws_a_panel_without_panicking() {
        let (font, small) = (font(), font());
        let phases = [
            ("idle", Phase::Idle),
            (
                "starting",
                Phase::Starting {
                    step: "Identifying the game build".into(),
                },
            ),
            (
                "importing",
                Phase::Importing {
                    archive: "FA_1.LIB".into(),
                    resources_done: 1200,
                    resources_total: Some(4800),
                },
            ),
            (
                "importing-unknown-total",
                Phase::Importing {
                    archive: "FA_4B.LIB".into(),
                    resources_done: 17,
                    resources_total: None,
                },
            ),
            (
                "done",
                Phase::Done {
                    summary: vec![
                        "FA.EXE: 1.0 (disc)".into(),
                        "archives read: FA_1.LIB, FA_2.LIB".into(),
                        "missing optional parts: recorded music, radio".into(),
                    ],
                },
            ),
            (
                "failed",
                Phase::Failed {
                    reason: "That folder is not a Fighters Anthology source: FA_1.LIB and SETUP.ESA are both missing.".into(),
                },
            ),
        ];
        let art = vec![90u8; WIDTH * HEIGHT * 4];
        for (name, phase) in phases {
            for background in [None, Some(&art[..])] {
                let mut l = Locate::new(None, candidates());
                l.set_hint(
                    "Mount the image and choose the mounted folder: udisksctl loop-setup on Linux, double-click on Windows and macOS."
                        .into(),
                );
                l.set_phase(phase.clone());
                let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
                l.draw(&mut pixels, &font, &small, background);
                let first = pixels[..4].to_vec();
                assert!(
                    pixels.chunks_exact(4).any(|p| p != &first[..]),
                    "{name} drew a blank frame"
                );
                // The panel is opaque over whatever the background was.
                let middle = ((PANEL.1 + 2) as usize * WIDTH + (PANEL.0 + 2) as usize) * 4;
                assert_eq!(pixels[middle + 3], 255);
                if background.is_none() {
                    save_ppm(name, &pixels);
                }
            }
        }
    }
    /// Visual evidence for a human, written only when asked for: the repository
    /// keeps generated images out of the tree, so `.local/` is the only home.
    fn save_ppm(name: &str, pixels: &[u8]) {
        use std::io::Write;
        if std::env::var_os("TORE_LOCATE_PPM").is_none() {
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(mut file) = std::fs::File::create(dir.join(format!("locate-{name}.ppm"))) else {
            return;
        };
        let _ = write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n");
        for p in pixels.chunks_exact(4) {
            let _ = file.write_all(&p[..3]);
        }
    }
}
