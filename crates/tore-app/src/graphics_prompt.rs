//! The way out for a player whose game dies during GPU setup: later launches skip
//! the import screen and never reach Pref > Graphics, so a launch that finds a dead
//! marker (`crate::graphics_api`) asks here first, on the same canvas path as the
//! import screen, drawn with the API that did not die. Nothing switches without the
//! player. Requested on 2026-11-02; the layout and
//! wording are agent decisions. docs/spec/graphics-options.md holds the contract.
use crate::canvas_present::CanvasPresenter;
use crate::graphics::GraphicsApi;
use crate::menu::{Canvas, HEIGHT, Sprite, WIDTH, flat_font_large};
use crate::{AppResult, diagnostics};
use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{Window, WindowId},
};

const TITLE: &str = "Graphics setup";
const SENTENCE: &str =
    "The last launch did not finish graphics setup, so you can choose the graphics API here.";

const PANEL: (i32, i32, i32, i32) = (80, 120, 480, 240);
const INNER_X: i32 = PANEL.0 + 20;
const INNER_W: i32 = PANEL.2 - 40;
const TITLE_Y: i32 = PANEL.1 + 10;
const TEXT_Y: i32 = PANEL.1 + 42;
const LINE: i32 = 18;
const CHOICE_W: i32 = 150;
const CHOICE_Y: i32 = PANEL.1 + 124;
const CHOICE_H: i32 = 30;
const CHOICES: [(i32, i32, i32, i32); 2] = [
    (
        WIDTH as i32 / 2 - CHOICE_W - 4,
        CHOICE_Y,
        CHOICE_W,
        CHOICE_H,
    ),
    (WIDTH as i32 / 2 + 4, CHOICE_Y, CHOICE_W, CHOICE_H),
];
const BUTTON_W: i32 = 150;
const BUTTON_H: i32 = 28;
const BUTTON_Y: i32 = PANEL.1 + PANEL.3 - 10 - BUTTON_H;
const CONTINUE: (i32, i32, i32, i32) = (INNER_X, BUTTON_Y, BUTTON_W, BUTTON_H);
const QUIT: (i32, i32, i32, i32) = (INNER_X + INNER_W - BUTTON_W, BUTTON_Y, BUTTON_W, BUTTON_H);

const _: () = assert!(CHOICE_Y + CHOICE_H < BUTTON_Y);

const BACKDROP: [u8; 4] = [16, 20, 26, 255];
const PANEL_FILL: [u8; 4] = [27, 36, 48, 255];
const PANEL_EDGE: [u8; 4] = [124, 144, 170, 255];
const EDGE_IDLE: [u8; 4] = [86, 101, 122, 255];
const EDGE_FOCUS: [u8; 4] = [156, 194, 238, 255];
const SELECTED: [u8; 4] = [50, 72, 100, 255];
const UNSELECTED: [u8; 4] = [32, 40, 51, 255];
const BUTTON_FILL: [u8; 4] = [44, 60, 80, 255];
const BUTTON_ACTIVE: [u8; 4] = [68, 94, 126, 255];
const DIM: [u8; 3] = [206, 216, 232];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Choice,
    Continue,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Continue(GraphicsApi),
    Quit,
}

fn in_rect(x: f64, y: f64, (rx, ry, rw, rh): (i32, i32, i32, i32)) -> bool {
    x >= rx as f64 && y >= ry as f64 && x < (rx + rw) as f64 && y < (ry + rh) as f64
}

pub(crate) struct Prompt {
    font: Sprite,
    api: GraphicsApi,
    focus: Focus,
    hover: Option<(i32, i32, i32, i32)>,
}

impl Prompt {
    pub(crate) fn new(api: GraphicsApi) -> Self {
        Self {
            font: flat_font_large([255, 255, 255]),
            api,
            focus: Focus::Continue,
            hover: None,
        }
    }
    fn toggle(&mut self) {
        self.api = match self.api {
            GraphicsApi::Dx12 => GraphicsApi::Vulkan,
            _ => GraphicsApi::Dx12,
        };
    }
    /// Key names as the import screen gets them.
    fn key(&mut self, name: &str) -> Option<Outcome> {
        match (name, self.focus) {
            ("Escape", _) => return Some(Outcome::Quit),
            ("Enter", Focus::Quit) => return Some(Outcome::Quit),
            ("Enter", Focus::Continue) => return Some(Outcome::Continue(self.api)),
            ("Enter" | "ArrowLeft" | "ArrowRight", Focus::Choice) => self.toggle(),
            ("Tab", Focus::Choice) => self.focus = Focus::Continue,
            ("Tab", Focus::Continue) => self.focus = Focus::Quit,
            ("Tab", Focus::Quit) => self.focus = Focus::Choice,
            _ => {}
        }
        None
    }
    fn hover(&mut self, x: f64, y: f64) {
        self.hover = CHOICES
            .into_iter()
            .chain([CONTINUE, QUIT])
            .find(|rect| in_rect(x, y, *rect));
    }
    fn click(&mut self, x: f64, y: f64) -> Option<Outcome> {
        if let Some(index) = CHOICES.iter().position(|rect| in_rect(x, y, *rect)) {
            self.api = GraphicsApi::CHOICES[index];
            self.focus = Focus::Choice;
        } else if in_rect(x, y, CONTINUE) {
            return Some(Outcome::Continue(self.api));
        } else if in_rect(x, y, QUIT) {
            return Some(Outcome::Quit);
        }
        None
    }
    pub(crate) fn draw(&self, pixels: &mut [u8]) {
        assert_eq!(pixels.len(), WIDTH * HEIGHT * 4);
        let font = &self.font;
        let mut canvas = Canvas(pixels);
        canvas.rect((0, 0, WIDTH as i32, HEIGHT as i32), BACKDROP);
        canvas.rect(PANEL, PANEL_FILL);
        canvas.outline(PANEL, PANEL_EDGE);
        canvas.text(font, TITLE, INNER_X, TITLE_Y, None);
        canvas.rect((INNER_X, TITLE_Y + 21, INNER_W, 1), PANEL_EDGE);
        for (row, line) in crate::locate::wrap(font, SENTENCE, INNER_W)
            .iter()
            .enumerate()
        {
            canvas.text(font, line, INNER_X, TEXT_Y + row as i32 * LINE, Some(DIM));
        }
        for (index, rect) in CHOICES.into_iter().enumerate() {
            let api = GraphicsApi::CHOICES[index];
            let chosen = self.api == api;
            canvas.rect(rect, if chosen { SELECTED } else { UNSELECTED });
            canvas.outline(
                rect,
                if chosen && self.focus == Focus::Choice {
                    EDGE_FOCUS
                } else if chosen {
                    PANEL_EDGE
                } else {
                    EDGE_IDLE
                },
            );
            canvas.centered_text(font, api.label(), rect);
        }
        for (label, rect, focus) in [
            ("Continue", CONTINUE, Focus::Continue),
            ("Quit", QUIT, Focus::Quit),
        ] {
            let active = self.focus == focus || self.hover == Some(rect);
            canvas.rect(rect, if active { BUTTON_ACTIVE } else { BUTTON_FILL });
            canvas.outline(
                rect,
                if self.focus == focus {
                    EDGE_FOCUS
                } else {
                    EDGE_IDLE
                },
            );
            canvas.centered_text(font, label, rect);
        }
    }
}

/// Shows the prompt until the player continues or quits.
pub(crate) fn run(
    event_loop: &mut EventLoop<()>,
    api: GraphicsApi,
    fullscreen: bool,
) -> AppResult<Outcome> {
    let mut shell = Shell {
        prompt: Prompt::new(api),
        presenter: None,
        pixels: vec![0; WIDTH * HEIGHT * 4],
        fullscreen,
        last_pointer: None,
        retry_at: None,
        outcome: Outcome::Quit,
        error: None,
    };
    event_loop.run_app_on_demand(&mut shell)?;
    match shell.error {
        Some(error) => Err(error),
        None => Ok(shell.outcome),
    }
}

struct Shell {
    prompt: Prompt,
    presenter: Option<CanvasPresenter>,
    pixels: Vec<u8>,
    fullscreen: bool,
    last_pointer: Option<(f64, f64)>,
    retry_at: Option<Instant>,
    outcome: Outcome,
    error: Option<Box<dyn std::error::Error>>,
}

impl Shell {
    fn settle(&mut self, event_loop: &ActiveEventLoop, outcome: Option<Outcome>) {
        if let Some(outcome) = outcome {
            self.outcome = outcome;
            event_loop.exit();
        }
        if let Some(presenter) = &self.presenter {
            presenter.window.request_redraw();
        }
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.presenter.is_some() {
            return;
        }
        let result = (|| -> AppResult<_> {
            diagnostics::stage("graphics choice window creation");
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("T.O.R.E-Fighters - Graphics setup")
                        .with_inner_size(LogicalSize::new(960.0, 720.0))
                        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
                        .with_fullscreen(crate::fullscreen_attribute(self.fullscreen)),
                )?,
            );
            diagnostics::stage_done();
            pollster::block_on(CanvasPresenter::new(window))
        })();
        match result {
            Ok(presenter) => {
                presenter.window.request_redraw();
                self.presenter = Some(presenter);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(presenter) = self.presenter.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.outcome = Outcome::Quit;
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.last_pointer = None;
                presenter.resize();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if let Some(name) = crate::locate_key_name(&event.logical_key) {
                    let outcome = self.prompt.key(name);
                    self.settle(event_loop, outcome);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_pointer = presenter.viewport().point(position.x, position.y);
                if let Some((x, y)) = self.last_pointer {
                    self.prompt.hover(x, y);
                    presenter.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some((x, y)) = self.last_pointer {
                    let outcome = self.prompt.click(x, y);
                    self.settle(event_loop, outcome);
                }
            }
            WindowEvent::RedrawRequested => {
                self.prompt.draw(&mut self.pixels);
                if let Err(error) = presenter.present(&self.pixels) {
                    self.error = Some(error);
                    event_loop.exit();
                    return;
                }
                self.retry_at = presenter.surface_retry().map(|wait| Instant::now() + wait);
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.retry_at.is_some_and(|at| Instant::now() >= at) {
            self.retry_at = None;
            if let Some(presenter) = &self.presenter {
                presenter.window.request_redraw();
            }
        }
        event_loop.set_control_flow(match self.retry_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // As the import screen does: release the GPU backend with the display alive.
        self.presenter = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::text_width;

    fn center((x, y, w, h): (i32, i32, i32, i32)) -> (f64, f64) {
        ((x + w / 2) as f64, (y + h / 2) as f64)
    }

    #[test]
    fn continuing_keeps_what_was_showing_unless_the_player_picks_another() {
        let mut p = Prompt::new(GraphicsApi::Vulkan);
        assert_eq!(
            p.key("Enter"),
            Some(Outcome::Continue(GraphicsApi::Vulkan)),
            "Continue has the focus, so Enter takes the shown choice"
        );
        let (x, y) = center(CHOICES[1]);
        assert_eq!(p.click(x, y), None);
        let (x, y) = center(CONTINUE);
        assert_eq!(p.click(x, y), Some(Outcome::Continue(GraphicsApi::Dx12)));
    }

    #[test]
    fn the_keyboard_reaches_everything() {
        let mut p = Prompt::new(GraphicsApi::Dx12);
        p.key("Tab");
        assert_eq!(p.focus, Focus::Quit);
        assert_eq!(p.key("Enter"), Some(Outcome::Quit));
        p.key("Tab");
        assert_eq!(p.focus, Focus::Choice);
        assert_eq!(p.key("ArrowLeft"), None);
        assert_eq!(p.api, GraphicsApi::Vulkan);
        assert_eq!(p.key("Enter"), None);
        assert_eq!(p.api, GraphicsApi::Dx12);
        p.key("Tab");
        assert_eq!(p.key("Enter"), Some(Outcome::Continue(GraphicsApi::Dx12)));
        assert_eq!(p.key("Escape"), Some(Outcome::Quit));
    }

    #[test]
    fn the_text_fits_above_the_choices_and_inside_the_panel() {
        let p = Prompt::new(GraphicsApi::Vulkan);
        let lines = crate::locate::wrap(&p.font, SENTENCE, INNER_W);
        assert!(TEXT_Y + lines.len() as i32 * LINE <= CHOICE_Y, "{lines:?}");
        for api in GraphicsApi::CHOICES {
            assert!(text_width(&p.font, api.label()) + 4 <= CHOICE_W);
        }
        let mut pixels = vec![0; WIDTH * HEIGHT * 4];
        p.draw(&mut pixels);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[0] == 255));
    }
}
