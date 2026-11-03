//! Media-free diagnostic entry points and unattended error-dialog policy.
//! Opinionated host behavior: docs/spec/startup-diagnostics.md.
use crate::{AppResult, canvas_present::CanvasPresenter, diagnostics};
use std::{
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{Window, WindowId},
};

pub(crate) fn interactive() -> bool {
    if std::env::var_os("TORE_NO_ERROR_DIALOG").is_some_and(|v| v == "1") {
        return false;
    }
    if [
        "TORE_ENVIRONMENT_PROBE",
        "TORE_AIRPORT_PROBE",
        "TORE_PERF_FRAMES",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
    {
        return false;
    }
    interactive_arguments(std::env::args_os().skip(1))
}

fn interactive_arguments(args: impl Iterator<Item = OsString>) -> bool {
    !args.filter_map(|arg| arg.into_string().ok()).any(|arg| {
        (arg.starts_with("--diagnostics-self-test") && arg != "--diagnostics-self-test=dialog")
            || matches!(
                arg.as_str(),
                "--help"
                    | "-h"
                    | "--version"
                    | "-V"
                    | "--import-only"
                    | "--headless-flight"
                    | "--reel-render"
                    | "--reel-music"
                    | "--snapshot"
                    | "--panel-snapshot"
                    | "--capture-terrain"
                    | "--capture-flight"
                    | "--smoke-test"
                    | "--replay-input"
                    | "--replay-combat"
                    | "--combat-smoke"
                    | "--combat-probe-ticks"
                    | "--missile-acceptance"
                    | "--missile-study"
                    | "--ai-probe-ticks"
                    | "--ai-roster-probe-ticks"
                    | "--rotor-probe"
                    | "--sensor-summary"
                    | "--validate-creator"
                    | "--validate-weather"
                    | "--validate-maps"
                    | "--native-flight-report"
                    | "--list-inputs"
                    | "--monitor-inputs"
                    | "--test-rumble"
                    | "--write-input-profile"
                    | "--airport-probe"
            )
    })
}

pub(crate) fn self_test() -> Option<AppResult<()>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = args.iter().filter_map(|s| s.to_str()).find_map(|s| {
        if s == "--diagnostics-self-test" {
            Some("success")
        } else {
            s.strip_prefix("--diagnostics-self-test=")
        }
    })?;
    Some(self_test_api(&args).and_then(|()| run_self_test(mode)))
}

/// The self-test stands alone, except that `--graphics-api VALUE` may come with
/// it, so a media-free run can check an API.
fn self_test_api(args: &[OsString]) -> AppResult<()> {
    let flag = match args {
        [_] => None,
        [first, value, _] if first == "--graphics-api" => Some(
            crate::graphics::GraphicsApi::from_flag(&value.to_string_lossy())?,
        ),
        [_, first, value] if first == "--graphics-api" => Some(
            crate::graphics::GraphicsApi::from_flag(&value.to_string_lossy())?,
        ),
        _ => {
            return Err("--diagnostics-self-test must be used alone or with --graphics-api".into());
        }
    };
    crate::graphics_api::select(
        std::path::Path::new(""),
        flag,
        crate::graphics::GraphicsApi::Auto,
        false,
        false,
    );
    Ok(())
}

fn run_self_test(mode: &str) -> AppResult<()> {
    diagnostics::stage(match mode {
        "native-crash" => "diagnostics self-test: deliberate native crash",
        "native-crash-window" => "diagnostics self-test: deliberate native crash in a window",
        "stack-overflow" => "diagnostics self-test: deliberate stack overflow",
        _ => "diagnostics self-test",
    });
    log::info!("Deliberate diagnostics self-test: {mode}");
    match mode {
        "success" => {
            diagnostics::stage_done();
            println!("Diagnostics self-test: PASS; log={:?}", diagnostics::log_path());
            Ok(())
        }
        "error" | "dialog" => Err(std::io::Error::other(
            "Deliberate diagnostics self-test error; no game data was loaded",
        ).into()),
        "panic" => panic!("Deliberate diagnostics self-test panic"),
        "worker-panic" => {
            let worker = std::thread::Builder::new().name("diagnostics-test-worker".into())
                .spawn(|| panic!("Deliberate diagnostics self-test worker panic"))?;
            if worker.join().is_err() {
                return Err("Deliberate diagnostics self-test worker panic was recorded".into());
            }
            Err("diagnostics worker unexpectedly returned normally".into())
        }
        "graphics" => graphics_self_test(),
        "native-crash" => {
            // Names what the crash report will say it was doing.
            tore_diagnostics_native::crash_context::GPU_STEP
                .set("deliberate self-test, no GPU step");
            tore_diagnostics_native::deliberate_crash(
                tore_diagnostics_native::DeliberateCrash::AccessViolation,
            )
            .map_err(Into::into)
        }
        "stack-overflow" => {
            tore_diagnostics_native::crash_context::GPU_STEP
                .set("deliberate self-test, no GPU step");
            tore_diagnostics_native::deliberate_crash(
                tore_diagnostics_native::DeliberateCrash::StackOverflow,
            )
            .map_err(Into::into)
        }
        "native-crash-window" => native_crash_window(),
        _ => Err("--diagnostics-self-test expects success, error, panic, worker-panic, graphics, dialog, native-crash, native-crash-window or stack-overflow".into()),
    }
}

/// The same crash, from inside a window callback with Windows' own message
/// dispatch on the stack: the path a lazily built pipeline would crash on. Windows
/// treats exceptions crossing that boundary specially, so this one is worth seeing
/// directly. Needs a display.
fn native_crash_window() -> AppResult<()> {
    if !cfg!(target_os = "windows") {
        tore_diagnostics_native::deliberate_crash(
            tore_diagnostics_native::DeliberateCrash::AccessViolation,
        )?;
    }
    diagnostics::stage("diagnostic event loop creation");
    let mut event_loop = EventLoop::new()?;
    diagnostics::stage("diagnostics self-test: deliberate native crash in a window");
    event_loop.run_app_on_demand(&mut CrashInWindow { window: None })?;
    Err("diagnostic window closed before its deliberate native crash".into())
}

struct CrashInWindow {
    window: Option<Window>,
}

impl ApplicationHandler for CrashInWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        match event_loop.create_window(
            Window::default_attributes().with_title("T.O.R.E-Fighters deliberate crash test"),
        ) {
            Ok(window) => {
                window.request_redraw();
                self.window = Some(window);
            }
            Err(error) => {
                log::error!("Diagnostic window unavailable: {error}");
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Err(error) = tore_diagnostics_native::deliberate_crash(
                    tore_diagnostics_native::DeliberateCrash::AccessViolation,
                ) {
                    log::error!("{error}");
                    event_loop.exit();
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn graphics_self_test() -> AppResult<()> {
    diagnostics::stage("diagnostic event loop creation");
    let mut event_loop = EventLoop::new()?;
    diagnostics::stage_done();
    let mut app = GraphicsCheck {
        presenter: None,
        error: None,
        presented: false,
        deadline: Instant::now() + Duration::from_secs(15),
    };
    event_loop.run_app_on_demand(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    if !app.presented {
        return Err("diagnostic graphics window closed before a frame was presented".into());
    }
    log::info!("Diagnostics graphics self-test: PASS");
    Ok(())
}

struct GraphicsCheck {
    presenter: Option<CanvasPresenter>,
    error: Option<Box<dyn std::error::Error>>,
    presented: bool,
    deadline: Instant,
}

impl ApplicationHandler for GraphicsCheck {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.presenter.is_some() {
            return;
        }
        let result = (|| -> AppResult<_> {
            diagnostics::stage("diagnostic window creation");
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("T.O.R.E-Fighters graphics diagnostic")
                        .with_inner_size(LogicalSize::new(640., 480.)),
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                let Some(presenter) = &mut self.presenter else {
                    return;
                };
                let mut pixels = vec![0; 640 * 480 * 4];
                for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                    pixel.copy_from_slice(&[32, (i % 640 / 3) as u8, (i / 640 / 2) as u8, 255]);
                }
                match presenter.present(&pixels) {
                    Ok(true) => {
                        self.presented = true;
                        event_loop.exit();
                    }
                    Ok(false) => presenter.window.request_redraw(),
                    Err(error) => {
                        self.error = Some(error);
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.deadline {
            self.error = Some("diagnostic graphics presentation timed out after 15 seconds".into());
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_self_test_stands_alone_except_for_the_graphics_api() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        let test = "--diagnostics-self-test";
        assert!(self_test_api(&args(&[test])).is_ok());
        assert!(self_test_api(&args(&[test, "--graphics-api", "auto"])).is_ok());
        assert!(self_test_api(&args(&["--graphics-api", "auto", test])).is_ok());
        assert!(self_test_api(&args(&[test, "--graphics-api", "opengl"])).is_err());
        assert!(self_test_api(&args(&[test, "--windowed"])).is_err());
    }

    #[test]
    fn unattended_modes_never_request_dialogs_even_with_malformed_arguments() {
        for mode in [
            "--headless-flight",
            "--snapshot",
            "--import-only",
            "--capture-flight",
            "--smoke-test",
            "--version",
            "--help",
            "--list-inputs",
            "--diagnostics-self-test=panic",
            "--diagnostics-self-test=graphics",
            "--diagnostics-self-test=native-crash",
            "--diagnostics-self-test=stack-overflow",
        ] {
            assert!(
                !interactive_arguments([mode, "invalid"].into_iter().map(OsString::from)),
                "{mode}"
            );
        }
        assert!(interactive_arguments(std::iter::empty()));
        assert!(interactive_arguments(
            [OsString::from("--diagnostics-self-test=dialog")].into_iter()
        ));
        assert!(interactive_arguments(
            [OsString::from("--windowed")].into_iter()
        ));
    }
}

/// Track first presentation without emitting a record for every redraw retry.
#[derive(Default)]
pub(crate) enum FirstFrame {
    #[default]
    Pending,
    Waiting,
    Presented,
}
impl FirstFrame {
    pub(crate) fn begin(&mut self, stage: &'static str) {
        if matches!(self, Self::Pending) {
            diagnostics::stage(stage);
            *self = Self::Waiting;
        }
    }
    /// True the one time the first frame is recorded.
    pub(crate) fn complete(&mut self) -> bool {
        if matches!(self, Self::Waiting) {
            diagnostics::stage_done();
            log::info!("First frame presented successfully");
            *self = Self::Presented;
            return true;
        }
        false
    }
}
