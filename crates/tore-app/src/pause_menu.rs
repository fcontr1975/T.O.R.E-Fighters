//! The paused Escape menu shared by flight and the replay viewer: a tab
//! strip over a menu tree (the imported `FMENUD.MNU` rows, or a tree built
//! from them), a panel of the current tab's rows, a row of bottom buttons
//! and pages of keyboard help. It owns navigation, hit geometry and drawing;
//! what a row, tab or button does is the caller's. Placement, colours,
//! submenu presentation and the bottom buttons are authored; see
//! docs/FLIGHT-CONTROLS.md.
use crate::{hud::Paint, menu::Canvas};
use tore_formats::{font::Font, ui::MenuNode};

/// A control's id, rectangle in the 640x480 layer and text. Rows are
/// numbered from 0, tabs from 100, bottom buttons from 200 and the help
/// page buttons are 300 and 301.
pub(crate) type Control = (usize, (i32, i32, i32, i32), String);

/// How one user of the menu presents it.
pub struct Look {
    /// Over the top-level rows of a tab.
    pub title: &'static str,
    /// The bottom buttons, left to right.
    pub buttons: &'static [&'static str],
    /// The keyboard help lines for a tree, paged 23 to a page.
    pub help: fn(&[MenuNode]) -> Vec<String>,
}

/// What a key or click asks of the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Nothing to do, and no sound.
    None,
    /// The menu changed on its own (a help page): a click sound.
    Click,
    /// Row `index` of the rows shown was chosen: see [`PauseMenu::focus_row`].
    Select(usize),
    /// The keyboard moved to the top level of tab `index`.
    Switched(usize),
    /// Tab `index` was clicked; the caller shows it with
    /// [`PauseMenu::show_tab`] or does something else.
    Tab(usize),
    /// Bottom button `index` of [`Look::buttons`] was clicked.
    Button(usize),
}

/// Where the menu is: the tab, the submenu path, the focused row, the
/// control a mouse press went down on, and the help pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PauseMenu {
    /// The keyboard help pages are showing.
    pub help: bool,
    pub(crate) root: usize,
    pub(crate) path: Vec<usize>,
    pub(crate) focus: usize,
    pub(crate) pressed: Option<usize>,
    /// The control a right press went down on.
    pub(crate) right_pressed: Option<usize>,
    pub(crate) help_page: usize,
    /// Row labels drawn dim: rows that cannot be chosen now, with their
    /// reason beside them (the away player's Spawn in Aircraft). They still
    /// answer a choice, with the reason.
    pub dim: Vec<String>,
}

impl PauseMenu {
    /// The rows shown: the current tab's, or the open submenu's.
    pub fn rows<'a>(&self, tree: &'a [MenuNode]) -> &'a [MenuNode] {
        let mut rows = &tree[self.root].children[..];
        for &index in &self.path {
            rows = &rows[index].children;
        }
        rows
    }

    /// The first help page.
    pub fn open_help(&mut self) {
        self.help = true;
        self.help_page = 0;
    }

    /// The top level of tab `index`.
    pub fn show_tab(&mut self, index: usize) {
        self.root = index;
        self.path.clear();
        self.focus = 0;
    }

    /// Forgets a mouse press, when the window loses the pointer.
    pub fn cancel_press(&mut self) {
        self.pressed = None;
        self.right_pressed = None;
    }

    /// Focuses row `index` and returns it, or `None` past the last row.
    pub fn focus_row<'t>(&mut self, tree: &'t [MenuNode], index: usize) -> Option<&'t MenuNode> {
        self.focus = index;
        self.rows(tree).get(index)
    }

    /// Opens the submenu of row `index`.
    pub fn enter(&mut self, index: usize) {
        self.path.push(index);
        self.focus = 0;
    }

    /// Escape inside the menu: closes the help or goes up one submenu.
    /// False at a tab's top level, where Escape closes the menu.
    pub fn back(&mut self) -> bool {
        self.pressed = None;
        if self.help {
            self.help = false;
        } else if !self.path.is_empty() {
            self.path.pop();
            self.focus = 0;
        } else {
            return false;
        }
        true
    }

    /// A key while the menu is open, other than Escape: the help pages
    /// turn; the arrows, Tab, Enter and Space move through the rows and
    /// tabs.
    pub fn key(&mut self, key: &str, tree: &[MenuNode]) -> Event {
        if self.help {
            if matches!(key, "ArrowRight" | "PageDown" | "Space" | "Enter") {
                self.help_page += 1;
            }
            if matches!(key, "ArrowLeft" | "PageUp") {
                self.help_page = self.help_page.saturating_sub(1);
            }
            return Event::None;
        }
        let len = self.rows(tree).len();
        match key {
            // A tab without rows has nothing to move through or choose.
            "ArrowDown" | "ArrowUp" | "Tab" | "Enter" | "Space" if len == 0 => {}
            "ArrowDown" | "Tab" => self.focus = (self.focus + 1) % len,
            "ArrowUp" => self.focus = (self.focus + len - 1) % len,
            "ArrowRight" => {
                if len > 0 && !self.rows(tree)[self.focus].children.is_empty() {
                    return Event::Select(self.focus);
                }
                self.show_tab((self.root + 1) % tree.len());
                return Event::Switched(self.root);
            }
            "ArrowLeft" => {
                if self.path.pop().is_none() {
                    self.root = (self.root + tree.len() - 1) % tree.len();
                }
                self.focus = 0;
                if self.path.is_empty() {
                    return Event::Switched(self.root);
                }
            }
            "Enter" | "Space" => return Event::Select(self.focus),
            _ => {}
        }
        Event::None
    }

    /// The tab strip, the rows with `state` (On/Off) or their shortcut
    /// beside them, and the bottom `buttons`; or the help page buttons.
    pub(crate) fn controls(
        &self,
        tree: &[MenuNode],
        buttons: &[&str],
        state: &dyn Fn(&str) -> Option<&'static str>,
    ) -> Vec<Control> {
        let mut out = vec![];
        if self.help {
            return vec![
                (300, (480, 430, 140, 24), "Next page / Enter".into()),
                (301, (20, 430, 140, 24), "Back / Escape".into()),
            ];
        }
        let mut x = 4;
        for (i, n) in tree.iter().enumerate() {
            let w = n.label.len() as i32 * 7 + 16;
            out.push((100 + i, (x, 2, w, 22), n.label.clone()));
            x += w;
        }
        let rows = self.rows(tree);
        for (i, n) in rows.iter().enumerate() {
            let label = display_label(&n.label);
            out.push((
                i,
                (142, 50 + i as i32 * 19, 356, 19),
                format!(
                    "{label}  {}{}",
                    state(&n.label).unwrap_or(&n.shortcut),
                    if n.children.is_empty() { "" } else { " >" }
                ),
            ));
        }
        for (i, label) in buttons.iter().enumerate() {
            out.push((
                200 + i,
                (10 + i as i32 * 210, 448, 200, 23),
                (*label).into(),
            ));
        }
        out
    }

    /// The control at `point` in the 640x480 layer.
    fn hit(&self, tree: &[MenuNode], look: &Look, point: Option<(f64, f64)>) -> Option<usize> {
        let (x, y) = point?;
        self.controls(tree, look.buttons, &|_| None)
            .into_iter()
            .find_map(|(id, (rx, ry, w, h), _)| {
                ((rx as f64..(rx + w) as f64).contains(&x)
                    && (ry as f64..(ry + h) as f64).contains(&y))
                .then_some(id)
            })
    }

    /// The right button went down or up at `point`. A row acts when the
    /// press and the release are both on it; the caller turns the row's value
    /// back (the left button turns it forward, see [`Event::Select`]). Only a
    /// row answers: `None` for a tab, a bottom button or empty ground.
    pub fn right_pointer(
        &mut self,
        tree: &[MenuNode],
        look: &Look,
        point: Option<(f64, f64)>,
        down: bool,
    ) -> Option<usize> {
        let hit = self.hit(tree, look, point);
        if down {
            self.pressed = None;
            self.right_pressed = hit.filter(|id| *id < 100);
            return None;
        }
        let pressed = self.right_pressed.take();
        if self.help {
            return None;
        }
        pressed.filter(|id| Some(*id) == hit)
    }

    /// The left button went down or up at `point` in the 640x480 layer. A
    /// control acts when the press and the release are both on it.
    pub fn pointer(
        &mut self,
        tree: &[MenuNode],
        look: &Look,
        point: Option<(f64, f64)>,
        down: bool,
    ) -> Event {
        let hit = self.hit(tree, look, point);
        if down {
            self.pressed = hit;
            return Event::None;
        }
        let pressed = self.pressed.take();
        if hit != pressed {
            return Event::None;
        }
        match hit {
            Some(300) => {
                self.help_page += 1;
                Event::Click
            }
            Some(301) => {
                self.help = false;
                Event::Click
            }
            Some(id) if id >= 200 => Event::Button(id - 200),
            Some(id) if id >= 100 => Event::Tab(id - 100),
            Some(id) => Event::Select(id),
            None => Event::None,
        }
    }

    /// Draws the menu into a 640x480 layer. `state` gives a row's On/Off
    /// readout, shown in place of its shortcut.
    pub fn draw(
        &self,
        pixels: &mut [u8],
        font: &Font,
        tree: &[MenuNode],
        look: &Look,
        state: &dyn Fn(&str) -> Option<&'static str>,
    ) {
        Canvas(pixels).rect((0, 0, 640, 26), [200, 207, 219, 255]);
        if self.help {
            Canvas(pixels).rect((10, 40, 620, 398), [24, 34, 45, 255]);
            let lines = (look.help)(tree);
            let pages = lines.len().div_ceil(23).max(1);
            let start = (self.help_page % pages) * 23;
            let mut p = Paint {
                pixels,
                clip: (16, 44, 608, 390),
                color: [223, 233, 240, 255],
            };
            for (i, line) in lines.iter().skip(start).take(23).enumerate() {
                p.text(font, line, 20, 50 + i as i32 * 16);
            }
        } else {
            Canvas(pixels).rect(
                (138, 28, 364, 22 + self.rows(tree).len() as i32 * 19),
                [160, 172, 186, 255],
            );
            let mut p = Paint {
                pixels,
                clip: (0, 0, 640, 480),
                color: [15, 30, 50, 255],
            };
            p.text(
                font,
                if self.path.is_empty() {
                    look.title
                } else {
                    "SUBMENU - Left / Escape to go back"
                },
                146,
                34,
            );
        }
        for (id, r, label) in self.controls(tree, look.buttons, state) {
            let selected = (!self.help && (id == self.focus || id == 100 + self.root))
                || self.pressed == Some(id);
            Canvas(pixels).rect(
                r,
                if selected {
                    [62, 86, 118, 255]
                } else {
                    [201, 210, 222, 255]
                },
            );
            let dimmed = id < 100
                && !self.help
                && self
                    .rows(tree)
                    .get(id)
                    .is_some_and(|n| self.dim.contains(&n.label));
            let mut p = Paint {
                pixels,
                clip: r,
                color: match (selected, dimmed) {
                    (true, true) => [160, 172, 190, 255],
                    (true, false) => [247, 250, 255, 255],
                    (false, true) => [110, 124, 146, 255],
                    (false, false) => [20, 39, 65, 255],
                },
            };
            p.text(font, &label, r.0 + 5, r.1 + 6);
        }
    }
}

/// How a row's imported label reads on screen. The retail `Exit to Windows` is
/// shown as `Exit to Desktop` (John's wording, 2026-09-29); the imported label
/// stays the row's identity in the tree and in the dispatch.
pub fn display_label(label: &str) -> &str {
    if label == "Exit to Windows" {
        "Exit to Desktop"
    } else {
        label
    }
}

/// Every row in `tree` with a shortcut, as help lines `shortcut: label`.
pub fn shortcut_lines(tree: &[MenuNode], lines: &mut Vec<String>) {
    for n in tree {
        if !n.shortcut.is_empty() {
            lines.push(format!("{}: {}", n.shortcut, display_label(&n.label)));
        }
        shortcut_lines(&n.children, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(label: &str) -> MenuNode {
        MenuNode {
            label: label.into(),
            shortcut: String::new(),
            children: vec![],
        }
    }

    fn tree() -> Vec<MenuNode> {
        vec![
            MenuNode {
                label: "?".into(),
                shortcut: String::new(),
                children: vec![leaf("End"), leaf("Exit to Windows")],
            },
            MenuNode {
                label: "Pref".into(),
                shortcut: String::new(),
                children: vec![MenuNode {
                    label: "Time".into(),
                    shortcut: String::new(),
                    children: vec![leaf("1x"), leaf("2x")],
                }],
            },
        ]
    }

    const LOOK: Look = Look {
        title: "PAUSED",
        buttons: &["Resume", "Help"],
        help: |_| (0..30).map(|n| format!("Line {n}")).collect(),
    };

    #[test]
    fn keys_walk_rows_tabs_and_submenus() {
        let t = tree();
        let mut m = PauseMenu::default();
        assert_eq!(m.key("ArrowDown", &t), Event::None);
        assert_eq!(m.key("Enter", &t), Event::Select(1));
        assert_eq!(m.key("ArrowRight", &t), Event::Switched(1));
        assert_eq!(m.key("ArrowRight", &t), Event::Select(0));
        m.enter(0);
        assert_eq!(m.rows(&t)[1].label, "2x");
        assert_eq!(m.key("ArrowLeft", &t), Event::Switched(1));
        assert!(m.path.is_empty());
        m.enter(0);
        assert!(m.back());
        assert!(!m.back());
        m.open_help();
        assert_eq!(m.key("ArrowRight", &t), Event::None);
        assert_eq!(m.help_page, 1);
        assert!(m.back());
        assert!(!m.help);
    }

    #[test]
    fn clicks_need_the_press_and_release_on_one_control() {
        let t = tree();
        let mut m = PauseMenu::default();
        // Row 1 of the first tab, then the second tab, then the second
        // button.
        for (point, event) in [
            ((160., 75.), Event::Select(1)),
            ((30., 10.), Event::Tab(1)),
            ((230., 455.), Event::Button(1)),
        ] {
            m.pointer(&t, &LOOK, Some(point), true);
            assert_eq!(m.pointer(&t, &LOOK, Some(point), false), event);
        }
        m.pointer(&t, &LOOK, Some((160., 75.)), true);
        assert_eq!(m.pointer(&t, &LOOK, Some((160., 55.)), false), Event::None);
    }

    #[test]
    fn a_right_click_answers_only_for_a_row_pressed_and_released_on_it() {
        let t = tree();
        let mut m = PauseMenu::default();
        // Row 1 of the first tab: press and release on it.
        let row = Some((160., 75.));
        assert_eq!(m.right_pointer(&t, &LOOK, row, true), None);
        assert_eq!(m.right_pointer(&t, &LOOK, row, false), Some(1));
        // Released on another row, or off the menu, it does not.
        m.right_pointer(&t, &LOOK, row, true);
        assert_eq!(m.right_pointer(&t, &LOOK, Some((160., 55.)), false), None);
        m.right_pointer(&t, &LOOK, row, true);
        assert_eq!(m.right_pointer(&t, &LOOK, None, false), None);
        // A tab and a bottom button take no right click.
        for point in [(30., 10.), (230., 455.)] {
            m.right_pointer(&t, &LOOK, Some(point), true);
            assert_eq!(m.right_pointer(&t, &LOOK, Some(point), false), None);
        }
        // A left press is forgotten when the right one goes down, and the
        // right press when the window loses the pointer.
        m.pointer(&t, &LOOK, row, true);
        m.right_pointer(&t, &LOOK, row, true);
        assert_eq!(m.pressed, None);
        m.cancel_press();
        assert_eq!(m.right_pointer(&t, &LOOK, row, false), None);
    }

    #[test]
    fn the_retail_exit_row_reads_exit_to_desktop_everywhere_it_is_shown() {
        let mut t = tree();
        t[0].children[1].shortcut = "Alt-F4".into();
        let mut lines = Vec::new();
        shortcut_lines(&t, &mut lines);
        assert_eq!(lines, ["Alt-F4: Exit to Desktop"]);
        let m = PauseMenu::default();
        let labels: Vec<String> = m
            .controls(&t, &[], &|_| None)
            .into_iter()
            .map(|c| c.2)
            .collect();
        assert!(labels.iter().any(|l| l.starts_with("Exit to Desktop")));
        assert!(labels.iter().all(|l| !l.contains("Windows")));
    }

    #[test]
    fn the_title_and_state_readouts_are_the_callers() {
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![(0, 0)],
                })
                .collect(),
        };
        let t = tree();
        let m = PauseMenu::default();
        let draw = |state: &dyn Fn(&str) -> Option<&'static str>| {
            let mut pixels = vec![0u8; 640 * 480 * 4];
            m.draw(&mut pixels, &font, &t, &LOOK, state);
            pixels
        };
        let plain = draw(&|_| None);
        let on = draw(&|label| (label == "End").then_some("On"));
        assert_ne!(plain, on);
        // The bottom buttons and the tab strip are opaque.
        let at = |pixels: &[u8], x: usize, y: usize| pixels[(y * 640 + x) * 4 + 3];
        assert_eq!(at(&plain, 300, 460), 255);
        assert_eq!(at(&plain, 630, 10), 255);
        assert_eq!(at(&plain, 630, 460), 0);
    }
}
