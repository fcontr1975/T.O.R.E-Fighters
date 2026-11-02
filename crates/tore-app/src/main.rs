// Windows release launches have no console. Diagnostics are initialized inside
// the executable before startup and fatal interactive errors use an OS dialog.
// CLI/probe output stays on stdout; startup diagnostics also go to session logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod a10_animation;
mod a2g_radar;
mod a2g_run;
mod a310_animation;
mod a4e_animation;
mod a7_animation;
mod ac130_animation;
mod additional_animation;
mod ai_roster_probe;
mod aim_box;
mod aircraft;
mod aircraft_animation;
mod aircraft_animation_probe;
mod airfield_audit;
mod airfield_lights;
mod airfield_sheets;
mod assets;
mod attitude;
mod audio;
mod av8_animation;
mod awacs_animation;
mod b747_animation;
mod blast_climate;
mod blast_preview;
mod bridge_audit;
mod c130_animation;
mod camera;
mod canvas_present;
mod canvas_rows;
mod celestial;
mod city_fill_dump;
mod city_fires;
mod clouds;
mod cockpit_lighting;
mod cockpit_renderer;
mod cockpit_sheet;
mod combat_smoke;
mod combat_view;
mod console_keys;
mod controls_editor;
mod countermeasure_renderer;
mod crt;
mod damage_art;
mod debrief;
mod diagnostics;
mod direct_screen;
mod e2_animation;
mod effect_renderer;
mod ejection_art;
mod engine_material;
mod f104_animation;
mod f14_animation;
mod f14_geometry;
mod f15_animation;
mod f16_animation;
mod f22_animation;
mod f4_animation;
mod feedback;
mod fill_renderer;
mod flak_pass;
mod flight;
mod flight_canvas;
mod flight_map;
mod flight_map_preview;
mod flight_music;
mod flight_probe;
mod flight_ui;
mod flight_views;
mod flight_watch;
mod formation_trace;
mod frame_gpu;
mod frame_log;
mod frame_pacing;
mod frame_rate;
mod gpu_setup;
mod graphics;
mod graphics_api;
mod graphics_screen;
mod gun_flash;
mod gun_flash_preview;
mod gunsight_probe;
mod gunsight_view;
mod hud;
mod hud_aperture;
mod il76_animation;
mod ils_survey;
mod impact_fx;
mod impact_preview;
mod input;
mod input_catalog;
mod input_script;
mod instruments;
mod internet_screen;
mod lens_flare;
mod loading;
mod lobby_screen;
mod locate;
mod look;
mod menu;
mod mi24_animation;
mod mig17_animation;
mod mig21_animation;
mod mig23_animation;
mod mig29_animation;
mod mirrors;
mod missile_acceptance;
mod missile_sag;
mod missile_study;
mod mission_brief;
mod mission_timer;
mod multiplayer_menu;
mod navigation;
mod net;
mod night_glow;
mod night_sky;
mod ocean;
mod ordnance;
mod ordnance_audit;
mod pause_menu;
mod performance;
mod powered_hud;
mod preferences;
mod probe_invariants;
mod pylons;
mod qmc_distance_table;
mod quick_mission;
mod rafale_animation;
mod reel;
mod regen;
mod render_snapshot;
mod renderer;
mod replay;
mod rocker;
mod roster_animation;
mod rwr_tone;
mod scenery;
mod scope;
mod seeker_view;
mod sensor_console;
mod sim_renderer;
mod sky_mix;
mod smoke_renderer;
mod sound_prefs;
mod sound_screen;
mod startup;
mod static_art;
mod store_sizes;
mod stores_preview;
mod su25_animation;
mod su27_animation;
mod su35_animation;
mod surface_battle;
mod surface_drive;
mod surface_dump;
mod surface_fx;
mod surface_fx_preview;
mod surface_lighting;
mod surface_objective;
mod surface_parked;
mod surface_preview;
mod surface_scene;
mod surface_trace;
mod tape_file;
mod target_info;
mod target_preview;
mod target_select;
mod terrain_audit;
mod terrain_chunks;
mod terrain_colors;
mod terrain_seams;
mod ui_text;
mod ui_text_renderer;
mod updates;
mod v22_animation;
mod variety_animation;
mod variety_rotors;
mod vegetation;
mod vegetation_limits;
mod vegetation_pages;
mod vegetation_preview;
mod vegetation_renderer;
mod vegetation_shapes;
mod version;
mod view_compass;
mod weapon_hud;
mod weather;
mod widgets;
mod wing_loadout_probe;
mod x31_animation;
mod yak141_animation;

// The mission core lives in tore-world; these keep the app's module paths.
pub(crate) use tore_world::{
    ai_wings, aircraft_type, airfield_radio, combat, combat_tape, comms, crew_voice, frame,
    mission, mission_layout, radio_calls, seats, situation, snapshot, target_window, terrain,
    world,
};

/// The seat single player flies from.
const SEAT: seats::SeatId = seats::SeatId(0);
/// The cockpit of the plane `SEAT` flies, which the game presents.
const OWN: usize = 0;
/// The most commands the seat can have waiting for a tick.
const MAX_SEAT_COMMANDS: usize = 256;

use assets::Assets;
use menu::{Action, Menu};
use renderer::Renderer;
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tore_import::media_source;
use tore_sim::models::FlightModel;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{CursorIcon, Fullscreen, Window, WindowId},
};
use world::{airport_aircraft, step_turbulence};
type AppResult<T> = Result<T, Box<dyn Error>>;

/// A parse error for the number of a command-line option, naming the option.
fn bad_number(option: &str, error: &dyn std::fmt::Display) -> Box<dyn Error> {
    format!("{option} needs a number ({error})").into()
}

/// A number given to a command-line option, or an error that names the
/// option and what was typed rather than the parser's bare message.
fn option_number<T: std::str::FromStr>(option: &str, value: &str) -> AppResult<T> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{option} needs a number, not '{value}'").into())
}
/// How an interactive start opens its window. Requested by John on 2026-09-22:
/// the game runs native borderless fullscreen by default on every platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowMode {
    Fullscreen,
    Windowed,
}
impl WindowMode {
    fn fullscreen(self) -> bool {
        self == WindowMode::Fullscreen
    }
}
/// The window mode shared by the locate shell and the game window, so one
/// Alt-Enter in the shell holds for the rest of the session.
struct WindowState {
    /// The mode the next window opens in.
    fullscreen: bool,
    /// What the `fullscreen` preference is saved as. Only a toggle changes it,
    /// so `--windowed` does not overwrite the player's saved choice.
    preference: bool,
}
impl WindowState {
    fn toggle(&mut self) {
        self.fullscreen = !self.fullscreen;
        self.preference = self.fullscreen;
    }
}
/// The winit setting for a window mode. `Borderless(None)` uses whichever
/// monitor the window would otherwise have opened on.
fn fullscreen_attribute(on: bool) -> Option<Fullscreen> {
    on.then_some(Fullscreen::Borderless(None))
}
/// Decide the initial window mode. Borderless fullscreen is the default; a
/// window is used when the player asked for one, when a flag fixes the window
/// size, or when the saved preference says so.
///
/// * `windowed_flag`: `--windowed` was given.
/// * `window_size_flag`: `--window-size` was given.
/// * `fixed_size`: a capture, snapshot or `--smoke-test` run, all of which
///   depend on a known window size.
/// * `preference`: the saved `fullscreen` preference, default on.
fn initial_window_mode(
    windowed_flag: bool,
    window_size_flag: bool,
    fixed_size: bool,
    preference: bool,
) -> WindowMode {
    if windowed_flag || window_size_flag || fixed_size || !preference {
        WindowMode::Windowed
    } else {
        WindowMode::Fullscreen
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Main,
    Quick,
    Viewer,
    Flight,
    /// The mission replay viewer; see replay/viewer.rs.
    Replay,
}
/// Keeps the own powered-lift aircraft `flight` at the preferred stability
/// level `wanted` (VTOL overhaul, design 5.2): a flight, a restart or a seat
/// taken over starts at Damper, and the level the player chose in Pref or
/// with Ctrl+Shift+A follows as a pilot command, so it reaches the host,
/// recordings and pilot tapes like any other.
fn sync_stability(
    flight: &tore_sim::flight::State,
    input: &mut input::Input,
    wanted: tore_input::StabilityLevel,
) {
    if flight.crashed || flight.stability_in_effect().is_none() {
        return;
    }
    if input.pending_stability(flight.lift_controls.aids.stability) != wanted {
        input.queue(tore_input::PilotCommand::Lift(
            tore_input::LiftCommand::SetStability(wanted),
        ));
    }
}
/// The most wall time the first running flight frame after a stopped one
/// turns into ticks, seconds: one frame at 30 a second.
const RESUME_MAX_ELAPSED: f64 = 1. / 30.;
/// A flight frame that took longer than this, seconds, tells the workers'
/// governor the loop is late (slice S9).
const LATE_FRAME_SECONDS: f64 = 0.1;

struct App {
    creator_loading_view: Option<loading::CreatorView>,
    network_loading_stage: Option<loading::Progress>,
    loading_heartbeat: Option<Instant>,
    loading_max_gap: Duration,
    background_finishes: Vec<std::thread::JoinHandle<()>>,
    pending_host: Option<(net::options::HostOptions, bool)>,
    loading_pixels: Vec<u8>,
    pending_join: Option<(net::session::Join, String)>,
    loading: Option<loading::Job<loading::Prepared>>,
    mission_selection_ready: bool,
    /// The Quick Mission bar and photo, kept when automatic launch continues
    /// straight from aircraft selection into the one mission build.
    mission_loading_progress: Option<loading::Progress>,
    restart_recorded: bool,
    prepared_recording: Option<Option<replay::recorder::Recorder>>,
    prepared_restart: Option<world::Restarted>,
    prepared_scenery: Option<(scenery::Scenery, Option<renderer::LoadingGraphics>)>,
    preference_path: Option<PathBuf>,
    preference_saved: String,
    input: input::Input,
    focused: bool,
    input_recording: Option<std::io::BufWriter<std::fs::File>>,
    recorded_ticks: u64,
    performance: performance::Performance,
    /// Every piece of mutable mission state; see world.rs.
    world: loading::WorldSlot,
    /// What the renderer draws of the world's terrain: art, palettes, camera
    /// weather and the render origin. Rebuilt wherever the terrain is.
    scenery: scenery::Scenery,
    /// The art and models `world.combat` is drawn with; replaced with it.
    combat_view: combat_view::CombatView,
    /// The combat tape being written (`--record-combat`). Combat collects the
    /// records and the app writes them after every tick and when the tape ends.
    combat_tape: Option<tape_file::Recorder>,
    hornet: aircraft::Airframe,
    researched_flight: bool,
    native_tables: Option<std::sync::Arc<tore_sim::native::Tables>>,
    flight_clock: flight::Clock,
    /// The AC-130 gunsight camera's look smoothing and framing zoom.
    sight: gunsight_view::Sight,
    /// The AC-130 sensor console's polarity and static (presentation only).
    console: sensor_console::Console,
    /// The target camera's seeker view of the player's last seeker weapon.
    seeker: seeker_view::SeekerCam,
    /// AC-130 muzzle flashes: the gunship rounds each picture showed.
    gun_flash: gun_flash::Tracker,
    /// Situation music observations; audio only, never read by the simulation.
    flight_music: flight_music::Observer,
    /// The RWR warning tones' lock memory, reset with each flight.
    rwr_warnings: rwr_tone::Warnings,
    /// The other aircraft's launches already sounded.
    launch_watch: audio::LaunchWatch,
    /// Each loaded aircraft type's engine recordings, found once per set of
    /// types for the tick presenter's loop sources.
    engine_sounds: audio::EngineSoundsCache,
    /// The spatial sound the ticks of this frame asked for, until the frame's
    /// last tick is presented.
    spatial_steps: Vec<audio::SpatialStep>,
    vapor: tore_sim::vapor::Vapor,
    /// Player blackout and redout, stepped with the simulation.
    g_effects: tore_sim::g_effects::GEffects,
    flight_view: u8,
    view_rig: flight_views::Rig,
    flight_canvas: flight_canvas::FlightCanvas,
    window_size: [u32; 2],
    /// Live window mode. Alt-Enter toggles it.
    fullscreen: bool,
    /// What the `fullscreen` preference is saved as. A flag such as
    /// `--windowed` chooses the mode for one run without overwriting the
    /// player's saved choice; only Alt-Enter changes this.
    fullscreen_preference: bool,
    flight_ui: flight_ui::FlightUi,
    instruments: instruments::Instruments,
    /// The seat's commands given since the last tick, in the order given. The
    /// next tick applies them (`World::step`), so a menu command given while
    /// the game is paused waits for the first tick after resuming.
    seat_commands: Vec<seats::SeatCommand>,
    /// Whether the gun key (Tab) was last told to the mission as down.
    gun_key_sent: bool,
    /// The cheats the mission has been told of; `None` until a flight's first
    /// tick, so every flight starts by hearing them.
    cheats_sent: Option<tore_sim::cheats::Cheats>,
    /// Shared with a networked session, which builds its mission from it.
    theater_resources: Arc<std::collections::BTreeMap<String, Vec<u8>>>,
    camera: camera::Camera,
    quick: quick_mission::QuickMission,
    /// The creator's draft kept in the profile between sessions (slice BQ,
    /// B22); `None` for diagnostic runs, scripted runs and a file this build
    /// cannot read.
    quick_memory: Option<quick_mission::remember::Memory>,
    /// The custom loadouts flown, kept in the profile with the mission each
    /// was made for (slice BQ2, item 19); `None` where `quick_memory` is not
    /// opened, and with `--loadout`.
    quick_loadouts: Option<quick_mission::loadout_memory::LoadoutMemory>,
    launch_creator: bool,
    /// `--loadout none|guns`: the stores the Load Ordnance page would leave, applied
    /// to the Quick Mission's loadout before launch.
    quick_loadout: Option<String>,
    /// Quick Mission uses AI by default. `--fixture-wings` retains the
    /// straight-flight compatibility setup.
    ai_wings_enabled: bool,
    /// Session-only flight-menu enemy-skill preference; see `--enemy-skill`.
    enemy_skill: Option<tore_sim::ai::experience::EnemySkillOverride>,
    ai_mission: ai_wings::Preset,
    screen: Screen,
    frame_time: Instant,
    instrument_time: Instant,
    target_refresh: target_preview::Refresh,
    /// The open `TORE_FORMATION_TRACE` file for the mission in flight.
    formation_trace: Option<formation_trace::FormationTrace>,
    menu: Menu,
    updates: updates::Updates,
    audio: Option<audio::Audio>,
    renderer: Option<Renderer>,
    /// Graphics choices for the 3D view, applied to the renderer.
    graphics: graphics::Options,
    /// Where the Graphics screen saves them; `None` for diagnostics.
    graphics_path: Option<PathBuf>,
    /// The Sound/Music Prefs settings in effect, handed to the mixer.
    sound: sound_prefs::Settings,
    /// Where the Sound screen saves them; `None` for diagnostics.
    sound_path: Option<PathBuf>,
    pointer: Option<(f64, f64)>,
    modifiers: ModifiersState,
    smoke_test: bool,
    capture_terrain: Option<PathBuf>,
    /// Set by Pref > Re-import media: the menu frame the player was looking
    /// at, which the locate screen then draws over.
    reimport: Option<Vec<u8>>,
    /// The input configuration screen, open over the main menu or the
    /// paused flight menu. One component serves both.
    controls: Option<controls_editor::Editor>,
    feedback: Option<feedback::Form>,
    /// The Graphics options screen, open over the main menu.
    graphics_screen: Option<graphics_screen::Editor>,
    /// The Sound/Music Prefs screen, open over the main menu or the paused
    /// flight menu.
    sound_screen: Option<sound_screen::Screen>,
    /// The mission replay viewer, while `Screen::Replay` shows it.
    replay: Option<Box<replay::host::Replay>>,
    /// Cursor position while the right button drags mouse look.
    mouse_look: Option<(f64, f64)>,
    /// Unused fraction of a smooth-scrolling wheel notch.
    wheel: f64,
    /// Head-tracker view offset added to the player's look angles.
    head_look: [f32; 2],
    finished: bool,
    exit_after_net_debrief: bool,
    next_frame: Option<Instant>,
    /// When a menu with a session open pumps the network and draws.
    menu_pace: net::pace::MenuPace,
    /// Frame pacing (`frame_pacing`): the capped rates' timer, whether a
    /// stopped flight's picture has settled, whether the window is hidden,
    /// and whether the last flight frame had its time stopped.
    pacer: frame_pacing::Pacer,
    stillness: frame_pacing::Stillness,
    occluded: bool,
    stopped_frame: bool,
    error: Option<Box<dyn Error>>,
    /// Where every flight's mission recording goes; `None` turns recording
    /// off (captures and diagnostics). See docs/REPLAYS.md.
    replay_library: Option<replay::library::Library>,
    /// The flight being recorded.
    replay_recorder: Option<replay::recorder::Recorder>,
    /// The mission timer, right-click menu and debug panels in flight,
    /// shown when Pref > Debug panels? is on. See docs/REPLAYS.md.
    live_debug: replay::live::Live,
    /// The Replays screen, open over the main menu. It stays open while the
    /// replay viewer plays one of its recordings.
    replays_screen: Option<replay::screen::Replays>,
    /// `--input-script`: key presses and clicks fed in as if from the window.
    script: Option<input_script::Runner>,
    /// `--connect` or `--host`: the session to start once the window is up.
    connect: Option<net::options::Session>,
    /// The joined (or joining) server.
    net: Option<net::session::NetSession>,
    /// The mission the game built for the session, until the flight starts.
    net_built: Option<net::session::Built>,
    /// The mission's markup for the in-flight map (slice CL-MAP): the starts
    /// and the approximate objective area, read from the mission as built.
    /// `None` for a free flight.
    mission_markup: Option<flight_map::Plan>,
    mission_brief_sentences: Option<Vec<mission_brief::Sentence>>,
    /// A networked flight on screen, with the single-player state it set
    /// aside.
    net_flight: Option<net::play::NetFlight>,
    /// The game's watch of a flying mission while it has no plane, and the
    /// replay viewer's live view of it (slice F2-O2).
    observing: Option<net::observe::Observing>,
    /// Why the session ended, in plain words, for the player.
    net_ending: Option<String>,
    /// The Direct Connection screen, open over the main menu (EF7).
    direct: direct_screen::app::Direct,
    /// The Internet Lobby screen, open over the main menu (I4).
    internet: internet_screen::app::Internet,
    /// The lobby screen and the pages opened over it (EF8).
    lobby: lobby_screen::app::Lobby,
    multiplayer_menu: multiplayer_menu::Menu,
    leave_dialog: Option<net::leave::Dialog>,
    leave_approved: Option<Action>,
}
/// Wing vapor line segments: position then RGBA, two vertices per segment.
/// The five native colors are patterned fill types resolved through LAY
/// header remap tables we have located but not decoded, so the rendered
/// color and fade are fitted: the brightest source sky entry, thinning
/// along the trail.
fn vapor_vertices(
    vapor: &tore_sim::vapor::Vapor,
    world: &terrain::Terrain,
    presented: &flight::State,
    attachments: Option<[[f64; 3]; 2]>,
) -> Vec<f32> {
    let hazing = world
        .weather
        .sample(presented.position[1])
        .is_some_and(|l| l.night_hazing());
    let roll_rate = presented.roll_rate.to_degrees();
    let mut out = Vec::new();
    for side in 0..2 {
        let Some(mut trail) = vapor.trail(side, presented.g, roll_rate, hazing) else {
            continue;
        };
        // The mesh is interpolated for display; pin only the drawn head to that
        // same pose. Authoritative trail samples remain unchanged.
        if let Some(points) = attachments {
            trail[0] = points[side];
        }
        for (i, pair) in trail.windows(2).enumerate() {
            for (end, point) in pair.iter().enumerate() {
                let step = (i + end) as f32 / vapor.segments() as f32;
                out.extend([
                    point[0] as f32,
                    point[1] as f32,
                    point[2] as f32,
                    1.,
                    1.,
                    1.,
                    0.55 * (1. - step),
                ]);
            }
        }
    }
    out
}

fn airport_wind(
    world: &terrain::Terrain,
    flight: &flight::State,
    guidance: Option<&tore_sim::airport::Guidance>,
) -> Option<tore_sim::runway_wind::Assessment> {
    if flight.crashed {
        return None;
    }
    let heading = if let Some((id, height)) = world
        .airport_scene
        .runway_surface(flight.position[0], flight.position[2])
        && flight.supported_at(height)
    {
        let runway = world.airport_scene.runway(id)?;
        let near = runway.heading;
        if (flight.yaw - near).cos() >= 0. {
            near
        } else {
            near + std::f64::consts::PI
        }
    } else {
        let guidance = guidance?;
        world
            .airport_scene
            .runway(guidance.runway)?
            .approach_heading(guidance.end)
    };
    tore_sim::runway_wind::assessment(
        flight.model().configuration().mass.max_takeoff_lbs,
        world.wind(),
        heading,
    )
}

/// The release sounds of `seat`'s plane, with the weapon each came from, for
/// the replay recorder: other seats' releases are theirs to present.
fn seat_releases<'a>(
    world: &'a world::World,
    seat: seats::SeatId,
    releases: &'a [world::Release],
) -> Vec<(&'a str, &'a tore_formats::weapons::Weapon)> {
    let Some(ownship) = world
        .cockpit_of(seat)
        .and_then(|cockpit| world.combat.state.ownship(world.cockpits[cockpit].plane.0))
    else {
        return Vec::new();
    };
    let stations = &ownship.configuration().stations;
    releases
        .iter()
        .filter(|release| release.seat == seat)
        .map(|release| (release.sound.as_str(), &stations[release.station].weapon))
        .collect()
}

/// What `seat`'s plane keeps outside combat, for the commands that change it.
/// What the screens show of it (the tower, NAV mode, the mission result) is in
/// the frame's readout. The presenter's seat always flies a plane.
fn seat_cockpit(world: &world::World, seat: seats::SeatId) -> &world::Cockpit {
    &world.cockpits[world
        .cockpit_of(seat)
        .expect("the presented seat flies a plane")]
}

/// The flight frame of the tick just stepped, for `seat`'s plane: its flight
/// as the tick left it, the tick's newest picture and `cues`, the tick's cues.
/// The seat flies, as the presenter's seat always does.
fn tick_frame<'a>(
    world: &'a world::World,
    seat: seats::SeatId,
    cues: &'a [world::Cue],
) -> frame::FlightFrame<'a> {
    world
        .flight_frame(seat, None, world.combat.render_snapshot(), cues)
        .expect("the presented seat flies a plane")
}

/// [`tick_frame`] whose readout is built once, when the first of the tick's
/// frames reads it, and kept in `shared` for the rest of the tick's frames.
fn shared_tick_frame<'a>(
    world: &'a world::World,
    seat: seats::SeatId,
    cues: &'a [world::Cue],
    shared: &'a std::cell::OnceCell<tore_world::readout::CockpitReadout>,
) -> frame::FlightFrame<'a> {
    world
        .flight_frame_sharing(seat, world.combat.render_snapshot(), cues, shared)
        .expect("the presented seat flies a plane")
}

/// Everything that presents a tick, borrowed from `App` apart from the
/// renderer, which the redraw handler holds while the ticks run.
struct TickPresenter<'a> {
    world: &'a mut world::World,
    scenery: &'a mut scenery::Scenery,
    flight_ui: &'a mut flight_ui::FlightUi,
    input: &'a mut input::Input,
    audio: Option<&'a audio::Audio>,
    recorder: Option<&'a mut replay::recorder::Recorder>,
    instruments: &'a mut instruments::Instruments,
    flight_view: &'a mut u8,
    view_rig: &'a mut flight_views::Rig,
    hornet: &'a aircraft::Airframe,
    combat_view: &'a combat_view::CombatView,
    head_look: [f32; 2],
    g_effects: &'a mut tore_sim::g_effects::GEffects,
    /// The blackout level the bounded perf run forces (`TORE_PERF_VEIL`).
    perf_blackout: Option<f64>,
    vapor: &'a mut tore_sim::vapor::Vapor,
    flight_music: &'a mut flight_music::Observer,
    rwr_warnings: &'a mut rwr_tone::Warnings,
    launch_watch: &'a mut audio::LaunchWatch,
    engine_sounds: &'a mut audio::EngineSoundsCache,
    /// The spatial sound the frame's ticks asked for so far.
    spatial: &'a mut Vec<audio::SpatialStep>,
}

impl TickPresenter<'_> {
    /// Presents one tick in the order the redraw loop ran it: HUD lines,
    /// rumble, camera weather, sounds and the replay recorder, from what
    /// `World::step` reported. Presentation reads the finished tick; see
    /// docs/ARCHITECTURE.md, "One tick". Returns false when a fault in the
    /// native research adapter stopped the tick, which pauses the flight.
    fn present(&mut self, input: &seats::SeatInput, out: &world::TickOutput) -> bool {
        // The tick's readout, built when a presenter first reads it.
        let shared = std::cell::OnceCell::new();
        let mut weapon_cycled = false;
        for (index, cue) in out.cues.iter().enumerate() {
            if index == out.commanded {
                self.commands_applied();
            }
            match cue {
                // Another seat's output is that seat's to present.
                cue if !cue.is_for(input.seat) => {}
                world::Cue::Message { text, .. } => self.flight_ui.message(text.clone()),
                world::Cue::Feedback { event, .. } => self.input.feedback(*event),
                world::Cue::Tower { stem, .. } => {
                    if let Some(audio) = self.audio {
                        match stem {
                            Some(stem) => audio.airport_radio(&[stem]),
                            None => audio.cancel_airport_radio(),
                        }
                    }
                }
                world::Cue::WeaponCycled { .. } => weapon_cycled = true,
                world::Cue::OrderVoice { stems, .. } => {
                    if let Some(audio) = self.audio {
                        audio.radio(stems, true);
                    }
                }
                world::Cue::Flown => self.flown(input.seat, &shared),
                world::Cue::CombatStepped => {
                    let frame = shared_tick_frame(self.world, input.seat, &out.cues, &shared);
                    let (before, after) = (frame.previous, frame.flight);
                    if let Some(view) = self.flight_ui.pilot_death_view(
                        before.systems.pilot.dead || before.escape.is_some(),
                        after.systems.pilot.dead || after.escape.is_some(),
                    ) {
                        *self.flight_view = view;
                        self.view_rig.select(flight_views::Reference::Player);
                    }
                }
                world::Cue::WingEjection {
                    id,
                    message,
                    friendly,
                } => {
                    if let Some(recording) = &mut self.recorder {
                        recording.wing_ejection(*id, message, *friendly);
                    }
                    self.flight_ui.message(message.clone());
                    if *friendly && let Some(audio) = &self.audio {
                        audio.wingman_ejected();
                    }
                }
                world::Cue::Picture => {
                    // The mission recording reads the tick's picture.
                    let frame = shared_tick_frame(self.world, input.seat, &out.cues, &shared);
                    if let Some(recording) = &mut self.recorder {
                        let idle = flight::PilotInput::default();
                        let others = other_crews(&self.world.cockpits, frame.plane, &idle);
                        recording.begin(replay::recorder::Tick {
                            snapshot: frame.picture,
                            combat: &self.world.combat,
                            flight: frame.flight,
                            previous: frame.previous,
                            pilot: &input.pilot,
                            others: &others,
                            wings: self.world.ai_wings.as_ref(),
                            world: &self.world.terrain,
                            events: &out.events,
                            outcomes: &out.outcomes,
                            journal: out.journal.as_ref(),
                        });
                    }
                    if (frame.flight.crashed
                        || frame.flight.escape.is_some()
                        || frame.flight.systems.pilot.dead)
                        && let Some(audio) = &self.audio
                    {
                        audio.cancel_airport_radio();
                    }
                }
                world::Cue::Radio { call, .. } => match call.route {
                    comms::Route::Radio | comms::Route::Airport => {
                        self.flight_ui.message(call.line());
                        if let Some(audio) = self.audio {
                            if call.route == comms::Route::Airport {
                                audio.airport_speech(&call.stems);
                            } else {
                                audio.speech(&call.stems);
                            }
                        }
                    }
                    comms::Route::Direct => {
                        if let (Some(audio), Some(stem)) = (self.audio, call.stems.first()) {
                            audio.direct_voice(stem);
                        }
                    }
                },
            }
        }
        if out.commanded >= out.cues.len() {
            self.commands_applied();
        }
        if weapon_cycled {
            show_selected_weapon_page(
                &shared_tick_frame(self.world, input.seat, &out.cues, &shared),
                &self.world.combat,
                self.instruments,
            );
        }
        if let Some(error) = &out.fault {
            self.flight_ui.message(error.clone());
            self.flight_ui.paused = true;
            log::warn!("{error}");
            return false;
        }
        // Every communication decision of the tick, with its trigger and
        // reason. Write-only.
        if let Some(recording) = &mut self.recorder {
            recording.drain_comms(&mut self.world.comms);
            recording.drain_datalink(&mut self.world.datalink);
        }
        let frame = shared_tick_frame(self.world, input.seat, &out.cues, &shared);
        let (plane, flight) = (frame.plane.0, frame.flight);
        // The danger is the ejection warning's alone, so it is worked out
        // only for a mixer that sounds it.
        if let Some(audio) = self.audio {
            let danger = tore_sim::ejection::assess(flight, |x, z| {
                f64::from(self.world.terrain.height(x as f32, z as f32))
            })
            .is_some();
            audio.ejection(frame.previous, flight, danger);
        }
        if let Some(audio) = self.audio {
            // The result and home checks are the mission core's, which also
            // sends the result calls whatever the audio.
            let music = self
                .flight_music
                .step(&frame, &out.events, &self.world.terrain);
            audio.situation(&music.inputs, music.now);
            audio.rwr(self.rwr_warnings.step(
                frame.readout.tick,
                rwr_tone::inbound(&frame.readout, flight.position),
                &frame.readout.rwr.locks,
                flight.escape.is_some() || flight.systems.pilot.dead,
            ));
            // What the music's inputs asked for, and why.
            if let Some(recording) = &mut self.recorder {
                recording.comms(music.journal);
            }
        }
        // Audio observes authoritative poses and consumes each emission once.
        if self.audio.is_some() {
            // The listener's camera needs the other aircraft only for a view
            // that looks at one.
            let scene = if self.view_rig.needs_bodies(*self.flight_view) {
                flight_views::Scene::new(
                    &frame,
                    flight,
                    &self.world.combat,
                    self.world.ai_wings.as_ref(),
                    None,
                )
            } else {
                flight_views::Scene::lean(&frame, flight, &self.world.combat)
            };
            let scene = self
                .view_rig
                .prepare(scene, *self.flight_view, &self.world.terrain);
            let listener_camera = self
                .view_rig
                .clone()
                .camera(
                    *self.flight_view,
                    &scene,
                    self.hornet
                        .camera(flight, *self.flight_view, Default::default()),
                    look::combine(
                        self.flight_ui.look,
                        self.head_look,
                        matches!(*self.flight_view, 1 | 2),
                    ),
                    self.flight_ui.zoom,
                )
                .unwrap_or_else(|_| self.hornet.camera(flight, 0, Default::default()));
            let basis = tore_sim::attitude::Basis::new(
                f64::from(listener_camera.yaw),
                f64::from(listener_camera.pitch),
                -f64::from(listener_camera.roll),
            );
            // Every other aircraft's launches this tick (AI wings, enemies,
            // other players), heard from where they were fired.
            let others = self.launch_watch.fresh(&self.world.combat.state, plane);
            // The tick's own part of the spatial sound is kept for the end of
            // the frame, when the sources and loops of its last tick are
            // worked out once for all the ticks ([`audio::Audio::spatial_frame`]).
            self.spatial.push(audio::SpatialStep {
                listener: tore_sim::acoustics::Listener {
                    position: listener_camera.position,
                    right: basis.right,
                    view: *self.flight_view,
                    external: !self.view_rig.cockpit(*self.flight_view),
                    own: Some(plane),
                },
                emissions: out.emissions.clone(),
                launches: seat_releases(self.world, input.seat, &out.releases)
                    .into_iter()
                    .map(|(sound, weapon)| {
                        audio::Launch::new(sound, weapon.effects.max_sound_distance)
                    })
                    .chain(others.iter().map(|launch| {
                        audio::Launch::by(
                            launch.shooter,
                            launch.position,
                            &launch.sound,
                            launch.max_distance_ft,
                        )
                        .with_gun(launch.gun)
                    }))
                    .map(|launch| audio::OwnedLaunch::of(&launch))
                    .collect(),
                player_position: flight.position,
                player_velocity: flight.velocity,
            });
        }
        if let Some(recording) = &mut self.recorder {
            let releases = seat_releases(self.world, input.seat, &out.releases);
            recording.sounds(&out.emissions, &releases);
        }
        if flight.crashed && !frame.previous.crashed && flight.escape.is_none() {
            self.input.feedback(tore_input::FeedbackEvent::Crash);
        }
        if flight.afterburner_active() && !frame.previous.afterburner_active() {
            self.input
                .feedback(tore_input::FeedbackEvent::AfterburnerEngaged);
        }
        self.input.afterburner_feedback(flight.afterburner_active());
        self.input.feedback_tick();
        drop(frame);
        if let Some(recording) = &mut self.recorder {
            recording.end(Some(&mut self.flight_ui), &mut self.world.combat);
        }
        true
    }

    /// The tick's commands have been applied and their cockpit messages
    /// shown: the mission recording notes them on the frame before the tick,
    /// where they were given, and then opens the tick. It used to open the
    /// tick before the step, when the commands had already been given.
    fn commands_applied(&mut self) {
        if let Some(recording) = &mut self.recorder {
            recording.start_tick(Some(&mut self.flight_ui), &mut self.world.combat);
        }
    }

    /// Presentation that follows the player's flight, the weather clock and
    /// turbulence: each camera slot's weather, the view rig, blackout and
    /// redout, wing vapor and control-surface sounds.
    fn flown(
        &mut self,
        seat: seats::SeatId,
        shared: &std::cell::OnceCell<tore_world::readout::CockpitReadout>,
    ) {
        let frame = shared_tick_frame(self.world, seat, &[], shared);
        let flight = frame.flight;
        let speed = flight.speed;
        let scene = if self.view_rig.needs_bodies(*self.flight_view) {
            flight_views::Scene::new(
                &frame,
                flight,
                &self.world.combat,
                self.world.ai_wings.as_ref(),
                None,
            )
        } else {
            flight_views::Scene::lean(&frame, flight, &self.world.combat)
        };
        let scene = self
            .view_rig
            .prepare(scene, *self.flight_view, &self.world.terrain);
        let weather_view = self
            .view_rig
            .camera(
                *self.flight_view,
                &scene,
                self.hornet
                    .camera(flight, *self.flight_view, Default::default()),
                look::combine(
                    self.flight_ui.look,
                    self.head_look,
                    matches!(*self.flight_view, 1 | 2),
                ),
                self.flight_ui.zoom,
            )
            .unwrap_or_else(|_| self.hornet.camera(flight, 0, Default::default()));
        self.scenery
            .step_view_weather(&self.world.terrain, &weather_view, speed);
        self.scenery
            .step_view_weather(&self.world.terrain, &mirrors::camera(flight), speed);
        self.scenery.step_view_weather(
            &self.world.terrain,
            &self.hornet.panel_camera(flight, 2),
            speed,
        );
        self.view_rig.observe(&scene);
        if let Ok(camera) = self.view_rig.other_camera(
            &scene,
            self.hornet
                .camera(flight, self.view_rig.other_view(), Default::default()),
        ) {
            self.scenery
                .step_view_weather(&self.world.terrain, &camera, speed);
        }
        if let Some(camera) =
            self.combat_view
                .target_camera(&self.world.combat, &frame.readout, flight)
        {
            self.scenery
                .step_view_weather(&self.world.terrain, &camera, speed);
        }
        self.g_effects.step(
            flight.g,
            !self.flight_ui.cheats.no_g_effects && !flight.crashed && flight.native.is_none(),
        );
        if let Some(level) = self.perf_blackout {
            self.g_effects.blackout = level;
        }
        if let Some(points) = self.hornet.streamer_points(flight) {
            self.vapor.step(self.world.terrain.weather.ticks(), points);
        }
        if let Some(audio) = self.audio {
            audio.controls(frame.previous, flight);
        }
    }
}

impl App {
    /// The plane the app presents: the one `SEAT` flies.
    fn plane(&self) -> u32 {
        seat_cockpit(&self.world, SEAT).plane.0
    }

    /// Queues a command of the seat for the next tick.
    fn queue(&mut self, command: seats::SeatCommand) {
        queue_command(&mut self.seat_commands, command);
    }

    /// Queues what the instrument panels asked for: weapon-page buttons and a
    /// designation from a scope click. The simulation revalidates the
    /// requested identity, so a click can never select a target it does not
    /// observe.
    fn queue_scope_commands(&mut self) {
        for button in std::mem::take(&mut self.instruments.weapon_controls) {
            self.queue(seats::SeatCommand::CycleWeapon {
                forward: button == 1,
            });
        }
        if let Some(id) = self.instruments.designation.take() {
            self.queue(seats::SeatCommand::Combat(
                tore_sim::combat::live::Command::DesignateTarget(id),
            ));
        }
    }

    /// Lets go of the trigger, as a menu, a pause, a modifier key or losing
    /// focus does. In flight the next tick does it, in order with the seat's
    /// other commands; anywhere else there is no tick to wait for.
    fn release_trigger(&mut self) {
        // The release lets go of the gun key too; a key still held goes down
        // again at the next tick.
        self.gun_key_sent = false;
        if self.screen == Screen::Flight {
            self.queue(seats::SeatCommand::ReleaseTrigger);
        } else {
            self.world.combat.cancel();
        }
    }

    /// Seed after the final launch position is set, so no trail crosses a teleport.
    fn reset_vapor(&mut self) {
        self.vapor = tore_sim::vapor::Vapor::seeded(
            self.hornet
                .streamer_points(&self.world.cockpits[OWN].flight)
                .unwrap_or([[0.; 3]; 2]),
        );
    }

    /// Switch between borderless fullscreen and the previous windowed size.
    /// winit restores the window's pre-fullscreen size and position, so the
    /// player returns to the window they had. Works on every screen: the
    /// menus, the creator and flight.
    fn toggle_fullscreen(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        self.fullscreen_preference = self.fullscreen;
        renderer
            .window
            .set_fullscreen(fullscreen_attribute(self.fullscreen));
        renderer.window.request_redraw();
        // The letterbox and the pointer mapping both follow the new size, and
        // a press in flight must not survive the change.
        self.menu.state.cancel();
        self.quick.cancel();
        self.instruments.cancel_press();
        self.flight_ui.cancel_press();
        self.pointer = None;
        self.save_preferences();
    }

    fn save_preferences(&mut self) {
        if let Some(memory) = &mut self.quick_memory {
            memory.save(&self.quick);
        }
        let Some(path) = &self.preference_path else {
            return;
        };
        let text = preferences::Preferences::capture(
            &self.flight_ui,
            &self.instruments,
            self.fullscreen_preference,
        )
        .text();
        if text == self.preference_saved {
            return;
        }
        match preferences::write(path, &text) {
            Ok(()) => self.preference_saved = text,
            Err(e) => {
                log::warn!("Preferences: {e}");
                self.flight_ui
                    .message(format!("Could not save preferences: {e}"));
            }
        }
    }
    /// Writes the records combat collected to the combat tape, if one is
    /// being recorded.
    fn drain_combat_tape(&mut self) {
        if let Some(tape) = &mut self.combat_tape {
            tape.write_all(self.world.combat.take_tape());
        }
    }
    /// Ends the combat tape: writes what is left and flushes the file.
    fn finish_combat_tape(&mut self) -> AppResult<()> {
        self.drain_combat_tape();
        self.world.combat.stop_tape();
        if let Some(mut tape) = self.combat_tape.take() {
            tape.flush()?;
        }
        Ok(())
    }
    fn finish_recording(&mut self) {
        use std::io::Write;
        if let Some(mut recording) = self.input_recording.take()
            && let Err(error) = recording.flush()
        {
            self.error = Some(error.into());
        }
    }

    /// Starts recording the flight that was just set up. A recording that
    /// cannot start is logged and the flight goes on unrecorded.
    fn start_replay_recording(&mut self) {
        if let Some(recording) = self.prepared_recording.take() {
            self.replay_recorder = recording;
            return;
        }
        let start = self.recording_starter();
        self.replay_recorder = start(&mut self.world);
    }

    fn recording_starter(
        &self,
    ) -> impl FnOnce(&mut world::World) -> Option<replay::recorder::Recorder> + Send + 'static {
        let library = self.replay_library.clone();
        let aircraft_id = self.hornet.profile.id;
        let aircraft_name = self.hornet.profile.name.clone();
        let model = if self.native_tables.is_some() {
            "native-tables"
        } else if self.researched_flight {
            "researched"
        } else {
            "legacy"
        };
        let audio = self.audio.is_some();
        let ai_mission = self.ai_mission.to_string();
        let cheats = replay::recorder::cheats_on(&self.flight_ui.cheats);
        move |world| {
            let _timing = loading::timing("recording initialization");
            use replay::{convert, recorder};
            let library = library?;
            // Commands from before this flight belong to no recording.
            let _ = world.combat.take_notes();
            let started = std::time::SystemTime::now();
            let snapshot = world.combat.render_snapshot();
            let presentation =
                convert::Presentation::of(snapshot).with_player(world.cockpits[OWN].plane.0);
            let aircraft = convert::identity_key(aircraft_id);
            let path = match library.new_path(
                started,
                world.terrain.layout.trim_end_matches(".MM"),
                aircraft.trim_end_matches(".PT"),
            ) {
                Ok(path) => path,
                Err(error) => {
                    log::info!("Recording unavailable: {error}");
                    return None;
                }
            };
            let mut extra = vec![
                ("flight_model".to_owned(), model.to_owned()),
                ("player.aircraft".into(), aircraft.into()),
                ("audio".into(), if audio { "on" } else { "off" }.into()),
            ];
            if let Some((altitude, fuel)) = world.setup.mission {
                extra.push((
                    "mission.start".into(),
                    world
                        .setup
                        .ground_start
                        .map_or("airborne".to_owned(), |object| format!("runway {object}")),
                ));
                extra.push(("mission.altitude_ft".into(), altitude.to_string()));
                extra.push(("mission.fuel_lb".into(), fuel.to_string()));
                extra.push(("ai.mission".into(), ai_mission));
                extra.push((
                    "ai.aircraft".into(),
                    world
                        .ai_wings
                        .as_ref()
                        .map_or(0, ai_wings::AiWings::len)
                        .to_string(),
                ));
            }
            if world.combat.range {
                extra.push(("range".into(), "live fire".into()));
            }
            let cheats = cheats;
            if !cheats.is_empty() {
                extra.push(("cheats".into(), cheats.join(",")));
            }
            let mission = if world.setup.mission.is_some() {
                tore_replay::MissionKind::QuickMission
            } else {
                tore_replay::MissionKind::FreeFlight
            };
            let header = recorder::header(mission, &world.terrain, &presentation, extra, started);
            let (player, others) = recorded_humans(world, SEAT, &aircraft_name);
            let roster = recorder::roster(
                snapshot,
                &player,
                &others,
                world.ai_wings.as_ref(),
                world.combat.dummy_types(),
            );
            let mut recording = match recorder::Recorder::start(path, &header, &roster) {
                Ok(recording) => recording.for_seat(SEAT, player.id),
                Err(error) => {
                    log::info!("Recording unavailable: {error}");
                    return None;
                }
            };
            // The flight as it starts, before the first tick.
            let idle = flight::PilotInput::default();
            let crews = other_crews(&world.cockpits, world.cockpits[OWN].plane, &idle);
            recording.begin(recorder::Tick {
                snapshot: world.combat.render_snapshot(),
                combat: &world.combat,
                flight: &world.cockpits[OWN].flight,
                previous: &world.cockpits[OWN].flight,
                pilot: &idle,
                others: &crews,
                wings: world.ai_wings.as_ref(),
                world: &world.terrain,
                events: &[],
                outcomes: &[],
                journal: None,
            });
            recording.end(None, &mut world.combat);
            Some(recording)
        }
    }

    fn finish_replay_recording(&mut self, reason: &str) {
        let Some(mut recording) = self.replay_recorder.take() else {
            return;
        };
        recording.note(
            tore_replay::Event::new(tore_replay::vocab::kind::SYSTEM_END)
                .with(tore_replay::vocab::field::REASON, reason),
        );
        let report = self
            .world
            .setup
            .mission
            .is_some()
            .then(|| debrief::capture(&self.world, SEAT))
            .flatten();
        let footer = replay_footer(
            &self.world.combat,
            recording.player(),
            report.as_ref(),
            reason,
        );
        let library = self.replay_library.clone();
        let finish = std::thread::Builder::new()
            .name("recording-finish".into())
            .spawn(move || {
                if recording.finish(&footer).is_some()
                    && let Some(library) = &library
                {
                    let settings = library.settings();
                    let cleanup = library.cleanup(&settings, std::time::SystemTime::now(), &[]);
                    for path in &cleanup.deleted {
                        log::info!("Recording auto-deleted: {}", path.display());
                    }
                    for (path, error) in &cleanup.failed {
                        log::info!("Recording not deleted: {}: {error}", path.display());
                    }
                }
            });
        match finish {
            Ok(handle) => self.shelve_background(handle),
            Err(error) => log::warn!("Could not finish the recording: {error}"),
        }
    }

    /// Keeps a finished or still-running cleanup worker until the window has
    /// closed. Completed handles are dropped here so a long session does not
    /// accumulate them.
    fn shelve_background(&mut self, handle: std::thread::JoinHandle<()>) {
        self.background_finishes
            .retain(|finished| !finished.is_finished());
        self.background_finishes.push(handle);
    }

    /// Drops a loading job without installing its result, and keeps the
    /// worker so shutdown can wait for it. The receiver goes with the job,
    /// so a late result cannot be installed.
    fn dismiss_loading(&mut self) {
        if let Some(job) = self.loading.take()
            && let Some(handle) = job.into_thread()
        {
            self.shelve_background(handle);
        }
    }

    /// `Stability: Damper` and so on for a stability-level command on a
    /// powered-lift aircraft (VTOL overhaul, design 5.2). The level becomes
    /// the preference, which [`sync_stability`] keeps the flight at.
    fn announce_stability(&mut self, command: tore_input::PilotCommand) {
        use tore_input::{LiftCommand, PilotCommand};
        if !matches!(
            command,
            PilotCommand::Lift(LiftCommand::SetStability(_) | LiftCommand::CycleStability)
        ) {
            return;
        }
        let flight = &self.world.cockpits[OWN].flight;
        if flight.stability_in_effect().is_none() {
            return;
        }
        let level = self
            .input
            .pending_stability(flight.lift_controls.aids.stability);
        self.flight_ui.stability = level;
        self.flight_ui
            .message(format!("Stability: {}", flight_ui::stability_name(level)));
    }
    fn input_action(&mut self, action: tore_input::Action) -> Action {
        use flight_ui::Command;
        if self.leave_dialog.is_some() && !matches!(action, tore_input::Action::Ui(_)) {
            return Action::None;
        }
        let name = match action {
            tore_input::Action::Pilot(command) => {
                if self.screen == Screen::Flight && !self.flight_ui.frozen() && !self.respawn_open()
                {
                    if matches!(
                        command,
                        tore_input::PilotCommand::Toggle(tore_input::Switch::Hook)
                            | tore_input::PilotCommand::Set(tore_input::Switch::Hook, _)
                    ) && !self.world.cockpits[OWN].flight.hook_available()
                    {
                        self.flight_ui.message("Hook unavailable for this aircraft");
                    } else if command == tore_input::PilotCommand::Toggle(tore_input::Switch::Radar)
                        && self.radar_key_returns()
                    {
                        // The same rule as the keyboard: returning to the air
                        // radar comes before spending the power switch.
                    } else {
                        self.input.queue(command);
                        self.announce_stability(command);
                    }
                }
                return Action::None;
            }
            tore_input::Action::Axis(_) => return Action::None,
            tore_input::Action::Ui(name) => name,
        };
        let key = match name.as_str() {
            "menu-up" => Some("ArrowUp"),
            "menu-down" => Some("ArrowDown"),
            "menu-left" => Some("ArrowLeft"),
            "menu-right" => Some("ArrowRight"),
            "menu-accept" => Some("Enter"),
            "menu-back" | "menu" => Some("Escape"),
            _ => None,
        };
        if self.leave_dialog.is_some() {
            return key.map_or(Action::None, |key| self.leave_dialog_key(key));
        }
        if self.multiplayer_menu_active() {
            let menu_key = if name == "menu" { Some("F1") } else { key };
            if let Some(action) = menu_key.and_then(|key| self.multiplayer_menu.key(key)) {
                return action;
            }
            // Left and Right walk lounge controls, Up and Down operate their lists.
            if (self.direct_open() || self.internet_open())
                && matches!(key, Some("ArrowLeft" | "ArrowRight"))
            {
                let shift = key == Some("ArrowLeft");
                if let Some(screen) = &mut self.direct.screen
                    && self.lobby.screen.is_none()
                {
                    let outcome = screen.key("Tab", shift);
                    return self.direct_outcome(outcome);
                }
                if let Some(screen) = &mut self.internet.screen
                    && self.lobby.screen.is_none()
                {
                    let outcome = screen.key("Tab", shift);
                    return self.internet_outcome(outcome);
                }
            }
        }
        // Controller menu buttons work the Sound screen while it is open.
        if let Some(screen) = &mut self.sound_screen {
            let Some(key) = key else {
                return Action::None;
            };
            let outcome = screen.key(key, false);
            return self.sound_result(outcome);
        }
        // And the Direct Connection screen.
        if self.direct_open() {
            let Some(key) = key else {
                return Action::None;
            };
            return self.direct_key(key, None);
        }
        // And the Internet Lobby screen.
        if self.internet_open() {
            let Some(key) = key else {
                return Action::None;
            };
            return self.internet_key(key, None);
        }
        // Controller menu buttons navigate the Graphics screen while it is open.
        if let Some(editor) = &mut self.graphics_screen {
            let Some(key) = key else {
                return Action::None;
            };
            let result = editor.key(key, false);
            return self.graphics_result(result);
        }
        // And the Replays screen, unless the replay viewer is showing.
        if self.screen == Screen::Main
            && let Some(screen) = &mut self.replays_screen
        {
            let Some(key) = key else {
                return Action::None;
            };
            let result = screen.key(key, false, false);
            return self.replays_result(result);
        }
        if self.screen == Screen::Flight && self.respawn_open() {
            return key.map_or(Action::None, |key| self.respawn_key(key));
        }
        if self.screen == Screen::Flight && name == "menu-accept" && self.fly_again() {
            return Action::None;
        }
        if self.screen != Screen::Flight {
            return match (self.screen, key) {
                (Screen::Main, Some(key)) => self.menu.state.key(key, false),
                (Screen::Quick, Some(key)) => self.quick.key(key, false),
                _ => Action::None,
            };
        }
        if let Some(key) = key {
            // Menu navigation buttons have no flight semantics while the menu is closed.
            if self.flight_ui.menu || self.flight_ui.map.open || name == "menu" {
                let command = self.flight_ui.key(
                    key,
                    false,
                    false,
                    false,
                    net::play::menu(&self.hornet, &self.net_flight),
                );
                return self.flight_command(command);
            }
            return Action::None;
        }
        if name == "pause" {
            let command = self.flight_ui.key(
                "p",
                false,
                true,
                false,
                net::play::menu(&self.hornet, &self.net_flight),
            );
            return self.flight_command(command);
        }
        let lighting_action = matches!(
            name.as_str(),
            "key:Ctrl-i"
                | "key:Ctrl-Shift-f"
                | "key:Ctrl-h"
                | "key:Shift-["
                | "key:Shift-]"
                | "instruments-dim"
                | "instruments-brighten"
                | "flood-dim"
                | "flood-brighten"
        );
        if self.flight_ui.frozen()
            && !(lighting_action && !self.flight_ui.menu && !self.flight_ui.map.open)
        {
            return Action::None;
        }
        if let Some(mut key) = name.strip_prefix("key:") {
            let ctrl = key.starts_with("Ctrl-");
            if ctrl {
                key = &key[5..];
            }
            let alt = key.starts_with("Alt-");
            if alt {
                key = &key[4..];
            }
            let shift = key.starts_with("Shift-");
            if shift {
                key = &key[6..];
            }
            let map_before = self.flight_ui.map.open;
            let command = self.flight_ui.key(
                key,
                shift,
                ctrl,
                alt,
                net::play::menu(&self.hornet, &self.net_flight),
            );
            if !map_before && self.flight_ui.map.open {
                self.flight_ui.map.brief = None;
            }
            return self.flight_command(command);
        }
        // In the AC-130's sensor console the mouse wheel zooms the sight
        // (slice nr-cona).
        if let Some(steps) = console_keys::zoom_action(&name)
            && self.console_open()
        {
            return self.flight_command(Command::SightZoom(steps));
        }
        let command = match name.as_str() {
            "instruments-dim" => self.flight_ui.activate("Dim instruments", ""),
            "instruments-brighten" => self.flight_ui.activate("Brighten instruments", ""),
            "flood-dim" => self.flight_ui.activate("Dim flood", ""),
            "flood-brighten" => self.flight_ui.activate("Brighten flood", ""),
            "weapon-next" => Command::NextWeapon,
            "weapon-group-next" => Command::Combat(tore_sim::combat::live::Command::NextGunGroup),
            "weapon-group-toggle" => {
                Command::Combat(tore_sim::combat::live::Command::ToggleGunGroup)
            }
            "weapon-previous" => Command::PreviousWeapon,
            "weapon-seeker-mode" => {
                Command::Combat(tore_sim::combat::live::Command::ToggleSeekerMode)
            }
            "designate" => Command::Target,
            "designate-previous" => Command::TargetPrevious,
            "designate-visual" => Command::TargetVisual,
            "designate-centre" => Command::TargetCentre,
            "clear-designation" => {
                Command::Combat(tore_sim::combat::live::Command::ClearDesignation)
            }
            "master-arm" => Command::None, // Retired binding in older profiles.
            "jettison" => Command::Combat(tore_sim::combat::live::Command::Jettison),
            "jettison-tanks" => Command::Combat(tore_sim::combat::live::Command::JettisonTanks),
            "range-target" => Command::RangeReset,
            "sight-designate" => Command::SightDesignate,
            "sight-pin" => Command::SightPinGround,
            "sensor-console" => Command::SensorConsole,
            "sight-pin-toggle" => Command::SightPinToggle,
            "gun-lock" => Command::GunLock,
            // The air-to-ground radar page and the Easy targeting cheat's
            // nearest ground target (slice G1).
            "a-g-radar" => return self.air_ground_key(),
            "nearest-ground" => return self.nearest_ground_key(),
            // The laser and the targeting sight's next object (slice T1).
            "laze" => return self.laze_key(),
            "sight-next" => return self.sight_next_key(),
            // The sensor console's white hot / black hot (presentation).
            "sight-polarity" => return self.sight_polarity_key(),
            // Shift+F5 to Shift+F8: straight to one picture (nr-replayviews).
            "sight-whot" | "sight-bhot" | "sight-tv" | "sight-nvg" => {
                return sensor_console::SensorMode::of_action(name.as_str())
                    .map_or(Action::None, |mode| self.sight_picture_key(mode));
            }
            // The sensor console radar's range (5, 10, 25 nm).
            "console-range" => return self.console_range_key(),
            // Tab is a hold: `queue_gun_key` reads it every tick.
            "fire-gun" => Command::None,
            "sight-zoom-in" => Command::SightZoom(1),
            "sight-zoom-out" => Command::SightZoom(-1),
            "damage-class" => Command::Combat(tore_sim::combat::live::Command::CycleClass),
            "fail-station" => Command::Combat(tore_sim::combat::live::Command::FailStation),
            "damage-report" => Command::DamageReport,
            "damage-player" => Command::Combat(tore_sim::combat::live::Command::DamagePlayer),
            "incoming" => Command::Combat(tore_sim::combat::live::Command::Incoming),
            "target-jammer" => Command::Combat(tore_sim::combat::live::Command::ToggleTargetJammer),
            // The console's own countermeasure actions (John, 2026-10-14):
            // bound apart from the main keys, the same dispensers.
            "chaff" | "console-chaff" => Command::Chaff,
            "flare" | "console-flare" => Command::Flare,
            "end-flight" => Command::End,
            "restart" => Command::Restart,
            "bookmark" => Command::Bookmark,
            "view-front" => Command::View(0),
            "view-back" => Command::View(3),
            "view-up" => Command::View(4),
            "view-external" => Command::View(1),
            "view-track" => Command::View(flight_views::TRACK),
            "view-threat" => Command::View(flight_views::THREAT),
            "view-wing" => Command::View(flight_views::WING),
            "view-target" => Command::View(flight_views::TARGET),
            "view-target-player" => Command::View(flight_views::TARGET_PLAYER),
            "view-fly-by" => Command::View(flight_views::FLY_BY),
            "view-missile" => Command::View(flight_views::MISSILE),
            "store-view" => Command::StoreView,
            "view-target-track" => {
                Command::ViewRelative(flight_views::TRACK, flight_views::Reference::Target)
            }
            "center-look" => Command::CenterLook,
            "waypoint-next" => Command::Waypoint(true),
            "waypoint-previous" => Command::Waypoint(false),
            s if s.starts_with("throttle-preset=") => match &s[16..] {
                "burner" => Command::ThrottlePreset(1., true),
                value => value
                    .parse()
                    .map_or(Command::None, |value| Command::ThrottlePreset(value, false)),
            },
            s if s.starts_with("throttle-step=") => {
                s[14..].parse().map_or(Command::None, Command::ThrottleStep)
            }
            "instrument-next" => Command::InstrumentCycle(1),
            "instrument-previous" => Command::InstrumentCycle(-1),
            "range-down" => Command::Range(-1),
            "range-up" => Command::Range(1),
            // The recovered radar-mode action is the available sensor-channel
            // cycle; the two newer names are explicit aliases for it.
            "radar-mode" | "sensor-channel" => Command::Mode,
            "sensor-infrared" => Command::SensorInfrared,
            "sensor-history" => Command::SensorHistory,
            "airport-next" => {
                // The pilot's side's own and neutral fields, never a short
                // strip or the other side's (slice AL1).
                let plane = self
                    .net_flight
                    .as_ref()
                    .and_then(|flight| flight.frame.as_ref())
                    .map_or(self.world.cockpits[OWN].plane, |frame| frame.plane);
                let next = navigation::next_airport(
                    &self.world.terrain.airport_scene,
                    self.world.cockpits[OWN].airport_service.selected(),
                    self.world.roster.redfor(plane),
                );
                next.map_or(Command::None, |id| {
                    Command::Airport(tore_sim::airport::Command::SelectAirport(id))
                })
            }
            "airport-request-landing" => {
                Command::Airport(tore_sim::airport::Command::RequestLanding)
            }
            "airport-repeat" => Command::Airport(tore_sim::airport::Command::RepeatReply),
            "airport-cancel" => Command::Airport(tore_sim::airport::Command::CancelApproach),
            "airport-nav" => Command::AirportNav,
            "cockpit" => {
                self.flight_ui.cockpit = !self.flight_ui.cockpit;
                Command::Click
            }
            "hud" => {
                self.flight_ui.hud = !self.flight_ui.hud;
                Command::Click
            }
            "zoom-in" => {
                self.flight_ui.zoom = (self.flight_ui.zoom * 1.1).min(4.);
                Command::None
            }
            "zoom-out" => {
                self.flight_ui.zoom = (self.flight_ui.zoom / 1.1).max(0.5);
                Command::None
            }
            s if s.starts_with("page-") => Command::Panel(s[5..].parse().unwrap_or(0)),
            s if s.starts_with("control-") => {
                Command::InstrumentControl(s[8..].parse::<usize>().unwrap_or(1) - 1)
            }
            s if s.starts_with("instrument-") => {
                if let Some((slot, button)) = s[11..].split_once("-control-") {
                    let slot = slot.parse::<usize>().unwrap_or(1) - 1;
                    let button = button.parse::<usize>().unwrap_or(1) - 1;
                    if self.instruments.control(slot, button) {
                        self.queue_scope_commands();
                        return Action::Click;
                    }
                    self.flight_ui.message("Instrument control unavailable");
                    return Action::None;
                }
                Command::InstrumentSelect(s[11..].parse::<usize>().unwrap_or(1) - 1)
            }
            _ => Command::None,
        };
        self.flight_command(command)
    }

    /// A camera change the debug menu asked for: the chase view or the
    /// front view on an aircraft, the player's own cockpit for the player.
    fn live_view(&mut self, view: replay::live::View) -> Action {
        let (view, id) = match view {
            replay::live::View::Follow(id) => (1, id),
            replay::live::View::Cockpit(id) => (0, id),
        };
        self.flight_command(if id == 0 {
            flight_ui::Command::View(view)
        } else {
            flight_ui::Command::ViewRelative(view, flight_views::Reference::Aircraft(id))
        })
    }

    fn flight_command(&mut self, command: flight_ui::Command) -> Action {
        use flight_ui::Command;
        if self.native_tables.is_some() && !self.flight_ui.cheats.no_turbulence {
            self.flight_ui.cheats.no_turbulence = true;
            self.flight_ui
                .message("Environmental turbulence is unavailable in native research flight");
        }
        match command {
            Command::AirportNav => {
                self.queue(seats::SeatCommand::Airport(world::AirportInput::NavMode));
                Action::None
            }
            Command::Airport(command) => {
                if !self.flight_ui.frozen() {
                    self.queue(seats::SeatCommand::Airport(world::AirportInput::Command(
                        command,
                    )));
                }
                Action::None
            }
            Command::WingRecipient(recipient) => {
                if !self.flight_ui.frozen() {
                    self.queue(seats::SeatCommand::WingRecipient(recipient));
                    self.flight_ui.message(recipient.map_or_else(
                        || "Orders address all wingmen".to_owned(),
                        |n| format!("Orders address wingman {n}"),
                    ));
                }
                Action::None
            }
            Command::WingFormationCycle => {
                if self.world.ai_wings.is_none() {
                    self.flight_ui.message("Wing order unavailable: no AI wing");
                } else if !self.flight_ui.frozen() {
                    self.queue(seats::SeatCommand::WingFormationCycle);
                }
                Action::None
            }
            Command::Wing(order) => {
                if !self.flight_ui.frozen() {
                    self.queue(seats::SeatCommand::WingOrder(order));
                }
                Action::None
            }
            Command::Bookmark => {
                match self.replay_recorder.as_mut() {
                    Some(recording) => {
                        let number = recording.bookmark();
                        self.flight_ui.message(format!("Bookmark {number} saved"));
                    }
                    None => self
                        .flight_ui
                        .message("Bookmark not saved: this flight is not being recorded"),
                }
                Action::None
            }
            Command::RadioSilence => {
                self.queue(seats::SeatCommand::RadioSilence);
                Action::None
            }
            Command::BattleNet => {
                self.queue(seats::SeatCommand::BattleNet);
                Action::None
            }
            Command::SensorConsole => {
                self.queue(seats::SeatCommand::SensorConsole);
                Action::None
            }
            Command::GunLock => {
                if !self.input.gunsight() {
                    self.flight_ui
                        .message("Gun lock: no gunsight on this aircraft");
                    return Action::Click;
                }
                self.queue(seats::SeatCommand::Console(
                    tore_sim::autopilot::orbit::ConsoleCommand::GunLock,
                ));
                Action::None
            }
            Command::SightPinToggle => {
                if !self.input.gunsight() {
                    self.flight_ui
                        .message("Pin or release: no gunsight on this aircraft");
                    return Action::Click;
                }
                self.queue(seats::SeatCommand::Console(
                    tore_sim::autopilot::orbit::ConsoleCommand::Pin,
                ));
                Action::None
            }
            // Retail's IFF squawk: the answer comes from the flight's own copy
            // of the mission, so a networked flight answers as single player does.
            Command::Iff => {
                if !self.flight_ui.frozen() {
                    let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
                    let sides = target_info::Sides {
                        roster: &self.world.roster,
                        wings: self.world.ai_wings.as_ref(),
                        scene: &self.world.terrain.airport_scene,
                    };
                    let answer = target_info::iff(
                        &sides,
                        frame.plane,
                        frame
                            .readout
                            .targets
                            .display
                            .as_ref()
                            .map(|target| target.id),
                    );
                    self.flight_ui.message(answer.message());
                }
                Action::None
            }
            // K opens and closes a networked flight's score board
            // (net/scoreboard.rs); single player has none and says so. The
            // wing replies' calls come with the reply slice, so until then
            // each says what it will do.
            Command::ScoreBoard => {
                if !self.flight_ui.frozen() && !net::scoreboard::toggle(&mut self.net_flight) {
                    self.flight_ui
                        .message(flight_ui::score_board_answer(self.flight_ui.session));
                }
                Action::None
            }
            // The four reply keys: in a networked flight the world answers
            // (the call to the flight, or "You lead this flight." for the
            // plane that leads it, which succession can change); single player
            // has no human wingman to call and says so itself.
            Command::Reply(reply) => {
                if !self.flight_ui.frozen() {
                    if self.flight_ui.session {
                        self.queue(seats::SeatCommand::WingReply(reply.world()));
                    } else {
                        self.flight_ui.message(flight_ui::SINGLE_PLAYER_REPLY);
                    }
                }
                Action::None
            }
            Command::DamageReport => {
                self.flight_ui.message(
                    self.world.cockpits[OWN]
                        .flight
                        .systems
                        .summary(self.world.cockpits[OWN].flight.damage_fraction),
                );
                for index in 1..36 {
                    if self.world.cockpits[OWN].flight.systems.has(index) {
                        self.flight_ui
                            .message(tore_sim::aircraft_systems::label(index));
                    }
                }
                let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
                for message in combat_view::equipment_damage_report(frame.config, &frame.readout) {
                    self.flight_ui.message(message);
                }
                Action::None
            }
            Command::None => Action::None,
            Command::Click => Action::Click,
            Command::End => Action::Back,
            Command::Exit => Action::Exit,
            Command::Restart if self.flight_ui.session => {
                self.flight_ui.message(flight_ui::RESTART_REFUSED);
                Action::Click
            }
            Command::Restart => Action::FreeFlight,
            // Retail Ctrl+V works only while flying an aircraft. The message is
            // an opinionated agent addition (2026-09-23).
            Command::Valkyries => {
                if !self.flight_ui.frozen()
                    && self.world.cockpits[OWN].flight.escape.is_none()
                    && !self.world.cockpits[OWN].flight.crashed
                    && let Some(on) = self.audio.as_ref().and_then(audio::Audio::toggle_valkyries)
                {
                    self.flight_ui.message(if on {
                        "Valkyries music on"
                    } else {
                        "Valkyries music off"
                    });
                }
                Action::None
            }
            Command::Eject => {
                if !self.flight_ui.frozen() {
                    self.input.queue(tore_input::PilotCommand::Eject);
                }
                Action::None
            }
            Command::Combat(command) => {
                use tore_sim::combat::live::Command as Live;
                if matches!(command, Live::ClearDesignation) {
                    // Also the way back from a weapon's seeker view to the
                    // targeting pod on the target camera.
                    self.seeker.dismiss();
                    self.instruments.keep_seeker_window();
                }
                if matches!(command, Live::NextGunGroup | Live::ToggleGunGroup) {
                    let Some(own) = self
                        .world
                        .combat
                        .state
                        .ownship(self.world.cockpits[OWN].plane.0)
                    else {
                        return Action::None;
                    };
                    let Some(mut group) = own.gunship.clone() else {
                        return Action::None;
                    };
                    let mut selected = own.selected;
                    if let Some(readout) = self
                        .net_flight
                        .as_ref()
                        .and_then(|flight| flight.frame.as_ref())
                        .and_then(|frame| frame.readout.as_ref())
                    {
                        group.included =
                            std::array::from_fn(|slot| readout.stores.gun_group & (1 << slot) != 0);
                        selected = readout.stores.selected();
                    }
                    if command == Live::NextGunGroup {
                        let stations: Vec<_> = group.stations.iter().flatten().copied().collect();
                        if stations.is_empty() {
                            return Action::None;
                        }
                        let next = stations
                            .iter()
                            .position(|station| *station == selected)
                            .map_or(0, |i| (i + 1) % stations.len());
                        selected = stations[next];
                    } else {
                        group.toggle(selected);
                    }
                    let names: Vec<_> = tore_sim::combat::gunship::NAMES
                        .into_iter()
                        .zip(group.included)
                        .filter_map(|(name, on)| on.then_some(name))
                        .collect();
                    let candidate = group
                        .slot(selected)
                        .map_or("NONE", |slot| tore_sim::combat::gunship::NAMES[slot]);
                    self.flight_ui.message(format!(
                        "Gun group: {}. Candidate: {candidate}",
                        if names.is_empty() {
                            "EMPTY".into()
                        } else {
                            names.join("+")
                        }
                    ));
                }
                if self.flight_ui.session
                    && matches!(
                        command,
                        Live::Incoming | Live::DamagePlayer | Live::ToggleTargetJammer
                    )
                {
                    // Range fixtures: they change the mission, which the
                    // server alone does.
                    self.flight_ui.message(
                        "That is a development command, not available in a multiplayer flight",
                    );
                } else {
                    self.queue(seats::SeatCommand::Manual(command));
                }
                Action::None
            }
            Command::Chaff | Command::Flare => {
                // A paused game releases nothing. The rest of the refusals
                // (destroyed, ejected, no hit points) are the tick's.
                if !self.flight_ui.frozen() {
                    self.queue(if command == Command::Chaff {
                        seats::SeatCommand::ReleaseChaff
                    } else {
                        seats::SeatCommand::ReleaseFlare
                    });
                }
                Action::None
            }
            Command::NextWeapon | Command::PreviousWeapon => {
                self.queue(seats::SeatCommand::CycleWeapon {
                    forward: command == Command::NextWeapon,
                });
                Action::None
            }
            // After a loss in a networked flight Enter flies again (slice
            // F2-V, net/play.rs); otherwise it designates.
            Command::TargetVisual if self.fly_again() => Action::None,
            // In the sensor console T and Shift+T step through the A-G radar
            // contacts the sensor can see, and zoom to each (slice nr-conb).
            Command::Target | Command::TargetPrevious if self.console_showing() => {
                self.console_target_key(command == Command::Target)
            }
            Command::Target | Command::TargetPrevious => {
                use tore_sim::combat::live::Command as Live;
                let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
                let sensors = &frame.readout.sensors;
                let controls = self.instruments.controls();
                // Preserve the ground and HARM pages' existing cycle.
                let command = if sensors.air_ground
                    || controls.channel != tore_sim::sensors::Channel::Radar
                {
                    Some(if command == Command::Target {
                        Live::Designate
                    } else {
                        Live::DesignatePrevious
                    })
                } else {
                    target_select::radar(sensors, controls, command == Command::Target)
                        .map(Live::DesignateTarget)
                };
                log::info!(
                    "Target selection: radar before={:?} requested={:?} range={}",
                    sensors.selected,
                    command,
                    controls.range_nmi()
                );
                drop(frame);
                if let Some(command) = command {
                    self.queue(seats::SeatCommand::Combat(command));
                } else {
                    self.flight_ui.message("No selectable radar contacts");
                }
                Action::None
            }
            Command::TargetVisual | Command::TargetCentre => {
                let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
                let centre = command == Command::TargetCentre;
                let mut forward = self.hornet.camera(frame.flight, 0, Default::default());
                forward.zoom = self.flight_ui.zoom;
                let camera = if centre { &forward } else { &self.camera };
                let picked = target_select::visual(
                    &frame.readout.visual,
                    frame.readout.targets.designated,
                    camera,
                    self.window_size,
                    centre,
                );
                log::info!(
                    "Target selection: visual centre={} before={:?} requested={:?}",
                    centre,
                    frame.readout.targets.designated,
                    picked
                );
                drop(frame);
                if let Some(id) = picked {
                    self.queue(seats::SeatCommand::Combat(
                        tore_sim::combat::live::Command::DesignateVisualTarget(id),
                    ));
                } else {
                    self.flight_ui.message("No visible aircraft");
                }
                Action::None
            }
            Command::RangeReset => {
                self.queue(seats::SeatCommand::RangeReset);
                Action::None
            }
            // Retail's Backslash designates the IR/laser target: the
            // AC-130's gunsight and the targeting sight of every aircraft
            // with a laser designator or a FLIR (slice T1). The others say
            // they have none. The pin key and the zoom keys act on the sight
            // alone.
            Command::SightDesignate | Command::SightPinGround if !self.has_sight() => {
                self.flight_ui
                    .message(if command == Command::SightDesignate {
                        "IR/laser designate: no targeting sight"
                    } else {
                        "Pin ground point: no targeting sight"
                    });
                Action::Click
            }
            Command::SightZoom(_) if !self.has_sight() => {
                self.flight_ui
                    .message("Bomb camera zoom: not implemented yet");
                Action::Click
            }
            Command::SightDesignate | Command::SightPinGround => {
                use tore_sim::combat::live::Command as Live;
                self.queue(seats::SeatCommand::Combat(match command {
                    Command::SightDesignate => Live::SightDesignate,
                    _ => Live::SightPinGround,
                }));
                Action::None
            }
            Command::SightZoom(steps) => {
                let before = self.input.sight().1;
                let zoom = self.input.zoom_sight(steps);
                if zoom == before {
                    self.flight_ui.message(format!(
                        "Gunsight zoom {zoom} of {}, the limit",
                        tore_input::sight::ZOOM_STEPS
                    ));
                } else {
                    self.flight_ui.message(format!(
                        "Gunsight zoom {zoom} of {}",
                        tore_input::sight::ZOOM_STEPS
                    ));
                }
                Action::None
            }
            Command::SoundOpen => {
                self.flight_ui.menu = true;
                self.input.context(true, self.focused);
                self.camera.keys.clear();
                self.release_trigger();
                self.flight_clock.remainder = 0.;
                self.open_sound(true);
                Action::Click
            }
            Command::GraphicsOpen => {
                self.flight_ui.menu = true;
                self.input.context(true, self.focused);
                self.camera.keys.clear();
                self.release_trigger();
                self.flight_clock.remainder = 0.;
                self.open_graphics("Flight paused");
                Action::Click
            }
            Command::RespawnAircraft | Command::RespawnLoadout => {
                self.open_respawn_menu(command == Command::RespawnAircraft);
                Action::Click
            }
            Command::FeedbackOpen => {
                if self.feedback_enabled() {
                    let tick = self
                        .net_flight
                        .as_ref()
                        .and_then(|f| f.frame.as_ref())
                        .map_or(0, |f| f.tick);
                    self.feedback = Some(feedback::Form::new(tick));
                    self.release_trigger();
                }
                Action::Click
            }
            Command::ControlsOpen => {
                self.flight_ui.menu = true;
                self.input.context(true, self.focused);
                self.camera.keys.clear();
                self.release_trigger();
                self.flight_clock.remainder = 0.;
                self.open_controls("Flight paused");
                Action::Click
            }
            Command::Toggle(switch) => {
                if switch == tore_input::Switch::Hook
                    && !self.world.cockpits[OWN].flight.hook_available()
                {
                    self.flight_ui.message("Hook unavailable for this aircraft");
                    return Action::None;
                }
                // Returning to the air radar comes first, so the radar power
                // switch is not spent leaving the passive page or A-G.
                if switch == tore_input::Switch::Radar && self.radar_key_returns() {
                    return Action::Click;
                }
                self.input.queue(tore_input::PilotCommand::Toggle(switch));
                Action::None
            }
            Command::InstrumentSelect(slot) => {
                if self.instruments.select(slot) {
                    self.flight_ui
                        .message(format!("Instrument {} selected", slot + 1));
                } else {
                    self.flight_ui.message("Instrument slot unavailable");
                }
                Action::None
            }
            Command::InstrumentCycle(delta) => {
                if self.instruments.cycle_selection(delta) {
                    self.flight_ui.message(format!(
                        "Instrument {} selected",
                        self.instruments.selected + 1
                    ));
                }
                Action::None
            }
            Command::InstrumentControl(button) => {
                if self.instruments.control(self.instruments.selected, button) {
                    self.queue_scope_commands();
                    Action::Click
                } else {
                    self.flight_ui.message("Instrument control unavailable");
                    Action::None
                }
            }
            Command::CenterLook => {
                self.flight_ui.look = [0.; 2];
                self.input.center_head();
                Action::None
            }
            Command::StoreView => {
                self.view_rig
                    .save(self.flight_view, self.flight_ui.look, self.flight_ui.zoom);
                if !self.instruments.pages.contains(&3) {
                    self.instruments.toggle(3);
                }
                self.instruments.cameras.remove(&3);
                self.flight_ui.message("Other View saved");
                Action::Click
            }
            Command::View(_) | Command::ViewRelative(_, _) => {
                let (mut view, reference) = match command {
                    Command::View(view) => (view, flight_views::Reference::Player),
                    Command::ViewRelative(view, reference) => (view, reference),
                    _ => unreachable!(),
                };
                let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
                let scene = if self.net_flight.is_some() {
                    flight_views::Scene::from_frame(
                        &frame,
                        frame.flight,
                        self.world.ai_wings.as_ref(),
                    )
                } else {
                    flight_views::Scene::new(
                        &frame,
                        &self.world.cockpits[OWN].flight,
                        &self.world.combat,
                        self.world.ai_wings.as_ref(),
                        None,
                    )
                };
                let scene = self
                    .view_rig
                    .prepare(scene, self.flight_view, &self.world.terrain);
                self.view_rig.observe(&scene);
                let holding = self.view_rig.holding_weapon() || self.view_rig.weapon_stopped();
                let previous = self.view_rig.missile_subject();
                let mut candidate = self.view_rig.clone();
                // Pressing F6 again moves on to the next wingman.
                let cycle = view == flight_views::WING
                    && self.flight_view == flight_views::WING
                    && self.view_rig.reference == reference;
                if cycle {
                    candidate.next_wingman();
                } else {
                    candidate.select(reference);
                    if holding && view == flight_views::MISSILE {
                        candidate.next_live_weapon(&scene, previous);
                    }
                }
                if let Err(reason) = candidate.camera(
                    view,
                    &scene,
                    self.hornet
                        .camera(&self.world.cockpits[OWN].flight, view, Default::default()),
                    [0.; 2],
                    1.,
                ) {
                    self.flight_ui.message(reason);
                    if !holding {
                        return Action::None;
                    }
                    view = 0;
                    candidate.select(flight_views::Reference::Player);
                }
                if view == flight_views::WING
                    && let Some(id) = candidate.wingman()
                {
                    let name = self
                        .world
                        .ai_wings
                        .as_ref()
                        .and_then(|wings| wings.radio_members().into_iter().find(|m| m.id == id))
                        .map_or_else(|| format!("Aircraft {id}"), |m| radio_calls::label(&m));
                    self.flight_ui.message(format!("Wingman view: {name}"));
                }
                self.view_rig = candidate;
                self.flight_view = view;
                self.flight_ui.look = [0.; 2];
                self.input.center_head();
                self.flight_ui.zoom = 1.;
                Action::Click
            }
            Command::WindowLayout => {
                self.instruments.toggle_layout();
                self.flight_ui
                    .message(if self.instruments.layout == instruments::Layout::Large {
                        "Large instruments: four corners"
                    } else {
                        "Small instruments: two bottom groups of three"
                    });
                Action::Click
            }
            Command::Panel(page) => {
                self.instruments.cancel_press();
                self.instruments.toggle(page);
                Action::Click
            }
            // In the sensor console the throttle keys set the console
            // autopilot's speed (slice nr-cona).
            Command::ThrottlePreset(..) | Command::ThrottleStep(_)
                if self.console_open() && console_keys::throttle(&command).is_some() =>
            {
                if let Some(console) = console_keys::throttle(&command) {
                    self.queue(seats::SeatCommand::Console(console));
                }
                Action::None
            }
            Command::ThrottlePreset(value, burner) => {
                self.input.queue(tore_input::PilotCommand::Throttle(value));
                self.input.queue(tore_input::PilotCommand::Set(
                    tore_input::Switch::Burner,
                    burner,
                ));
                Action::None
            }
            // On the helicopters and the V-22 the step moves the collective.
            Command::ThrottleStep(delta) if self.input.collective_role() => {
                self.input
                    .queue(tore_input::PilotCommand::AdjustThrottle(delta));
                Action::None
            }
            Command::ThrottleStep(delta) => {
                let (throttle, burner) = self.input.pending_throttle(
                    self.world.cockpits[OWN].flight.throttle,
                    self.world.cockpits[OWN].flight.burner,
                );
                let (throttle, burner) = input::fa_throttle_step(throttle, burner, delta);
                self.input
                    .queue(tore_input::PilotCommand::Throttle(throttle));
                self.input.queue(tore_input::PilotCommand::Set(
                    tore_input::Switch::Burner,
                    burner,
                ));
                Action::None
            }
            Command::Waypoint(next) => {
                self.instruments.navigation.pending.push(usize::from(next));
                Action::None
            }
            Command::Range(delta) => {
                if self.instruments.pages.last() == Some(&0) {
                    let scales = tore_sim::sensors::passive::SCALE_LADDER_NMI.len() as i32 - 1;
                    self.instruments.rcs_range =
                        (self.instruments.rcs_range as i32 + delta).clamp(0, scales) as usize;
                } else {
                    // The RWR and radar share one range setting.
                    self.instruments.step_range(delta);
                }
                Action::Click
            }
            Command::Mode => {
                // Cycles the available sensor channels: radar, infrared and,
                // with an anti-radiation weapon aboard, the HARM page. Radar
                // search and track modes follow the selected display range.
                let sensors = net::play::current_frame(&self.world, &self.net_flight, &self.net)
                    .readout
                    .sensors
                    .clone();
                let mut skipped_infrared = false;
                for _ in 0..instruments::CHANNELS {
                    self.instruments.cycle_channel();
                    let channel = self.instruments.controls().channel;
                    if sensors.available(channel) {
                        break;
                    }
                    skipped_infrared |= channel == tore_sim::sensors::Channel::Infrared;
                }
                if skipped_infrared {
                    self.flight_ui.message("No infrared sensor is installed.");
                }
                Action::Click
            }
            Command::SensorHistory => {
                self.instruments.history = !self.instruments.history;
                Action::Click
            }
            Command::SensorInfrared => {
                if net::play::current_frame(&self.world, &self.net_flight, &self.net)
                    .readout
                    .sensors
                    .available(tore_sim::sensors::Channel::Infrared)
                {
                    self.instruments.channel = 1;
                } else {
                    self.flight_ui.message("No infrared sensor is installed.");
                }
                Action::Click
            }
        }
    }
    /// R while the scope is off the air radar (the IR or HARM page, or A-G):
    /// back to the air radar, without spending the power switch. Retail R
    /// turns the air radar on and A-G off.
    fn radar_key_returns(&mut self) -> bool {
        let air_ground = net::play::current_frame(&self.world, &self.net_flight, &self.net)
            .readout
            .sensors
            .air_ground;
        if air_ground {
            self.instruments.channel = 0;
            self.queue(seats::SeatCommand::Combat(
                tore_sim::combat::live::Command::ToggleAirGround,
            ));
            self.flight_ui.message("Air-Air Radar on");
            return true;
        }
        if self.instruments.channel != 0 {
            self.instruments.channel = 0;
            return true;
        }
        false
    }
    /// Ctrl+R or Shift+R: the radar's air-to-ground page on or off
    /// (docs/spec/air-to-ground.md, "The air-to-ground radar").
    fn air_ground_key(&mut self) -> Action {
        let (working, on) = {
            let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
            (
                frame
                    .readout
                    .sensors
                    .available(tore_sim::sensors::Channel::Radar)
                    && !frame.readout.damage.radar_failed,
                !frame.readout.sensors.air_ground,
            )
        };
        if !working && on {
            self.flight_ui.message("No working radar found");
            return Action::Click;
        }
        if on {
            // The page is the radar's: the scope returns to it, and a radar
            // switched off comes on (retail's ground radar has no switch).
            self.instruments.channel = 0;
            if self.net_flight.is_none() && !self.world.cockpits[OWN].flight.radar {
                self.input
                    .queue(tore_input::PilotCommand::Toggle(tore_input::Switch::Radar));
            }
        }
        self.queue(seats::SeatCommand::Combat(
            tore_sim::combat::live::Command::ToggleAirGround,
        ));
        self.flight_ui.message(if on {
            "Air-Ground Radar on"
        } else {
            "Air-Ground Radar off"
        });
        Action::Click
    }
    /// Ctrl+X: the nearest hostile ground object, with the Easy targeting
    /// cheat (or the AC-130's sight) only.
    /// The aircraft in flight has a sight the sight keys act on: the
    /// AC-130's gunsight, or a targeting sight (a laser designator or a
    /// FLIR).
    fn has_sight(&self) -> bool {
        self.input.gunsight()
            || net::play::current_frame(&self.world, &self.net_flight, &self.net)
                .readout
                .sight
                .is_some()
    }
    /// Ctrl+L: the laser on or off by hand (docs/spec/air-to-ground.md, "The
    /// laser"). Says so on an aircraft without a designator.
    fn laze_key(&mut self) -> Action {
        let laser = net::play::current_frame(&self.world, &self.net_flight, &self.net)
            .readout
            .laser;
        let Some(laser) = laser.filter(|laser| laser.designator) else {
            self.flight_ui.message("Laser: no laser designator");
            return Action::Click;
        };
        use tore_sim::combat::laser::LaserState;
        let on = laser.manual.unwrap_or(matches!(
            laser.state,
            LaserState::Lasing | LaserState::Masked
        ));
        self.queue(seats::SeatCommand::Combat(
            tore_sim::combat::live::Command::ToggleLaser,
        ));
        self.flight_ui
            .message(if on { "Laser off" } else { "Laser on" });
        Action::None
    }
    /// The AC-130's sensor console is open on the presented flight
    /// ([`console_open`]).
    fn console_open(&self) -> bool {
        console_open(&self.input, &self.world, &self.net_flight)
    }
    /// Ctrl+Backslash: the AC-130 sensor console's picture, white hot, black
    /// hot, TV and night vision in turn. Only the console shows it; elsewhere
    /// the key does nothing.
    fn sight_polarity_key(&mut self) -> Action {
        let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
        if !sensor_console::console_active(&frame.readout, frame.flight) {
            return Action::Click;
        }
        self.console.mode = self.console.mode.cycled();
        Action::None
    }
    /// Shift+F5 to Shift+F8: the AC-130 sensor console's picture straight
    /// to white hot, black hot, TV or night vision (nr-replayviews,
    /// 2026-10-16). Only the console shows it; elsewhere the key does
    /// nothing.
    fn sight_picture_key(&mut self, mode: sensor_console::SensorMode) -> Action {
        let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
        if !sensor_console::console_active(&frame.readout, frame.flight) {
            return Action::Click;
        }
        self.console.mode = mode;
        Action::None
    }
    /// Ctrl+0: the sensor console radar's range, 5, 10 and 25 nm in turn.
    /// It is the flight A-G radar's own range setting, so the two match.
    /// Only the console has it; elsewhere the key does nothing.
    fn console_range_key(&mut self) -> Action {
        let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
        if !sensor_console::console_active(&frame.readout, frame.flight) {
            return Action::Click;
        }
        self.instruments.radar_range =
            sensor_console::next_radar_range(self.instruments.radar_range);
        Action::None
    }
    /// The A-G radar on, for the sensor console, as Ctrl+R turns it on (the
    /// scope back on the radar, a radar switched off on), when it is off and
    /// the radar works. Says why not otherwise.
    fn console_radar_on(&mut self) {
        let (working, on) = {
            let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
            (
                frame
                    .readout
                    .sensors
                    .available(tore_sim::sensors::Channel::Radar)
                    && !frame.readout.damage.radar_failed,
                frame.readout.sensors.air_ground,
            )
        };
        if on {
            return;
        }
        if !working {
            self.flight_ui.message("No working radar found");
            return;
        }
        self.air_ground_key();
    }
    /// T and Shift+T in the sensor console: the next (or the previous) A-G
    /// radar contact the sensor can see within the console's range
    /// ([`sensor_console::in_view`]), designated as T and a scope click
    /// designate (so the gunsight tracks it and the target camera shows it),
    /// and the sight zoomed so it fills the picture.
    fn console_target_key(&mut self, forward: bool) -> Action {
        let picked = {
            let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
            if !frame.readout.sensors.air_ground {
                None
            } else {
                let sides = target_info::Sides {
                    roster: &self.world.roster,
                    wings: self.world.ai_wings.as_ref(),
                    scene: &self.world.terrain.airport_scene,
                };
                let launcher = combat::launcher(frame.flight);
                let terrain = &self.world.terrain;
                let ground = |x: f64, z: f64| f64::from(terrain.height(x as f32, z as f32));
                let list = sensor_console::in_view(
                    &frame.readout.sensors,
                    launcher,
                    self.instruments.radar_range,
                    |from, to| tore_sim::combat::sight::clear_line(from, to, &ground),
                    |id| sides.friendly_to(frame.plane, id),
                );
                Some(
                    sensor_console::cycle(&list, frame.readout.targets.designated, forward).map(
                        |id| {
                            let eye = tore_sim::combat::gunship::eye_position(launcher);
                            let at = frame
                                .readout
                                .sensors
                                .ground
                                .iter()
                                .find(|g| g.id() == id)
                                .map_or(eye, |g| g.contact.position);
                            let slant =
                                (0..3).map(|i| (at[i] - eye[i]).powi(2)).sum::<f64>().sqrt();
                            // The object's size: the largest side of its box.
                            let extent = self
                                .world
                                .combat
                                .ground_object(id)
                                .map_or(sensor_console::DEFAULT_EXTENT_FT, |o| {
                                    2. * o.bounds.half.iter().copied().fold(0., f64::max)
                                });
                            (id, sensor_console::fill_zoom(extent, slant))
                        },
                    ),
                )
            }
        };
        match picked {
            None => {
                self.console_radar_on();
                Action::Click
            }
            Some(None) => {
                self.flight_ui.message("Sensor: no radar target in view");
                Action::Click
            }
            Some(Some((id, zoom))) => {
                self.queue(seats::SeatCommand::Combat(
                    tore_sim::combat::live::Command::DesignateTarget(id),
                ));
                let current = self.input.sight().1;
                self.input.zoom_sight(zoom as i8 - current as i8);
                Action::None
            }
        }
    }
    /// The sensor console is showing on the presented flight.
    fn console_showing(&self) -> bool {
        let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
        sensor_console::console_active(&frame.readout, frame.flight)
    }
    /// `/`: the sight tracks the next visible ground object.
    fn sight_next_key(&mut self) -> Action {
        if !self.has_sight() {
            self.flight_ui
                .message("Sight: next visible object: no targeting sight");
            return Action::Click;
        }
        self.queue(seats::SeatCommand::Combat(
            tore_sim::combat::live::Command::SightNext,
        ));
        Action::None
    }
    fn nearest_ground_key(&mut self) -> Action {
        if !self.flight_ui.cheats.easy_targeting && !self.input.gunsight() {
            self.flight_ui
                .message("Nearest ground target: Easy targeting cheat only");
            return Action::Click;
        }
        self.queue(seats::SeatCommand::Combat(
            tore_sim::combat::live::Command::NearestGround,
        ));
        Action::None
    }
    fn open_controls(&mut self, context: &'static str) {
        let mut editor = controls_editor::Editor::new(
            self.input.settings_profile(),
            self.input.devices.values().cloned().collect(),
            context,
        );
        editor.head_status = self.input.head_status();
        self.controls = Some(editor);
        self.mouse_look = None;
    }
    fn controls_result(&mut self, result: controls_editor::ResultAction) -> Action {
        use controls_editor::ResultAction;
        match result {
            ResultAction::None => Action::None,
            ResultAction::Changed => Action::Click,
            ResultAction::Save => {
                if let Some(editor) = &mut self.controls {
                    match self.input.save_settings(&editor.profile) {
                        Ok(()) => {
                            editor.message = "Controls saved and applied".into();
                            editor.saved();
                            if let Some(replay) = &mut self.replay {
                                replay
                                    .viewer
                                    .set_input_profile(&self.input.resolver.profile);
                            }
                        }
                        Err(e) => editor.message = e,
                    }
                }
                Action::Click
            }
            ResultAction::Close => {
                self.controls = None;
                if self.screen == Screen::Flight {
                    self.flight_ui.controls_closed();
                }
                self.menu.state.cancel();
                Action::Click
            }
        }
    }
    fn open_graphics(&mut self, context: &'static str) {
        // The renderer exists whenever the menu is shown; without one every
        // level is offered and the renderer clamps on creation.
        let supported = graphics::AntiAliasing::ALL
            .map(|level| self.renderer.as_ref().is_none_or(|r| r.supports(level)));
        let current = self
            .renderer
            .as_ref()
            .map_or(self.graphics, |r| r.graphics());
        let mut editor = graphics_screen::Editor::new(current, supported, context);
        if let Some(renderer) = &self.renderer {
            let size = renderer.window.inner_size();
            editor.set_window(Some([size.width, size.height]));
        }
        self.graphics_screen = Some(editor);
        self.mouse_look = None;
    }
    /// Pref > Sound... in the main menu, or over the paused flight menu.
    fn open_sound(&mut self, in_flight: bool) {
        self.sound_screen = Some(sound_screen::Screen::new(self.sound, in_flight));
        self.mouse_look = None;
    }
    fn sound_result(&mut self, outcome: sound_screen::Outcome) -> Action {
        use sound_screen::Outcome;
        let Some(screen) = &self.sound_screen else {
            return Action::None;
        };
        let draft = screen.draft;
        let in_flight = screen.in_flight;
        // Levels apply on OK, except Other music, heard as it moves.
        if let Some(audio) = &self.audio {
            match outcome {
                Outcome::Preview | Outcome::Cancel => audio.set_volumes(screen.heard().volumes()),
                Outcome::Save => audio.set_volumes(draft.volumes()),
                _ => {}
            }
        }
        match outcome {
            Outcome::None | Outcome::Redraw | Outcome::Preview => Action::None,
            Outcome::Save | Outcome::Cancel => {
                if outcome == Outcome::Save {
                    self.sound = draft;
                    if let Some(path) = &self.sound_path
                        && let Err(e) = self.sound.save(path)
                    {
                        log::warn!("Sound settings not saved: {e}");
                    }
                }
                self.sound_screen = None;
                if in_flight {
                    self.flight_ui.controls_closed();
                }
                self.menu.state.cancel();
                Action::Click
            }
        }
    }
    fn graphics_result(&mut self, result: controls_editor::ResultAction) -> Action {
        use controls_editor::ResultAction;
        match result {
            ResultAction::None => Action::None,
            ResultAction::Changed => Action::Click,
            ResultAction::Save => {
                self.settle_graphics(true);
                Action::Click
            }
            ResultAction::Close => {
                // Anything still settling is applied and saved on the way out.
                self.settle_graphics(true);
                self.graphics_screen = None;
                // Back to the paused flight's menu, as the Sound screen
                // returns.
                if self.screen == Screen::Flight {
                    self.flight_ui.controls_closed();
                }
                self.menu.state.cancel();
                Action::Click
            }
        }
    }
    /// The window's state for frame pacing. Scripted and measured runs,
    /// captures and smoke tests are never throttled.
    fn pace_window(&self) -> frame_pacing::Window {
        let hidden = self.renderer.as_ref().is_some_and(|renderer| {
            let size = renderer.window.inner_size();
            size.width == 0 || size.height == 0
        });
        frame_pacing::Window {
            focused: self.focused,
            hidden: self.occluded || hidden,
            automated: self.script.is_some()
                || self.performance.measuring()
                || self.capture_terrain.is_some()
                || self.smoke_test,
        }
    }
    /// When to draw again after a frame that wants `want`; None waits for
    /// an event. The frame-rate counter keeps every view drawing while it
    /// measures.
    /// A frame whose surface gave no image is tried again after a short,
    /// growing wait (`frame_pacing::surface_retry`), whatever it wanted.
    fn pace(&mut self, want: frame_pacing::Want) -> Option<Instant> {
        let renderer = self.renderer.as_ref()?;
        let limit = renderer.frame_limit();
        let want = frame_pacing::counted(want, renderer.counter_shown());
        let retry = renderer.surface_retry();
        let window = self.pace_window();
        let now = Instant::now();
        let next = self.pacer.next(limit, want, window, now);
        match retry {
            Some(wait) => frame_pacing::after_surface_failure(
                self.pacer.soonest(limit, window, now),
                wait,
                now,
            ),
            None => next,
        }
    }
    /// Whether the frame-rate counter is shown, which keeps every screen
    /// drawing at the frame rate setting so it measures real frames.
    fn counter_shown(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.counter_shown())
    }
    /// Whether a redraw request arriving now waits for the frame timer
    /// (`frame_pacing::too_soon`): with the counter shown under a capped rate,
    /// or while the window is unfocused, events add no frames beyond the
    /// timer's.
    fn redraw_too_soon(&self) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        frame_pacing::too_soon(
            renderer.frame_limit(),
            renderer.counter_shown(),
            self.pace_window(),
            self.next_frame,
            Instant::now(),
        )
    }
    /// What a flight wants after its frame: the next frame, or, while a
    /// single-player flight is stopped and its picture has settled, nothing
    /// until an event or the next notice expiring. `moving` is an overlay
    /// that animated this frame.
    fn flight_want(&mut self, moving: bool) -> frame_pacing::Want {
        let pose = frame_pacing::Pose::of(&self.camera, self.flight_view);
        let moving = moving
            || !self.camera.keys.is_empty()
            || self.mouse_look.is_some()
            || self.flight_ui.map.open
            || self.pace_window().automated
            || self.renderer.as_ref().is_some_and(|r| r.counter_shown());
        if self.stillness.still(self.flight_ui.stopped(), pose, moving) {
            self.flight_ui
                .notices_until()
                .map_or(frame_pacing::Want::Nothing, frame_pacing::Want::At)
        } else {
            frame_pacing::Want::Live
        }
    }
    /// Applies the Graphics screen's pending changes and saves them, once
    /// they have settled (`graphics_screen::SETTLE` after the last one) or
    /// at once with `now`. Every change on the screen comes through here, so
    /// stepping quickly through anti-aliasing or vegetation choices rebuilds
    /// the renderer's pipelines and buffers once, for the value kept.
    /// Renderers created later start from `self.graphics`, and the world
    /// rebuild on entering flight keeps the renderer's copy.
    fn settle_graphics(&mut self, now: bool) {
        let Some(editor) = &mut self.graphics_screen else {
            return;
        };
        if let Some(renderer) = &self.renderer {
            let size = renderer.window.inner_size();
            editor.set_window(Some([size.width, size.height]));
        }
        let due = if now {
            editor.dirty()
        } else {
            editor.apply_due(Instant::now())
        };
        if !due {
            return;
        }
        self.graphics = editor.apply(self.graphics_path.as_deref());
        if let Some(renderer) = &mut self.renderer {
            renderer.set_graphics(self.graphics);
            renderer.window.request_redraw();
        }
    }
    /// Opens the Replays screen on the recordings folder. Recording may be
    /// off for this run; the folder is still listed.
    fn open_replays(&mut self, context: &'static str) {
        let library = self.replay_library.clone().or_else(|| {
            assets::data_directory()
                .ok()
                .map(|data| replay::library::Library::new(&data))
        });
        self.replays_screen = Some(replay::screen::Replays::open(library, context));
        self.mouse_look = None;
    }
    /// Shows the Replays screen after the replay viewer, with the list read
    /// again, opening it when the viewer was started from the command line.
    /// Called when the replay viewer closes (replay/host.rs).
    fn return_to_replays(&mut self) {
        match &mut self.replays_screen {
            Some(screen) => screen.refresh(),
            None => self.open_replays("Main menu"),
        }
    }
    fn replays_result(&mut self, outcome: replay::screen::Outcome) -> Action {
        use replay::screen::Outcome;
        match outcome {
            Outcome::None => Action::None,
            Outcome::Changed => Action::Click,
            // The screen stays open under the viewer, which returns to it.
            Outcome::Watch(path) => Action::WatchReplay(path),
            Outcome::Close => {
                self.replays_screen = None;
                self.menu.state.cancel();
                Action::Click
            }
        }
    }
    /// Builds the Quick Mission the creator describes and the screen's
    /// beside it: the creator's draft becomes a `MissionSpec` with stable
    /// names, `World::build` makes the mission from it, and the drawn models
    /// of the aircraft it loaded and the combat art are made for the new
    /// world. The error is the line the pilot reads on the Load Ordnance page.
    fn build_mission(&mut self) -> Result<(), String> {
        let mut spec = self.quick.mission_spec()?;
        spec.share_flight_loadout = true;
        spec.cheats = self.flight_ui.cheats;
        spec.researched_flight = self.researched_flight;
        spec.ai_flight_model = ai_wings::AiFlightModel::Standard;
        spec.enemy_skill = self.enemy_skill;
        spec.fixture_wings = !self.ai_wings_enabled;
        spec.loadout = Some(mission::LoadoutSpec::of(
            &self
                .quick
                .ordnance
                .as_ref()
                .ok_or("Open the Load Ordnance page first.")?
                .loadout,
        ));
        spec.weather = scenery::launch_overrides().map_err(|error| error.to_string())?;
        let brief_sentences = self.quick.sentences(&menu::flat_font([232, 233, 230]));
        let resources = Arc::clone(&self.theater_resources);
        let player = Arc::clone(&self.hornet.kind);
        let graphics = self
            .renderer
            .as_ref()
            .map(renderer::Renderer::loading_factory);
        let check_restart = self.smoke_test && self.creator_loading_view.is_some();
        let start_recording = self.recording_starter();
        // Automatic launch keeps the selection bar. A load of its own, such
        // as Fly after a custom loadout, opens on Building the mission.
        let progress = self
            .mission_loading_progress
            .take()
            .unwrap_or_else(loading::Progress::mission);
        self.loading = Some(loading::Job::continue_load(
            Some(progress),
            true,
            move |progress| {
                let (mut built, view, mut scenery) =
                    loading::prepare_mission(&spec, &resources, Arc::clone(&player), &progress)?;
                if check_restart {
                    built.restarted =
                        loading::verify_restart(&mut built.world, &player, &resources)?;
                }
                let graphics = if let Some(build) = graphics {
                    let aircraft = aircraft::Airframe::load(&resources, spec.player())
                        .map_err(|e| e.to_string())?;
                    let geometry = Arc::clone(scenery.static_geometry(
                        &built.world.combat.state.targets,
                        &built.world.combat.surface,
                    ));
                    Some(build(
                        renderer::LoadingScene {
                            scenery: &scenery,
                            world: &built.world.terrain,
                            eye: built.world.cockpits[OWN].flight.position,
                        },
                        &aircraft,
                        &view.models,
                        &view.art,
                        Some(&geometry),
                    ))
                } else {
                    None
                };
                let recording = start_recording(&mut built.world);
                Ok(loading::Prepared::Mission(Box::new((
                    built,
                    view,
                    scenery,
                    graphics,
                    recording,
                    brief_sentences,
                ))))
            },
        )?);
        Ok(())
    }

    fn finish_creator_launch(&mut self, event_loop: &ActiveEventLoop) {
        let Some((view, reference, look, zoom)) = self.creator_loading_view.take() else {
            return;
        };
        self.flight_view = view;
        self.view_rig.select(reference);
        self.flight_ui.look = look;
        self.flight_ui.zoom = zoom;
        if self.world.setup.mission.is_none() && self.error.is_none() {
            self.error = Some(
                self.quick
                    .ordnance
                    .as_ref()
                    .and_then(|o| o.message.clone())
                    .or_else(|| self.quick.notice.clone())
                    .unwrap_or_else(|| "Quick Mission could not launch the selected setup".into())
                    .into(),
            );
            event_loop.exit();
        } else if self.error.is_none() {
            let frame = net::play::current_frame(&self.world, &self.net_flight, &self.net);
            let listed = combat_view::readout(
                &self.world.combat,
                frame.config,
                &frame.readout,
                frame.flight,
                self.instruments.controls(),
                1.,
            )
            .weapons;
            println!(
                "Quick Mission launch: ground={:?} player_position={:?} supported={} airborne_targets={} parked_targets={} enemy_nm={:.1} ammo={:?} listed={:?}",
                self.world.setup.ground_start,
                self.world.cockpits[OWN].flight.position,
                self.world.cockpits[OWN].flight.supported_at(
                    self.world
                        .terrain
                        .surface(
                            self.world.cockpits[OWN].flight.position[0],
                            self.world.cockpits[OWN].flight.position[2]
                        )
                        .height
                ),
                self.world
                    .combat
                    .state
                    .targets
                    .iter()
                    .filter(|t| t.airborne && !t.on_ground)
                    .count(),
                self.world
                    .combat
                    .state
                    .targets
                    .iter()
                    .filter(|t| t.on_ground)
                    .count(),
                self.world
                    .combat
                    .mission_layout
                    .as_ref()
                    .map_or(0., |l| l.enemy.distance_ft / mission_layout::FEET_PER_NM),
                frame.readout.stores.ammo,
                listed,
            );
        }
    }

    /// Starts a flight: what ends the old one, the world (a free flight
    /// restarted, or the Quick Mission just built), and what a new flight
    /// resets in the app.
    fn begin_flight(
        &mut self,
        event_loop: &ActiveEventLoop,
        mission: Option<(world::Built, combat_view::CombatView)>,
    ) {
        let _timing = loading::timing("flight installation");
        // A flight still recording is being restarted.
        let restarted =
            std::mem::take(&mut self.restart_recorded) || self.replay_recorder.is_some();
        self.finish_replay_recording("restart");
        if let Some(audio) = &self.audio {
            audio.restart_flight();
        }
        if self.recorded_ticks > 0 {
            self.finish_recording();
        }
        self.input.context(true, self.focused);
        // The old mission's trace is closed first, so its last rows
        // land before the new header.
        self.formation_trace = None;
        let restarted_flight = match mission {
            // The Quick Mission was built whole, and started once, by
            // `World::build`. The renderer draws the new world's terrain.
            Some((built, view)) => {
                self.mission_markup =
                    Some(quick_mission::map::mission_plan(&built.world, false, None));
                self.world.0 = Some(built.world);
                self.combat_view = view;
                let prepared = self.prepared_scenery.take();
                let (scenery, graphics) = match prepared {
                    Some((scenery, graphics)) => (Ok(scenery), graphics),
                    None => (
                        scenery::Scenery::build(&self.theater_resources, &self.world.terrain),
                        None,
                    ),
                };
                match scenery {
                    Ok(scenery) => self.scenery = scenery,
                    Err(error) => {
                        self.error = Some(error);
                        event_loop.exit();
                        return;
                    }
                }
                let retired = if let Some(renderer) = &mut self.renderer {
                    if let Some(graphics) = graphics {
                        renderer.install_loading(graphics)
                    } else {
                        renderer.set_scenery(&self.scenery);
                        renderer.prepare_aircraft(&self.hornet);
                        None
                    }
                } else {
                    None
                };
                if let Some(handle) = retired {
                    self.shelve_background(handle);
                }
                built.restarted
            }
            None => match self
                .prepared_restart
                .take()
                .map(Ok)
                .unwrap_or_else(|| self.world.restart(&self.hornet, &*self.theater_resources))
            {
                Ok(restarted) => restarted,
                Err(error) => {
                    self.error = Some(error);
                    event_loop.exit();
                    return;
                }
            },
        };
        // A flight starts on fresh camera weather, as on a fresh clock.
        self.scenery.reset_presentations();
        if let Some(wings) = &mut self.world.ai_wings {
            match formation_trace::start(wings) {
                Ok(trace) => self.formation_trace = trace,
                Err(error) => {
                    self.error = Some(error.into());
                    event_loop.exit();
                    return;
                }
            }
        }
        self.seat_commands.clear();
        self.gun_key_sent = false;
        self.cheats_sent = None;
        self.instruments.navigation = navigation::Navigation::default();
        match restarted_flight.ai_aircraft {
            Some(0) => self
                .flight_ui
                .message("AI wings: no aircraft in this setup"),
            Some(count) => self
                .flight_ui
                .message(format!("AI wings: {count} aircraft")),
            None => {}
        }
        let layout = restarted_flight.layout;
        // Every flight records itself from this picture on.
        self.start_replay_recording();
        if restarted && let Some(recording) = &mut self.replay_recorder {
            recording.note(tore_replay::Event::new(
                tore_replay::vocab::kind::SYSTEM_RESTART,
            ));
        }
        self.flight_music = flight_music::Observer::new();
        self.rwr_warnings = Default::default();
        self.launch_watch = Default::default();
        self.reset_vapor();
        self.g_effects = Default::default();
        self.flight_clock.remainder = 0.;
        self.flight_view = 0;
        self.view_rig = flight_views::Rig::for_plane(self.plane());
        let saved = preferences::Preferences::capture(
            &self.flight_ui,
            &self.instruments,
            self.fullscreen_preference,
        );
        // Automatic creator captures keep the requested map page across loading.
        let map_page = self
            .creator_loading_view
            .as_ref()
            .map(|_| (self.flight_ui.map.open, self.flight_ui.map.mission_tab));
        self.flight_ui.reset_for_flight();
        if let Some((open, mission)) = map_page {
            self.flight_ui.map.open = open;
            self.flight_ui.map.mission_tab = mission;
        }
        self.live_debug.reset();
        saved.apply(&mut self.flight_ui, &mut self.instruments);
        if self.world.setup.ground_start.is_some() {
            self.flight_ui
                .message("Ground start: B releases brakes; 5 sets full throttle.");
        }
        if let Some(notice) = layout.as_ref().and_then(|l| l.notice()) {
            self.flight_ui.message(notice);
        }
        self.screen = Screen::Flight;
        self.camera.keys.clear();
        self.world.combat.cancel();
        self.quick.cancel();
        self.frame_time = Instant::now();
        // A loader completion enters here without going through Action's
        // shared menu/title update at the end of the input callback.
        if let Some(renderer) = &self.renderer {
            renderer.window.set_title(&format!(
                "T.O.R.E-Fighters - {} Free Flight",
                self.hornet.profile.id.label()
            ));
            renderer.window.request_redraw();
        }
        if let Some(audio) = &self.audio {
            audio.scene(audio::music::Scene::Score(0));
        }
    }

    fn action(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        if self.confirm_session_leave(&action) {
            return;
        }
        if action == Action::Exit
            && self
                .net
                .as_ref()
                .is_some_and(|s| !s.client.flight_record().sorties.is_empty())
        {
            self.exit_after_net_debrief = true;
            self.leave_direct_session(event_loop);
            if self.quick.debrief.is_some() {
                return;
            }
        }
        if action == Action::Exit
            || (self.exit_after_net_debrief && self.net.is_none() && self.quick.debrief.is_none())
        {
            self.finished = true;
            event_loop.exit();
            return;
        }
        // A page opened over the lobby (the creator, Load Ordnance) answers to
        // the lobby: Accept and Cancel.
        let action = self.lobby_page_action(action);
        // End Mission in a networked flight leaves the session: the host
        // answers with the debrief and ends the connection.
        if action == Action::Back && self.net_flight.is_some() && self.screen == Screen::Flight {
            self.leave_session();
            return;
        }
        match action {
            Action::LeaveFeedback | Action::OpenUpdate => {
                let url = if action == Action::LeaveFeedback {
                    Some(updates::FEEDBACK_URL)
                } else {
                    self.updates.available.as_ref().map(|r| r.url.as_str())
                };
                if let Some(url) = url
                    && let Err(error) = updates::open_page(url)
                {
                    self.menu.state.toast =
                        Some((format!("Could not open browser: {error}"), Instant::now()));
                }
            }
            Action::ToggleUpdates => match self.updates.toggle() {
                Ok(()) => {
                    self.menu.state.updates_enabled = self.updates.enabled;
                    let version = self
                        .updates
                        .available
                        .as_ref()
                        .filter(|_| self.updates.enabled)
                        .map(|r| r.version.to_string());
                    self.menu.update_link(version.as_deref());
                }
                Err(error) => {
                    self.menu.state.toast = Some((
                        format!("Could not save update setting: {error}"),
                        Instant::now(),
                    ))
                }
            },
            Action::Replays => self.open_replays("Main menu"),
            Action::Controls => self.open_controls("Main menu"),
            Action::Graphics => self.open_graphics("Main menu"),
            Action::Sound => self.open_sound(false),
            Action::MultiplayerMain => self.multiplayer_main(event_loop),
            Action::Direct => self.open_direct(),
            Action::DirectClose => self.close_direct(),
            Action::DirectLeave => self.leave_direct_session(event_loop),
            Action::AwayLeave => self.away_leave_game(event_loop),
            Action::Internet => self.open_internet(),
            Action::InternetClose => self.close_internet(),
            Action::WatchReplay(ref path) => self.watch_replay(path),
            Action::ReimportMedia => {
                // The pack on disk is still valid here, so the menu the player
                // is looking at becomes the locate screen's background.
                self.reimport = Some(self.menu.pixels.clone());
                self.finished = true;
                event_loop.exit();
                return;
            }
            Action::QuickMission => {
                self.screen = Screen::Quick;
                self.menu.state.cancel();
            }
            Action::Mission => {
                let Some(id) = self.quick.player() else {
                    return;
                };
                if self.quick.theater_code().is_none() {
                    return;
                }
                if !std::mem::take(&mut self.mission_selection_ready) {
                    if let Err(error) = self.finish_combat_tape() {
                        self.error = Some(error);
                        event_loop.exit();
                        return;
                    }
                    let resources = Arc::clone(&self.theater_resources);
                    // Standard loadout launches at once. Keep this bar and
                    // photo for that single mission build. A custom loadout
                    // ends here; Fly starts its own load.
                    let automatic = self.quick.draft.values[18] == 0;
                    // Ready would say Starting flight. This load only prepares
                    // the loadout page, or hands the same bar to the mission.
                    let work = move |_: loading::Progress| {
                        aircraft::Airframe::load(&resources, id)
                            .map(|aircraft| loading::Prepared::Selection(Box::new(aircraft)))
                            .map_err(|e| e.to_string())
                    };
                    // A custom loadout only opens the Load Ordnance page: the
                    // creator stays up unless the load runs long, not a
                    // photograph for a few frames.
                    let job = if automatic {
                        loading::Job::continue_load(None, false, work)
                    } else {
                        loading::Job::continue_from_menu(work)
                    };
                    match job {
                        Ok(job) => {
                            self.mission_loading_progress = automatic.then(|| job.progress.clone());
                            self.loading = Some(job);
                        }
                        Err(error) => {
                            self.error = Some(error.into());
                            event_loop.exit();
                        }
                    }
                    return;
                }
                let result = (|| -> AppResult<()> {
                    // Standard load rebuilds the standard load. A custom one
                    // opens on the player's earlier edits, a load flown for
                    // this very mission before, or the standard load.
                    let fingerprint = self.quick.mission_fingerprint();
                    let custom = self.quick.draft.values[18] != 0;
                    let session_has_one = self
                        .quick
                        .ordnance
                        .as_ref()
                        .is_some_and(|o| o.loadout.aircraft == id);
                    let choice = match (&self.quick_loadouts, custom) {
                        (_, false) => quick_mission::loadout_memory::Choice::Standard,
                        (Some(memory), true) => memory.choose_compatible(
                            &fingerprint,
                            &self.quick.legacy_airport_fingerprints(),
                            session_has_one,
                        ),
                        (None, true) if session_has_one => {
                            quick_mission::loadout_memory::Choice::Session
                        }
                        (None, true) => quick_mission::loadout_memory::Choice::Standard,
                    };
                    if choice != quick_mission::loadout_memory::Choice::Session {
                        let read = |name: &str| {
                            self.theater_resources.get(name).cloned().ok_or_else(|| {
                                std::io::Error::other(format!("missing loadout resource {name}"))
                            })
                        };
                        let mut load =
                            tore_sim::combat::loadout::Loadout::new(&self.hornet.profile, read)?;
                        if let quick_mission::loadout_memory::Choice::Remembered(spec) = &choice {
                            // Keep valid overweight loads open for adjustment;
                            // Fly still enforces the takeoff-weight limit.
                            // Invalid records fall back to the standard load.
                            match spec
                                .apply(load.clone(), &*self.theater_resources, None)
                                .and_then(|kept| {
                                    kept.validate_for_editing()
                                        .map_err(Into::into)
                                        .map(|()| kept)
                                }) {
                                Ok(kept) => load = kept,
                                Err(error) => log::warn!(
                                    "Quick Mission: the saved loadout no longer fits: {error}"
                                ),
                            }
                        }
                        self.quick.ordnance =
                            Some(ordnance::Ordnance::new(load, &self.theater_resources)?);
                    }
                    if custom && let Some(memory) = &mut self.quick_loadouts {
                        memory.opened_for(&fingerprint);
                    }
                    if let Some(choice) = self.quick_loadout.clone()
                        && let Some(o) = self.quick.ordnance.as_mut()
                    {
                        if choice == "none" {
                            o.loadout.quantities.fill(0);
                            if let Err(error) = o.loadout.clear_tanks() {
                                o.message = Some(error.to_string());
                            }
                        } else {
                            o.loadout.restrict_to_guns();
                        }
                    }
                    let guns_only = self.quick.guns_only();
                    let o = self.quick.ordnance.as_mut().unwrap();
                    if guns_only {
                        o.loadout.restrict_to_guns();
                    }
                    o.visible = true;
                    o.message = None;
                    Ok(())
                })();
                if let Err(e) = result {
                    self.quick.notice = Some(e.to_string());
                } else if self.quick.draft.values[18] == 0 {
                    self.action(event_loop, Action::MissionFly);
                }
            }
            Action::MissionFly => {
                if let Some(message) = self.quick.unsupported() {
                    self.quick.ordnance.as_mut().unwrap().message = Some(message);
                    return;
                }
                if self.native_tables.is_some() {
                    if self.quick.ground_start() {
                        self.quick.ordnance.as_mut().unwrap().message=Some("Ground start requires the researched flight model. Choose Airborne for this adapter.".into());
                    } else {
                        self.error = Some(
                            "native research flight currently requires clean free flight".into(),
                        );
                        event_loop.exit();
                    }
                    return;
                }
                match self.build_mission() {
                    Ok(()) => {}
                    Err(message) => self.quick.ordnance.as_mut().unwrap().message = Some(message),
                }
            }
            Action::FreeFlight => {
                self.restart_recorded = self.replay_recorder.is_some();
                self.finish_replay_recording("restart");
                if self.recorded_ticks > 0 {
                    self.finish_recording();
                }
                let start_recording = self.recording_starter();
                let mut world = self.world.0.take().expect("active world");
                let resources = Arc::clone(&self.theater_resources);
                let aircraft = Arc::clone(&self.hornet.kind);
                match loading::Job::spawn(move |_| {
                    let restarted = world
                        .restart(&aircraft, &*resources)
                        .map_err(|e| e.to_string())?;
                    let recording = start_recording(&mut world);
                    Ok(loading::Prepared::Restart(Box::new((
                        world, restarted, recording,
                    ))))
                }) {
                    Ok(job) => self.loading = Some(job),
                    Err(message) => {
                        self.error = Some(message.into());
                        event_loop.exit();
                    }
                }
            }
            Action::Back => {
                if self.screen == Screen::Flight && self.world.setup.mission.is_some() {
                    if let Some(report) = debrief::capture(&self.world, SEAT) {
                        match debrief::Debrief::new(report, &self.theater_resources, None) {
                            Ok(debrief) => self.quick.debrief = Some(debrief),
                            Err(error) => self.quick.notice = Some(error.to_string()),
                        }
                    }
                    if let Some(ordnance) = &mut self.quick.ordnance {
                        ordnance.visible = false;
                    }
                }
                // After the debrief, before the wings go: the footer carries
                // the same result.
                if self.screen == Screen::Flight {
                    self.finish_replay_recording(if self.world.setup.mission.is_some() {
                        "end mission"
                    } else {
                        "end flight"
                    });
                }
                self.world.ai_wings = None;
                self.formation_trace = None;
                self.world.roster.set_wing_recipient(SEAT, None);
                self.world.combat.ai_poses = false;
                if let Err(e) = self.finish_combat_tape() {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
                if let Some(renderer) = &mut self.renderer {
                    renderer.combat(&Default::default());
                }
                if self.recorded_ticks > 0 {
                    self.finish_recording();
                }
                self.input.context(true, self.focused);
                self.screen = if matches!(self.screen, Screen::Viewer | Screen::Flight) {
                    Screen::Quick
                } else {
                    Screen::Main
                };
                self.camera.keys.clear();
                self.world.combat.cancel();
                self.quick.cancel();
            }
            _ => {}
        }
        if self.loading.is_some() {
            self.input.context(true, self.focused);
            if let Some(audio) = &self.audio {
                audio.action(action);
            }
            if let Some(renderer) = &self.renderer {
                renderer.window.request_redraw();
            }
            return;
        }
        if let Some(renderer) = &self.renderer {
            renderer.window.set_title(&match self.screen {
                Screen::Flight => format!(
                    "T.O.R.E-Fighters - {} Free Flight",
                    self.hornet.profile.id.label()
                ),
                Screen::Main if self.lobby.screen.is_some() => {
                    "T.O.R.E-Fighters - Lobby".to_string()
                }
                Screen::Main if self.direct.screen.is_some() => {
                    "T.O.R.E-Fighters - Direct Network Lounge".to_string()
                }
                Screen::Main if self.internet.screen.is_some() => {
                    "T.O.R.E-Fighters - Internet Lounge".to_string()
                }
                Screen::Main => "T.O.R.E-Fighters - Choose Activity".to_string(),
                Screen::Quick => "T.O.R.E-Fighters - Quick Mission Creator".to_string(),
                Screen::Viewer => format!(
                    "T.O.R.E-Fighters - {} Terrain Viewer",
                    self.world.terrain.theater.name
                ),
                Screen::Replay => self
                    .replay
                    .as_ref()
                    .map_or_else(String::new, |r| r.viewer.title()),
            });
        }
        if let Some(audio) = &self.audio {
            audio.scene(match self.screen {
                // A replay has no music: nothing records the music's situation.
                Screen::Flight | Screen::Replay => audio::music::Scene::Score(0),
                Screen::Main => audio::music::Scene::Main,
                _ => audio::music::Scene::Brief,
            });
            audio.action(action);
        }
        self.save_preferences();
        if let Some(renderer) = &self.renderer {
            renderer.window.set_cursor_visible(
                !(self.screen == Screen::Quick
                    && self
                        .quick
                        .ordnance
                        .as_ref()
                        .is_some_and(|o| o.visible && o.dragging())),
            );
            renderer.window.set_cursor(
                if (self.screen == Screen::Main && self.menu.state.hover.is_some())
                    || (self.screen == Screen::Quick
                        && self
                            .quick
                            .debrief
                            .as_ref()
                            .map_or(self.quick.hover.is_some(), |d| d.hovering()))
                {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                },
            );
            renderer.window.request_redraw();
        }
    }
}
impl App {
    /// Runs the next due step of `--input-script`, if there is one.
    fn run_script(&mut self, event_loop: &ActiveEventLoop) {
        use input_script::Step;
        use winit::dpi::PhysicalPosition;
        use winit::event::{DeviceId, Modifiers, MouseScrollDelta, TouchPhase};
        let Some(mut runner) = self.script.take() else {
            return;
        };
        let Some(renderer) = &self.renderer else {
            self.script = Some(runner);
            return;
        };
        let id = renderer.window.id();
        let viewport = renderer.viewport();
        renderer.window.request_redraw();
        // The script stands in for a focused player, whatever the compositor says.
        self.focused = true;
        let device_id = DeviceId::dummy();
        let set_mods = |app: &mut App, runner: &mut input_script::Runner, mods: ModifiersState| {
            if runner.mods != mods {
                runner.mods = mods;
                app.window_event(
                    event_loop,
                    id,
                    WindowEvent::ModifiersChanged(Modifiers::from(mods)),
                );
            }
        };
        if let Some((spec, _)) = runner.release.take() {
            let action = self.key_input(event_loop, spec.input(false));
            self.action(event_loop, action);
            set_mods(self, &mut runner, ModifiersState::empty());
            self.script = Some(runner);
            return;
        }
        // A networked flight counts the client's ticks; the world's never move.
        let tick = match self.net_flight.as_ref().and_then(|f| f.frame.as_ref()) {
            Some(frame) => frame.tick,
            None => self.world.combat.state.tick(),
        };
        if let Some(step) = runner.due(tick, Instant::now()) {
            let mouse = |app: &mut App, event: WindowEvent| app.window_event(event_loop, id, event);
            match step {
                Step::Wait(_) | Step::WaitTick(..) => {}
                Step::Stall(seconds) => {
                    // The whole loop stops, as a long frame or a window held
                    // still does; the session's keepalive thread carries on.
                    println!("Input script: stalling {seconds} s");
                    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                    println!("Input script: stall over");
                }
                Step::Tap(spec) => {
                    set_mods(self, &mut runner, spec.mods);
                    let action = self.key_input(event_loop, spec.input(true));
                    self.action(event_loop, action);
                    runner.release = Some((spec, false));
                }
                Step::Down(spec) => {
                    set_mods(self, &mut runner, spec.mods);
                    let action = self.key_input(event_loop, spec.input(true));
                    self.action(event_loop, action);
                }
                Step::Up(spec) => {
                    let action = self.key_input(event_loop, spec.input(false));
                    self.action(event_loop, action);
                    set_mods(self, &mut runner, ModifiersState::empty());
                }
                Step::Move(x, y) => mouse(
                    self,
                    WindowEvent::CursorMoved {
                        device_id,
                        position: PhysicalPosition::new(x, y),
                    },
                ),
                Step::MoveMenu(x, y) => {
                    let (x, y) = (
                        f64::from(viewport.x) + x * f64::from(viewport.width) / menu::WIDTH as f64,
                        f64::from(viewport.y)
                            + y * f64::from(viewport.height) / menu::HEIGHT as f64,
                    );
                    mouse(
                        self,
                        WindowEvent::CursorMoved {
                            device_id,
                            position: PhysicalPosition::new(x, y),
                        },
                    );
                }
                Step::Press(button) => mouse(
                    self,
                    WindowEvent::MouseInput {
                        device_id,
                        state: ElementState::Pressed,
                        button,
                    },
                ),
                Step::Release(button) => mouse(
                    self,
                    WindowEvent::MouseInput {
                        device_id,
                        state: ElementState::Released,
                        button,
                    },
                ),
                Step::Click(button) => {
                    for state in [ElementState::Pressed, ElementState::Released] {
                        mouse(
                            self,
                            WindowEvent::MouseInput {
                                device_id,
                                state,
                                button,
                            },
                        );
                    }
                }
                Step::Wheel(notches) => mouse(
                    self,
                    WindowEvent::MouseWheel {
                        device_id,
                        delta: MouseScrollDelta::LineDelta(0., notches),
                        phase: TouchPhase::Moved,
                    },
                ),
                Step::Snapshot(path) => {
                    // A relative path lands in TORE_SCRIPT_OUT when that is set.
                    let path = match std::env::var_os("TORE_SCRIPT_OUT") {
                        Some(dir) if path.is_relative() => PathBuf::from(dir).join(path),
                        _ => path,
                    };
                    let result = (|| -> std::io::Result<()> {
                        use std::io::Write;
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        let mut file = std::fs::File::create(&path)?;
                        write!(file, "P6\n{} {}\n255\n", menu::WIDTH, menu::HEIGHT)?;
                        let mut pixels = self.menu.pixels.clone();
                        self.leave_dialog_snapshot(&mut pixels);
                        for pixel in pixels.chunks_exact(4) {
                            file.write_all(&pixel[..3])?;
                        }
                        Ok(())
                    })();
                    match result {
                        Ok(()) => println!("Script snapshot: {}", path.display()),
                        Err(error) => {
                            self.error = Some(format!("{}: {error}", path.display()).into())
                        }
                    }
                }
                Step::Shot(path) => {
                    let path = match std::env::var_os("TORE_SCRIPT_OUT") {
                        Some(dir) if path.is_relative() => PathBuf::from(dir).join(path),
                        _ => path,
                    };
                    if let Some(renderer) = &mut self.renderer {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        // The 3D view with the cockpit, HUD and instruments as
                        // the last frame drew them; in the replay viewer (the
                        // observer screen too), its camera, world and scenery.
                        if std::env::var_os("TORE_SCENE_F12").is_some()
                            && let Some(replay) = &self.replay
                        {
                            let camera = replay.viewer.camera();
                            println!(
                                "replay F12 shot: {} eye {:?} yaw {} pitch {}",
                                path.display(),
                                camera.position,
                                camera.yaw,
                                camera.pitch
                            );
                        }
                        let result = match &self.replay {
                            Some(replay) => renderer.capture_sim(
                                &path,
                                replay.viewer.camera(),
                                &replay.viewer.world,
                                &replay.viewer.scenery,
                                true,
                            ),
                            None => renderer.capture_sim(
                                &path,
                                &self.camera,
                                &self.world.terrain,
                                &self.scenery,
                                true,
                            ),
                        };
                        if let Err(error) = result {
                            self.error = Some(format!("{}: {error}", path.display()).into());
                        }
                    }
                }
                Step::Exit => {
                    println!("Input script: exit");
                    // A test script's explicit terminator ends automation.
                    // The UI Exit action first presents a network debrief.
                    self.finished = true;
                    event_loop.exit();
                }
            }
        }
        self.script = Some(runner);
    }

    /// A key press or release, from the window or from `--input-script`.
    /// Returns what the game should do next.
    fn key_input(&mut self, event_loop: &ActiveEventLoop, event: input_script::KeyInput) -> Action {
        let mut name = match &event.logical {
            Key::Named(k) => format!("{k:?}"),
            Key::Character(c) => c.to_ascii_lowercase(),
            _ => String::new(),
        };
        if self.screen == Screen::Flight {
            name = flight_key(event.physical, &name);
        }
        if self.leave_dialog.is_some() {
            return if event.pressed {
                self.leave_dialog_key(&name)
            } else {
                Action::None
            };
        }
        if self.screen == Screen::Flight
            && event.pressed
            && self.modifiers.control_key()
            && !self.flight_ui.frozen()
            && net::scoreboard::page_key(
                &mut self.net_flight,
                &name,
                self.net
                    .as_ref()
                    .and_then(|s| s.client.scores())
                    .map_or(0, |s| s.players.len()),
            )
        {
            return Action::None;
        }
        // Alt-Enter switches window mode on every screen, before any
        // screen claims the key. F11 is not used: it already opens the
        // flight keyboard help (docs/FLIGHT-CONTROLS.md).
        if name == "Enter" && self.modifiers.alt_key() && event.pressed {
            if !event.repeat {
                self.toggle_fullscreen();
            }
            return Action::None;
        }
        if event.pressed
            && self.multiplayer_menu_active()
            && !((self.modifiers.alt_key() && name == "F4")
                || (self.modifiers.super_key() && name == "q"))
            && let Some(action) = self.multiplayer_menu.key(&name)
        {
            return action;
        }
        // The replay viewer takes the window's keys itself (`replay_event`);
        // a script's come here (the rest it leaves to this function, with
        // Alt or Command held, are the window's own: quitting).
        if self.screen == Screen::Replay && !self.modifiers.alt_key() && !self.modifiers.super_key()
        {
            self.replay_script_key(
                event_loop,
                &event.physical,
                &name,
                event.pressed,
                event.repeat,
            );
            return Action::None;
        }
        // Exit to desktop keeps its meaning over every screen, the
        // controls, sound and graphics screens included.
        if event.pressed
            && ((self.modifiers.super_key() && name.eq_ignore_ascii_case("q"))
                || (self.modifiers.alt_key() && name == "F4"))
            && (self.feedback.is_some()
                || self.controls.is_some()
                || self.sound_screen.is_some()
                || self.graphics_screen.is_some())
        {
            self.action(event_loop, Action::Exit);
            return Action::None;
        }
        if self.feedback.is_some() {
            if event.pressed {
                let form = self.feedback.as_mut().expect("checked");
                let result = if !self.modifiers.control_key()
                    && !self.modifiers.alt_key()
                    && event
                        .text
                        .as_ref()
                        .is_some_and(|t| t.chars().any(|c| !c.is_control()))
                {
                    form.text(event.text.as_ref().unwrap());
                    feedback::Outcome::None
                } else {
                    form.key(&name)
                };
                self.feedback_result(result);
            }
            return Action::None;
        }
        // The controls screen takes every key press while it is open,
        // with the same physical key names flight uses for capture.
        if event.pressed
            && let Some(editor) = &mut self.controls
        {
            if event.repeat && editor.capturing() {
                return Action::None;
            }
            // The search field takes printable text with its case;
            // Ctrl and Alt combinations stay shortcuts.
            if editor.typing()
                && !self.modifiers.control_key()
                && !self.modifiers.alt_key()
                && let Some(text) = &event.text
                && text.chars().any(|c| !c.is_control())
            {
                let result = editor.text_input(text);
                let action = self.controls_result(result);
                self.action(event_loop, action);
                return Action::None;
            }
            let name = flight_key(event.physical, &name);
            let result = editor.key(
                &name,
                self.modifiers.shift_key(),
                self.modifiers.control_key(),
                self.modifiers.alt_key(),
            );
            let action = self.controls_result(result);
            self.action(event_loop, action);
            return Action::None;
        }
        if event.pressed
            && let Some(screen) = &mut self.sound_screen
        {
            let outcome = screen.key(&name, self.modifiers.shift_key());
            let action = self.sound_result(outcome);
            self.action(event_loop, action);
            return Action::None;
        }
        if event.pressed
            && let Some(editor) = &mut self.graphics_screen
        {
            let result = editor.key(&name, self.modifiers.shift_key());
            let action = self.graphics_result(result);
            self.action(event_loop, action);
            return Action::None;
        }
        // The Direct Connection screen takes key presses and typed text,
        // except the shortcuts that quit the game.
        if event.pressed
            && self.direct_open()
            && !((self.modifiers.super_key() && name.eq_ignore_ascii_case("q"))
                || (self.modifiers.alt_key() && name == "F4"))
        {
            let action = self.direct_key(&name, event.text.as_deref());
            self.action(event_loop, action);
            return Action::None;
        }
        // The Internet Lobby screen takes them the same way.
        if event.pressed
            && self.internet_open()
            && !((self.modifiers.super_key() && name.eq_ignore_ascii_case("q"))
                || (self.modifiers.alt_key() && name == "F4"))
        {
            let action = self.internet_key(&name, event.text.as_deref());
            self.action(event_loop, action);
            return Action::None;
        }
        // The Replays screen takes key presses too, except the
        // shortcuts that quit the game.
        if event.pressed
            && self.screen == Screen::Main
            && !((self.modifiers.super_key() && name.eq_ignore_ascii_case("q"))
                || (self.modifiers.alt_key() && name == "F4"))
            && let Some(screen) = &mut self.replays_screen
        {
            let result = screen.key(&name, self.modifiers.shift_key(), event.repeat);
            let action = self.replays_result(result);
            self.action(event_loop, action);
            return Action::None;
        }
        // A held menu key must not accept a newly opened page, close it,
        // or carry a Done press into the separate respawn confirmation.
        if self.screen == Screen::Flight
            && self.waiting_to_respawn()
            && event.repeat
            && matches!(name.as_str(), "Enter" | "Space" | "Escape")
        {
            return Action::None;
        }
        if self.screen == Screen::Flight && self.respawn_open() {
            if event.pressed {
                // Text belongs to the focused search field, including Space and M.
                let searching = !self.respawn_brief_open()
                    && self
                        .net_flight
                        .as_ref()
                        .and_then(|f| f.respawn_menu.as_ref())
                        .is_some_and(|m| m.searching());
                let action = self.respawn_key(&name);
                if searching
                    && let Some(text) = &event.text
                    && let Some(menu) = self
                        .net_flight
                        .as_mut()
                        .and_then(|f| f.respawn_menu.as_mut())
                {
                    menu.text(text);
                }
                self.action(event_loop, action);
            }
            return Action::None;
        }
        // The chat line of a networked flight (net/chat.rs) takes the keyboard
        // while it is open.
        if self.chat_key(&event, &name) {
            return Action::None;
        }
        if self.screen == Screen::Flight
            && event.pressed
            && self.flight_ui.debug_panels
            && !self.flight_ui.menu
            && let Some(view) = self.live_debug.key(&name, self.modifiers.shift_key())
        {
            if let Some(view) = view {
                let action = self.live_view(view);
                self.action(event_loop, action);
            }
            return Action::None;
        }
        if self.screen == Screen::Flight
            && event.pressed
            && !event.repeat
            && self.flight_ui.lighting_key(
                &name,
                self.modifiers.alt_key() || self.modifiers.super_key(),
            )
        {
            return Action::None;
        }
        if self.screen == Screen::Flight
            && !(event.pressed
                && self.flight_ui.map.open
                && !self.modifiers.control_key()
                && !self.modifiers.alt_key()
                && matches!(
                    name.as_str(),
                    "m" | "Escape"
                        | "+"
                        | "="
                        | "-"
                        | "_"
                        | "ArrowLeft"
                        | "ArrowRight"
                        | "ArrowUp"
                        | "ArrowDown"
                        | "Home"
                        | "Enter"
                        | "PageUp"
                        | "PageDown"
                        | "End"
                ))
            && !(event.pressed
                && self.flight_ui.menu
                && matches!(
                    name.as_str(),
                    "Escape"
                        | "Tab"
                        | "ArrowUp"
                        | "ArrowDown"
                        | "ArrowLeft"
                        | "ArrowRight"
                        | "Enter"
                        | "Space"
                ))
            && (if event.repeat {
                self.input.claimed(&name)
            } else {
                self.input.key(&name, event.pressed, self.modifiers)
            })
        {
            return Action::None;
        }
        if self.screen == Screen::Flight && name == "Space" {
            let blocked = self.flight_ui.frozen() || !self.focused || !self.modifiers.is_empty();
            self.queue(seats::SeatCommand::TriggerKey {
                down: event.pressed,
                repeat: event.repeat,
                blocked,
            });
            if !self.flight_ui.menu {
                return Action::None;
            }
        }
        if matches!(self.screen, Screen::Viewer | Screen::Flight) && !event.pressed {
            self.camera.keys.remove(&name);
            self.camera.keys.remove(&format!("Look{name}"));
            return Action::None;
        }
        if !event.pressed {
            return Action::None;
        }
        if (self.modifiers.super_key() && name.eq_ignore_ascii_case("q"))
            || (self.modifiers.alt_key() && name == "F4")
        {
            Action::Exit
        } else if self.screen == Screen::Flight {
            let map_before = self.flight_ui.map.open;
            let before = self.flight_ui.frozen();
            let command = if event.repeat || self.modifiers.super_key() {
                flight_ui::Command::None
            } else if let Some(steps) = console_keys::zoom_key(&name).filter(|_| {
                // In the sensor console the view zoom keys zoom the sight.
                self.modifiers.is_empty()
                    && !self.flight_ui.menu
                    && !self.flight_ui.map.open
                    && self.console_open()
            }) {
                flight_ui::Command::SightZoom(steps)
            } else {
                self.flight_ui.key(
                    &name,
                    self.modifiers.shift_key(),
                    self.modifiers.control_key(),
                    self.modifiers.alt_key(),
                    net::play::menu(&self.hornet, &self.net_flight),
                )
            };
            if self.flight_ui.map.open != map_before {
                if self.flight_ui.map.open {
                    self.flight_ui.map.brief = None;
                }
                self.flight_ui.map.cancel_press();
                self.camera.keys.clear();
                self.release_trigger();
                self.instruments.cancel_press();
            }
            if self.flight_ui.frozen() || before != self.flight_ui.frozen() {
                self.camera.keys.clear();
                self.release_trigger();
                self.instruments.cancel_press();
                self.flight_clock.remainder = 0.;
                let own = &mut self.world.cockpits[OWN];
                own.previous_flight.clone_from(&own.flight);
                self.frame_time = Instant::now();
            } else if !self.flight_ui.map.open {
                look::press(&mut self.camera.keys, &name, self.modifiers);
            }
            self.flight_command(command)
        } else if self.screen == Screen::Viewer {
            if name == "Escape" {
                Action::Back
            } else {
                self.camera.keys.insert(name);
                Action::None
            }
        } else if event.repeat {
            Action::None
        } else if self.screen == Screen::Quick {
            self.quick.key(
                if name == "Space" { " " } else { &name },
                self.modifiers.shift_key(),
            )
        } else {
            self.menu.state.key(
                if name == "Space" { " " } else { &name },
                self.modifiers.shift_key(),
            )
        }
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let result = (|| {
            diagnostics::stage("game window creation");
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title(match self.screen {
                            Screen::Flight => format!(
                                "T.O.R.E-Fighters - {} Free Flight",
                                self.hornet.profile.id.label()
                            ),
                            Screen::Main => "T.O.R.E-Fighters - Choose Activity".to_string(),
                            Screen::Quick => "T.O.R.E-Fighters - Quick Mission Creator".to_string(),
                            Screen::Viewer => format!(
                                "T.O.R.E-Fighters - {} Terrain Viewer",
                                self.world.terrain.theater.name
                            ),
                            Screen::Replay => self
                                .replay
                                .as_ref()
                                .map_or_else(String::new, |r| r.viewer.title()),
                        })
                        // Fixed-size diagnostic windows preserve requested capture aspect ratios
                        // on compositors that otherwise tile/rescale newly created windows.
                        .with_resizable(!self.smoke_test)
                        // The inner size is also the size Alt-Enter returns to.
                        .with_inner_size(LogicalSize::new(self.window_size[0], self.window_size[1]))
                        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
                        .with_fullscreen(fullscreen_attribute(self.fullscreen)),
                )?,
            );
            diagnostics::stage_done();
            pollster::block_on(Renderer::new(window, &self.scenery, self.graphics))
        })();
        match result {
            Ok(mut renderer) => {
                diagnostics::stage("aircraft graphics preparation");
                renderer.prepare_aircraft(&self.hornet);
                diagnostics::stage_done();
                renderer.window.request_redraw();
                self.renderer = Some(renderer);
                let automated = self.pace_window().automated;
                if let Some(renderer) = &mut self.renderer {
                    renderer.automate(automated);
                }
                if let Some(session) = self.connect.take() {
                    match session {
                        net::options::Session::Join(options) => self.start_session(options),
                        net::options::Session::Host(mut options) => {
                            // A game listed from the command line reports
                            // under the Internet Lobby's statistics switch.
                            if let Ok(data) = assets::data_directory() {
                                net::telemetry::for_command_line(&data, &mut options.listing);
                            }
                            self.start_hosting(*options)
                        }
                    }
                    if self.error.is_some() {
                        event_loop.exit();
                        return;
                    }
                }
                if std::mem::take(&mut self.launch_creator) {
                    self.creator_loading_view = Some((
                        self.flight_view,
                        self.view_rig.reference,
                        self.flight_ui.look,
                        self.flight_ui.zoom,
                    ));
                    self.action(event_loop, Action::Mission);
                } else if self.screen == Screen::Flight {
                    // A flight started straight from the command line records
                    // itself like one started from the menu; captures, smoke
                    // tests and timing runs have no recording library.
                    self.start_replay_recording();
                }
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.finished {
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested)
            && self.screen == Screen::Flight
            && (self.flight_ui.map.open || self.respawn_open())
            && self.flight_ui.map.brief.is_none()
        {
            self.prepare_mission_brief();
        }
        if self.leave_dialog_event(event_loop, &event) {
            return;
        }
        // Loading screens have their own event loop branch, so route a
        // connected window close before that branch can quit directly.
        if self.net.is_some() && matches!(event, WindowEvent::CloseRequested) {
            self.action(event_loop, Action::Exit);
            return;
        }
        self.note_menu_input(&event);
        // With the frame-rate counter under a capped rate, or an unfocused
        // window, the timer draws the frames: a request from an event waits
        // for the next slot.
        if matches!(event, WindowEvent::RedrawRequested) && self.redraw_too_soon() {
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested)
            && let Some(renderer) = &mut self.renderer
        {
            if frame_log::enabled() {
                let screen = if self.loading.is_some() {
                    "loading"
                } else {
                    match self.screen {
                        Screen::Flight => "flight",
                        Screen::Replay => "replay",
                        Screen::Viewer => "viewer",
                        _ => "menu",
                    }
                };
                let theater = self
                    .world
                    .0
                    .as_ref()
                    .map_or("loading", |w| w.terrain.theater.name.as_str());
                let role = self.net.as_ref().map_or("single", |net| {
                    if net.hosting() { "host" } else { "client" }
                });
                frame_log::begin(
                    screen,
                    self.flight_view,
                    theater,
                    role,
                    self.focused,
                    self.flight_ui.time_scale,
                );
            }
            renderer.begin_cpu_frame();
        }
        if self.loading.is_some() {
            if self.net.is_some()
                && self.world.0.is_some()
                && matches!(&event, WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && event.logical_key == Key::Named(winit::keyboard::NamedKey::Escape))
            {
                // The live world is still on this thread. A theater change or
                // restart has moved it into the worker; dropping that job
                // would discard it. Those loads run to completion.
                self.dismiss_loading();
                self.net_ending = Some("Join cancelled".into());
                self.end_session(event_loop);
                return;
            }
            // The session is pumped from the event loop's wait (about_to_wait).
            if let Some(renderer) = &mut self.renderer {
                if renderer.window.id() != id {
                    return;
                }
                match event {
                    WindowEvent::CloseRequested => {
                        self.finished = true;
                        event_loop.exit();
                    }
                    WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                        renderer.resize()
                    }
                    WindowEvent::Focused(focused) => {
                        self.focused = focused;
                        self.input.context(true, focused);
                    }
                    WindowEvent::RedrawRequested
                        if self
                            .loading
                            .as_ref()
                            .is_some_and(|job| job.screen_kept(Instant::now())) =>
                    {
                        // The page the load started from stays on screen;
                        // the photograph comes only if the load runs long.
                    }
                    WindowEvent::RedrawRequested => {
                        renderer.set_veil([0., 0.]);
                        let window = renderer.window.inner_size();
                        let lobby = self.net.as_ref().and_then(|session| session.client.lobby());
                        let name = lobby.map_or_else(
                            || self.net.as_ref().map_or("", |session| session.game_label()),
                            |lobby| lobby.name.as_str(),
                        );
                        let players = lobby
                            .map(|lobby| {
                                lobby
                                    .players
                                    .iter()
                                    .map(|p| p.callsign.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        let size = self.loading.as_ref().unwrap().draw(
                            &mut self.loading_pixels,
                            [window.width, window.height],
                            [name, &players],
                        );
                        if let Err(error) = renderer.draw(
                            &self.loading_pixels,
                            None,
                            Some(size),
                            None,
                            frame_rate::Surface::Loading,
                        ) {
                            self.error = Some(error);
                            event_loop.exit();
                        }
                        self.loading_next_frame();
                    }
                    _ => {}
                }
            }
            return;
        }
        // The replay viewer takes its own input and drawing; what it leaves
        // (resizing, focus, Alt-Enter, quitting) carries on below.
        let event = if self.screen == Screen::Replay
            && self.renderer.as_ref().is_some_and(|r| r.window.id() == id)
        {
            match self.replay_event(event_loop, event) {
                Some(event) => event,
                None => return,
            }
        } else {
            event
        };
        // A joined server takes its turn before a frame is drawn: the controls
        // go in, the frame to draw comes out.
        if matches!(event, WindowEvent::RedrawRequested)
            && self.renderer.as_ref().is_some_and(|r| r.window.id() == id)
        {
            if !self.net_pumped_in_wait() {
                self.net_tick(event_loop);
            }
            self.direct_tick(event_loop);
            self.internet_tick(event_loop);
        }
        if self.loading.is_none()
            && (self.pending_host.is_some()
                || self.pending_join.is_some()
                || self.net.as_ref().is_some_and(|session| {
                    session.client.is_building()
                        || matches!(
                            session.client.phase(),
                            tore_session::ClientPhase::Connecting
                                | tore_session::ClientPhase::Loading
                        )
                }))
        {
            if matches!(&event, WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && event.logical_key == Key::Named(winit::keyboard::NamedKey::Escape))
            {
                self.net_ending = Some("Join cancelled".into());
                self.end_session(event_loop);
                return;
            }
            if matches!(event, WindowEvent::RedrawRequested) {
                let lobby = self.net.as_ref().and_then(|session| session.client.lobby());
                let name = lobby.map_or_else(
                    || {
                        self.pending_join
                            .as_ref()
                            .map(|(_, label)| label.as_str())
                            .or_else(|| {
                                self.pending_host
                                    .as_ref()
                                    .map(|(options, _)| options.name.as_str())
                            })
                            .or_else(|| self.net.as_ref().map(|session| session.game_label()))
                            .unwrap_or("")
                    },
                    |lobby| lobby.name.as_str(),
                );
                let players = lobby
                    .map(|lobby| {
                        lobby
                            .players
                            .iter()
                            .map(|p| p.callsign.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_veil([0., 0.]);
                    let window = renderer.window.inner_size();
                    let stage = if self.pending_join.is_some() || self.pending_host.is_some() {
                        loading::Stage::Content
                    } else if self.net.as_ref().is_some_and(|s| s.client.is_building()) {
                        loading::Stage::Mission
                    } else if self
                        .net
                        .as_ref()
                        .is_some_and(|s| s.client.phase() == tore_session::ClientPhase::Loading)
                    {
                        loading::Stage::Waiting
                    } else {
                        loading::Stage::Connecting
                    };
                    let progress = self
                        .network_loading_stage
                        .get_or_insert_with(loading::Progress::network);
                    progress.stage(stage);
                    let size = progress.draw(
                        &mut self.loading_pixels,
                        [window.width, window.height],
                        [name, &players],
                    );
                    if let Err(error) = renderer.draw(
                        &self.loading_pixels,
                        None,
                        Some(size),
                        None,
                        frame_rate::Surface::Loading,
                    ) {
                        self.error = Some(error);
                        event_loop.exit();
                    }
                    // The next frame is asked for by about_to_wait, at 20 Hz,
                    // or at the frame rate while the counter is shown.
                    self.loading_next_frame();
                }
                return;
            }
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if renderer.window.id() != id {
            return;
        }
        // A stopped flight waiting on a still picture draws again when
        // anything happens, as soon as the frame rate allows.
        if self.screen == Screen::Flight
            && self.next_frame.is_none()
            && !matches!(
                event,
                WindowEvent::RedrawRequested | WindowEvent::Occluded(true)
            )
        {
            let limit = renderer.frame_limit();
            let window = self.pace_window();
            self.next_frame = self.pacer.soonest(limit, window, Instant::now());
        }
        if self.multiplayer_menu_active()
            && let WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } = &event
        {
            let point = self
                .renderer
                .as_ref()
                .and_then(|r| self.pointer.and_then(|(x, y)| r.viewport().point(x, y)))
                .map(|(x, y)| (x as i32, y as i32));
            if let Some(action) = self
                .multiplayer_menu
                .pointer(point, *state == ElementState::Pressed)
            {
                self.action(event_loop, action);
                if let Some(renderer) = &self.renderer {
                    renderer.window.request_redraw();
                }
                return;
            }
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let action = match event {
            WindowEvent::CloseRequested => Action::Exit,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                renderer.resize();
                // Minimised to nothing (Windows): a single-player flight
                // pauses, as it does when the window loses focus.
                let size = renderer.window.inner_size();
                if (size.width == 0 || size.height == 0)
                    && self.screen == Screen::Flight
                    && self.script.is_none()
                {
                    self.flight_ui.pause_for_focus();
                }
                self.menu.state.cancel();
                self.quick.cancel();
                self.instruments.cancel_press();
                self.flight_ui.cancel_press();
                if let Some(editor) = &mut self.controls {
                    editor.cancel_capture();
                }
                if let Some(editor) = &mut self.graphics_screen {
                    editor.cancel_press();
                }
                if let Some(screen) = &mut self.sound_screen {
                    screen.cancel_press();
                }
                if let Some(screen) = &mut self.replays_screen {
                    screen.cancel_press();
                }
                self.direct_cancel_press();
                self.internet_cancel_press();
                self.mouse_look = None;
                self.pointer = None;
                if let Some(menu) = self
                    .net_flight
                    .as_mut()
                    .and_then(|f| f.respawn_menu.as_mut())
                {
                    menu.moved(None);
                }
                self.live_debug.release();
                self.camera.keys.clear();
                self.release_trigger();
                self.modifiers = ModifiersState::empty();
                Action::None
            }
            WindowEvent::CursorMoved { position, .. } => {
                let point = renderer.viewport().point(position.x, position.y);
                self.pointer = Some((position.x, position.y));
                if let Some(screen) = &mut self.sound_screen {
                    let outcome = screen.moved(point);
                    let action = self.sound_result(outcome);
                    if outcome != sound_screen::Outcome::None {
                        self.action(event_loop, action);
                    }
                    return;
                }
                if self.screen == Screen::Main && self.direct.screen.is_some() {
                    direct_screen::app::pointer_moved(&mut self.lobby, &mut self.direct, point);
                    return;
                }
                if self.screen == Screen::Main && self.internet.screen.is_some() {
                    internet_screen::app::pointer_moved(&mut self.lobby, &mut self.internet, point);
                    return;
                }
                if self
                    .net_flight
                    .as_ref()
                    .is_some_and(|f| f.respawn_menu.is_some())
                {
                    if self
                        .net_flight
                        .as_ref()
                        .and_then(|f| f.respawn_menu.as_ref())
                        .is_some_and(|m| m.brief_open)
                        && let Some(brief) = &mut self.flight_ui.map.brief
                    {
                        brief.moved(point);
                    } else if let Some(menu) = self
                        .net_flight
                        .as_mut()
                        .and_then(|f| f.respawn_menu.as_mut())
                    {
                        menu.moved(point);
                    }
                    renderer.window.request_redraw();
                    return;
                }
                if let Some(editor) = &mut self.graphics_screen {
                    // A held scroll bar thumb follows the pointer.
                    if editor.moved(point) {
                        renderer.window.request_redraw();
                    }
                    return;
                }
                if let Some(editor) = &mut self.controls {
                    // A control's tooltip comes and goes with the pointer.
                    if editor.moved(point) {
                        renderer.window.request_redraw();
                    }
                    return;
                }
                if self.screen == Screen::Main && self.replays_screen.is_some() {
                    return;
                }
                match self.screen {
                    Screen::Main => self.menu.state.pointer(point),
                    Screen::Quick => {
                        self.quick.pointer(point);
                        Action::None
                    }
                    Screen::Flight => {
                        if self.flight_ui.map.open {
                            // A held scroll bar thumb follows the pointer.
                            self.flight_ui.map.moved(point);
                        }
                        if self.flight_ui.frozen() {
                            self.mouse_look = None;
                        }
                        if let Some(last) = self.mouse_look
                            && self.input.console.open
                        {
                            // The sensor console: mouse look slews the sight.
                            let profile = &self.input.resolver.profile;
                            let up = if profile.mouse_invert { 1. } else { -1. };
                            let sensitivity = profile.mouse_sensitivity;
                            self.input.console.mouse_moved(
                                [position.x - last.0, (position.y - last.1) * up],
                                sensitivity,
                            );
                            self.mouse_look = Some((position.x, position.y));
                        } else if let Some(last) = self.mouse_look {
                            let profile = &self.input.resolver.profile;
                            // 1.0 sensitivity turns 2 radians per 1,000 pixels.
                            let k = 0.002 * profile.mouse_sensitivity;
                            let up = if profile.mouse_invert { 1. } else { -1. };
                            look::nudge(
                                &mut self.flight_ui.look,
                                [
                                    ((position.x - last.0) * k) as f32,
                                    ((position.y - last.1) * k * up) as f32,
                                ],
                                matches!(self.flight_view, 1 | 2),
                            );
                            self.mouse_look = Some((position.x, position.y));
                            renderer.window.request_redraw();
                        }
                        if self.flight_ui.debug_panels {
                            let window = [position.x, position.y];
                            let (point, size) = replay::host::view_point(renderer, window);
                            self.live_debug.pointer(Some(point), window, size);
                        }
                        Action::None
                    }
                    Screen::Viewer | Screen::Replay => Action::None,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 40.,
                };
                let notches = self.wheel.trunc() as i32;
                self.wheel -= f64::from(notches);
                if notches == 0 {
                    return;
                }
                if self.screen == Screen::Flight && self.respawn_open() {
                    if self.respawn_brief_open() {
                        self.respawn_key(if notches > 0 { "ArrowUp" } else { "ArrowDown" })
                    } else {
                        if let Some(menu) = self
                            .net_flight
                            .as_mut()
                            .and_then(|f| f.respawn_menu.as_mut())
                        {
                            menu.wheel(notches);
                        }
                        Action::None
                    }
                } else if self.screen == Screen::Flight
                    && self.flight_ui.map.open
                    && self.flight_ui.map.mission_tab
                {
                    self.flight_ui
                        .map
                        .key(if notches > 0 { "ArrowUp" } else { "ArrowDown" });
                    Action::None
                } else if let Some(screen) = &mut self.sound_screen {
                    let outcome = screen.wheel(notches);
                    self.sound_result(outcome)
                } else if let Some(editor) = &mut self.controls {
                    let result = editor.wheel(notches);
                    self.controls_result(result)
                } else if let Some(editor) = &mut self.graphics_screen {
                    let result = editor.wheel(notches);
                    self.graphics_result(result)
                } else if self.screen == Screen::Flight
                    && self.flight_ui.debug_panels
                    && !self.flight_ui.menu
                    && self
                        .live_debug
                        .wheel(notches, self.world.combat.state.tick())
                {
                    Action::None
                } else if self.screen == Screen::Main
                    && let Some(screen) = &mut self.replays_screen
                {
                    let result = screen.wheel(notches);
                    self.replays_result(result)
                } else if self.direct_open() {
                    self.direct_wheel(notches);
                    Action::None
                } else if self.internet_open() {
                    self.internet_wheel(notches);
                    Action::None
                } else if self.screen == Screen::Flight && !self.flight_ui.frozen() {
                    self.input.mouse_wheel(notches);
                    Action::None
                } else {
                    return;
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                if let Some(editor) = &mut self.controls {
                    editor.moved(None);
                }
                if let Some(menu) = self
                    .net_flight
                    .as_mut()
                    .and_then(|f| f.respawn_menu.as_mut())
                {
                    menu.moved(None);
                }
                self.live_debug.release();
                self.quick.pointer(None);
                direct_screen::app::pointer_moved(&mut self.lobby, &mut self.direct, None);
                internet_screen::app::pointer_moved(&mut self.lobby, &mut self.internet, None);
                self.menu.state.pointer(None)
            }
            WindowEvent::Focused(true) => {
                self.focused = true;
                // Back to full rate at once, not after the unfocused wait.
                renderer.window.request_redraw();
                Action::None
            }
            // Hidden (minimised or covered, where the platform says so):
            // nothing is drawn until it shows again, and a single-player
            // flight pauses as it does on focus loss. A session's flight
            // keeps its network turns (`about_to_wait`).
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    renderer.window.request_redraw();
                } else if self.screen == Screen::Flight && self.script.is_none() {
                    self.flight_ui.pause_for_focus();
                }
                Action::None
            }
            WindowEvent::Focused(false) if self.script.is_none() => {
                self.focused = false;
                self.input.context(true, false);
                if self.screen == Screen::Flight {
                    self.flight_ui.pause_for_focus();
                }
                self.menu.state.cancel();
                self.quick.cancel();
                self.instruments.cancel_press();
                self.flight_ui.cancel_press();
                if let Some(editor) = &mut self.controls {
                    editor.cancel_capture();
                }
                if let Some(editor) = &mut self.graphics_screen {
                    editor.cancel_press();
                }
                if let Some(screen) = &mut self.sound_screen {
                    screen.cancel_press();
                }
                if let Some(screen) = &mut self.replays_screen {
                    screen.cancel_press();
                }
                self.direct_cancel_press();
                self.internet_cancel_press();
                self.mouse_look = None;
                self.pointer = None;
                if let Some(menu) = self
                    .net_flight
                    .as_mut()
                    .and_then(|f| f.respawn_menu.as_mut())
                {
                    menu.moved(None);
                }
                self.live_debug.release();
                self.camera.keys.clear();
                self.release_trigger();
                self.modifiers = ModifiersState::empty();
                Action::None
            }
            WindowEvent::MouseInput { state, button, .. } if self.sound_screen.is_some() => {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                let screen = self.sound_screen.as_mut().expect("guarded");
                let outcome = if button == MouseButton::Left {
                    screen.button(point, state == ElementState::Pressed)
                } else {
                    sound_screen::Outcome::None
                };
                self.sound_result(outcome)
            }
            WindowEvent::MouseInput { state, button, .. } if self.feedback.is_some() => {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                if button == MouseButton::Left {
                    let result = self
                        .feedback
                        .as_mut()
                        .unwrap()
                        .pointer(point, state == ElementState::Pressed);
                    self.feedback_result(result);
                }
                Action::None
            }
            WindowEvent::MouseInput { state, button, .. } if self.controls.is_some() => {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                let pressed = state == ElementState::Pressed;
                let editor = self.controls.as_mut().expect("guarded");
                let result = match (button, mouse_control(button)) {
                    (MouseButton::Left, _) => editor.pointer(point, pressed),
                    // Capturing a mouse input takes the right button; any
                    // other time it turns a cycling value back.
                    (MouseButton::Right, _) if !editor.capturing() => {
                        editor.right_pointer(point, pressed)
                    }
                    (_, Some(control)) if pressed => editor.mouse(control),
                    _ => controls_editor::ResultAction::None,
                };
                self.controls_result(result)
            }
            WindowEvent::MouseInput { state, button, .. } if self.graphics_screen.is_some() => {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                let editor = self.graphics_screen.as_mut().expect("guarded");
                let result = if button == MouseButton::Left {
                    editor.pointer(point, state == ElementState::Pressed)
                } else {
                    controls_editor::ResultAction::None
                };
                self.graphics_result(result)
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.screen == Screen::Main && self.replays_screen.is_some() =>
            {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                let screen = self.replays_screen.as_mut().expect("guarded");
                let result = if button == MouseButton::Left {
                    screen.pointer(point, state == ElementState::Pressed)
                } else {
                    replay::screen::Outcome::None
                };
                self.replays_result(result)
            }
            // The lobby's right button: the King's slot locks and the Settings
            // panel's turn back (slice F2-L).
            WindowEvent::MouseInput { state, button, .. }
                if self.screen == Screen::Flight
                    && self
                        .net_flight
                        .as_ref()
                        .is_some_and(|f| f.respawn_menu.is_some()) =>
            {
                let point = self
                    .pointer
                    .and_then(|(x, y)| renderer.viewport().point(x, y));
                match button {
                    MouseButton::Left => {
                        self.respawn_pointer(point, state == ElementState::Pressed)
                    }
                    MouseButton::Right => {
                        self.respawn_right_pointer(point, state == ElementState::Pressed)
                    }
                    _ => Action::None,
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } if self.screen == Screen::Main && self.lobby.screen.is_some() => {
                self.lobby_right_button(state == ElementState::Pressed)
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.screen == Screen::Main && self.direct.screen.is_some() =>
            {
                if button == MouseButton::Left {
                    self.direct_button(state == ElementState::Pressed)
                } else {
                    Action::None
                }
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.screen == Screen::Main && self.internet.screen.is_some() =>
            {
                if button == MouseButton::Left {
                    self.internet_button(state == ElementState::Pressed)
                } else {
                    Action::None
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } if self.screen == Screen::Quick => self.quick.right(state == ElementState::Pressed),
            // The Escape menu's right button turns a row's value back.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } if self.screen == Screen::Flight && self.flight_ui.menu => {
                let command = self.flight_ui.right_pointer(
                    net::play::menu(&self.hornet, &self.net_flight),
                    self.pointer
                        .and_then(|(x, y)| renderer.viewport().point(x, y)),
                    state == ElementState::Pressed,
                );
                self.flight_command(command)
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.screen == Screen::Flight && button != MouseButton::Left =>
            {
                let pressed = state == ElementState::Pressed;
                if button == MouseButton::Right && self.input.resolver.profile.mouse_look {
                    self.mouse_look = self.pointer.filter(|_| pressed && !self.flight_ui.frozen());
                } else if let Some(control) = mouse_control(button) {
                    self.input.mouse_button(control, pressed);
                }
                // With the debug panels on, a right-click (no drag) opens
                // their menu. A right button bound to something else, with
                // mouse look off, keeps its binding alone.
                if button == MouseButton::Right
                    && self.flight_ui.debug_panels
                    && !self.flight_ui.menu
                    && let Some((x, y)) = self.pointer
                {
                    let profile = &self.input.resolver.profile;
                    let allowed = profile.mouse_look
                        || !profile
                            .bindings
                            .iter()
                            .any(|b| b.device == "mouse" && b.control == "button:right");
                    let (point, size) = replay::host::view_point(renderer, [x, y]);
                    let slop = replay::context_menu::CLICK_SLOP * renderer.window.scale_factor();
                    if let Some(at) =
                        self.live_debug
                            .right(pressed, [x, y], Some(point), slop, allowed)
                    {
                        self.live_debug
                            .open_menu(at, size, &self.camera, &self.hornet.font);
                    }
                }
                Action::None
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let live = if self.screen == Screen::Flight
                    && self.flight_ui.debug_panels
                    && !self.flight_ui.menu
                    && !self.flight_ui.map.open
                {
                    let (point, size) = self
                        .pointer
                        .map(|(x, y)| replay::host::view_point(renderer, [x, y]))
                        .map_or((None, renderer.flight_size()), |(p, s)| (Some(p), s));
                    self.live_debug.left(
                        state == ElementState::Pressed,
                        point,
                        size,
                        self.world.combat.state.tick(),
                    )
                } else {
                    None
                };
                if self.screen == Screen::Flight
                    && self
                        .net_flight
                        .as_ref()
                        .is_some_and(|f| f.respawn_menu.is_some())
                {
                    let point = self
                        .pointer
                        .and_then(|(x, y)| renderer.viewport().point(x, y));
                    self.respawn_pointer(point, state == ElementState::Pressed)
                } else if let Some(view) = live {
                    view.map_or(Action::None, |view| self.live_view(view))
                } else if self.screen == Screen::Flight
                    && self.flight_ui.map.open
                    && !self.flight_ui.menu
                {
                    self.flight_ui.map.pointer(
                        self.pointer
                            .and_then(|(x, y)| renderer.viewport().point(x, y)),
                        state == ElementState::Pressed,
                    );
                    Action::None
                } else if self.screen == Screen::Flight && self.flight_ui.menu {
                    let command = self.flight_ui.pointer(
                        net::play::menu(&self.hornet, &self.net_flight),
                        self.pointer
                            .and_then(|(x, y)| renderer.viewport().point(x, y)),
                        state == ElementState::Pressed,
                    );
                    self.camera.keys.clear();
                    self.release_trigger();
                    self.frame_time = Instant::now();
                    self.flight_command(command)
                } else if self.screen == Screen::Flight
                    && (self.view_rig.cockpit(self.flight_view)
                        && self.flight_ui.weapon_diagnostics_shown(self.flight_view))
                    && !self.flight_ui.frozen()
                    && self.pointer.is_some_and(|p| {
                        let size = [
                            f64::from(renderer.window.inner_size().width),
                            f64::from(renderer.window.inner_size().height),
                        ];
                        weapon_hud::mode_hit(p, size) || weapon_hud::release_hit(p, size)
                    })
                {
                    if state == ElementState::Pressed {
                        queue_command(
                            &mut self.seat_commands,
                            seats::SeatCommand::Combat(
                                if self.pointer.is_some_and(|p| {
                                    weapon_hud::release_hit(
                                        p,
                                        [
                                            f64::from(renderer.window.inner_size().width),
                                            f64::from(renderer.window.inner_size().height),
                                        ],
                                    )
                                }) {
                                    tore_sim::combat::live::Command::ClearDesignation
                                } else {
                                    tore_sim::combat::live::Command::ToggleSeekerMode
                                },
                            ),
                        );
                    }
                    Action::Click
                } else if self.screen == Screen::Flight {
                    let hit = self.instruments.screen_pointer(
                        self.pointer,
                        [
                            renderer.window.inner_size().width as f64,
                            renderer.window.inner_size().height as f64,
                        ],
                        state == ElementState::Pressed,
                    );
                    self.queue_scope_commands();
                    if hit { Action::Click } else { Action::None }
                } else if self.screen == Screen::Viewer {
                    Action::None
                } else if self.screen == Screen::Quick {
                    if state == ElementState::Pressed {
                        self.quick.shift = self.modifiers.shift_key();
                        self.quick.down()
                    } else {
                        self.quick.up()
                    }
                } else if state == ElementState::Pressed {
                    self.menu.state.down();
                    Action::None
                } else {
                    self.menu.state.up()
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                if !modifiers.state().is_empty() {
                    self.release_trigger();
                }
                if self.screen == Screen::Flight {
                    look::modifiers_changed(&mut self.camera.keys, modifiers.state());
                }
                self.modifiers = modifiers.state();
                Action::None
            }
            WindowEvent::KeyboardInput { event, .. } => self.key_input(
                event_loop,
                input_script::KeyInput {
                    logical: event.logical_key.clone(),
                    physical: event.physical_key,
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                    text: event.text.clone(),
                },
            ),
            WindowEvent::RedrawRequested => {
                let frame_start = Instant::now();
                let mut simulation_ms = 0.;

                if self.screen == Screen::Flight
                    && let Some(view) = self.performance.view()
                {
                    self.flight_view = view;
                }
                if self.screen == Screen::Flight && self.performance.active() {
                    // Explicit bounded benchmark only: desktop automation may steal focus.
                    self.flight_ui.paused = false;
                }
                // The text the multiplayer screens drew this frame, for the
                // renderer to draw sharp over the canvas.
                let mut menu_text: Option<ui_text::Layer> = None;
                // A screen over a flight that animated this frame.
                let mut overlay_moving = false;
                let mut animating = match self.screen {
                    Screen::Main => {
                        let _frame_log = frame_log::span(frame_log::Phase::Ui);
                        // The Replays screen covers the menu, so a menu
                        // notice shows on its status line instead.
                        if let Some(screen) = &mut self.replays_screen {
                            let notice = self.menu.state.toast.take().map(|(text, _)| text);
                            // Back from the replay viewer: list again.
                            if screen.shown(notice) {
                                screen.refresh();
                            }
                        }
                        // Direct Connection covers the whole menu; it draws its
                        // own background, so the menu is not drawn under it.
                        ui_text::begin();
                        let drawn = direct_screen::app::draw_screen(
                            &self.lobby,
                            &self.direct,
                            &mut menu::Canvas(&mut self.menu.pixels),
                        ) || internet_screen::app::draw_screen(
                            &self.internet,
                            &mut menu::Canvas(&mut self.menu.pixels),
                        );
                        menu_text = ui_text::finish().filter(|_| drawn);
                        // The lobby, Direct and Internet screens are drawn on
                        // the menu pace (net::pace), not every frame.
                        let mut animating = if drawn {
                            false
                        } else {
                            self.menu.render()
                                || self.direct.is_building()
                                || self.internet.is_building()
                        };
                        if let Some(form) = &self.feedback {
                            form.draw(&mut self.menu.pixels, &self.hornet.font);
                        }
                        if let Some(editor) = &self.controls {
                            editor.draw(&mut self.menu.pixels, &self.hornet.font);
                            // Redraw until a tooltip's delay is over.
                            animating |= editor.tooltip_pending();
                        }
                        if let Some(editor) = &self.graphics_screen {
                            editor.draw(&mut self.menu.pixels, &self.hornet.font);
                        }
                        if let Some(screen) = &mut self.sound_screen {
                            animating |= screen.animate();
                            if screen.take_switch_sound()
                                && let Some(audio) = &self.audio
                            {
                                audio.action(Action::Toggle);
                            }
                            screen.draw(&mut self.menu.pixels, &self.menu.sprites);
                        }
                        if let Some(screen) = &mut self.replays_screen {
                            // Exports and details are read in the
                            // background; keep drawing until they finish.
                            animating |= screen.poll();
                            screen.draw(&mut self.menu.pixels, &self.hornet.font);
                        }
                        animating
                    }
                    Screen::Quick => self.quick.render(
                        &mut self.menu.pixels,
                        &self.menu.quick_sprites,
                        &self.world.terrain,
                    ),
                    Screen::Flight => {
                        self.scenery.no_sun_whiteout = self.flight_ui.cheats.no_sun_whiteout;
                        let now = Instant::now();
                        // The part of the gap since the last frame that the
                        // pacer did not choose to wait: what says the loop
                        // ran late (an unfocused window's tenth of a second
                        // or a still picture's wait does not).
                        let busy = self.pacer.busy(self.frame_time, now);
                        // Captures draw the explicitly prepared state. Startup
                        // and GPU initialization time must not advance it.
                        let elapsed = if self.capture_terrain.is_some() {
                            0.
                        } else {
                            (now - self.frame_time).as_secs_f64().min(0.25)
                        };
                        // A stopped flight with a still picture draws only on
                        // events, so its last frame can be long before the
                        // resume. Every resume resets the clock; should one
                        // not, the first running frame still owes no more
                        // than a frame's time, as when stopped frames ran
                        // continuously.
                        let elapsed = if self.stopped_frame && !self.flight_ui.stopped() {
                            elapsed.min(RESUME_MAX_ELAPSED)
                        } else {
                            elapsed
                        };
                        self.stopped_frame = self.flight_ui.stopped();
                        // A networked flight's ticks are the client session's.
                        let session = self.net_flight.is_some();
                        let steps = if session {
                            0
                        } else {
                            self.flight_ui.steps(&mut self.flight_clock, elapsed)
                        };
                        self.frame_time = now;
                        if let Some(audio) = &self.audio {
                            if !session {
                                let frame = tick_frame(&self.world, SEAT, &[]);
                                audio.seeker(frame.readout.seeker.tone);
                            }
                            audio.pause_flight(self.flight_ui.stopped());
                        }
                        // Pauses, time compression and cheats, noted as they happen.
                        if let Some(recording) = &mut self.replay_recorder {
                            recording.session(&self.flight_ui);
                        }
                        let mut output = world::TickOutput::default();
                        let ticks_started = Instant::now();
                        let sim_log = frame_log::span(frame_log::Phase::Simulation);
                        for _ in 0..steps {
                            frame_log::count(frame_log::Count::Ticks, 1);
                            // Navigation-page clicks become airport commands,
                            // from the state at the start of the tick.
                            let start = seat_cockpit(&self.world, SEAT);
                            self.instruments.navigation.refresh(
                                &self.world.terrain.airport_scene,
                                &start.airport_service,
                                start.flight.position,
                                self.world.roster.redfor(start.plane),
                            );
                            for button in std::mem::take(&mut self.instruments.navigation.pending) {
                                if let Some(id) = self.instruments.navigation.control(button) {
                                    queue_command(
                                        &mut self.seat_commands,
                                        seats::SeatCommand::Airport(world::AirportInput::Command(
                                            tore_sim::airport::Command::SelectAirport(id),
                                        )),
                                    );
                                }
                            }
                            sync_stability(
                                &self.world.cockpits[OWN].flight,
                                &mut self.input,
                                self.flight_ui.stability,
                            );
                            let lever = self.input.throttle_reference(
                                start.flight.throttle,
                                start.flight.lift_controls.collective,
                            );
                            self.input.console.open =
                                console_open(&self.input, &self.world, &self.net_flight);
                            let (pilot, _) = self.input.frame(&self.camera.keys, lever);
                            for command in self.input.console.take() {
                                queue_command(
                                    &mut self.seat_commands,
                                    seats::SeatCommand::Console(command),
                                );
                            }
                            if let Some(recording) = &mut self.input_recording {
                                self.recorded_ticks += 1;
                                if let Err(error) = tore_input::recording::write_frame(
                                    recording,
                                    self.recorded_ticks,
                                    &pilot,
                                ) {
                                    self.error = Some(error.into());
                                    event_loop.exit();
                                    return;
                                }
                            }
                            // The commands given since the last tick, in order.
                            queue_gun_key(
                                &mut self.seat_commands,
                                &mut self.gun_key_sent,
                                self.screen == Screen::Flight
                                    && !self.flight_ui.frozen()
                                    && self.focused
                                    && self.input.resolver.held("fire-gun"),
                            );
                            let commands = std::mem::take(&mut self.seat_commands);
                            let (sight, sight_zoom) = self.input.sight();
                            let input = seats::SeatInput {
                                seat: SEAT,
                                tick: self.world.tick(),
                                pilot,
                                trigger: self.input.resolver.held("fire"),
                                // Scope channel, display range and history are
                                // player controls, part of the tick's input so
                                // a recording reproduces every change.
                                sensors: self.instruments.controls(),
                                // The AC-130 gunsight's slew and zoom step,
                                // from the bound keys and axes.
                                sight,
                                sight_zoom,
                                commands,
                                // A local seat: no lag compensation.
                                view: None,
                            };
                            // The cheats the flight menu changed reach the
                            // mission before any seat's commands.
                            let cheats = self.flight_ui.cheats;
                            let mission: Vec<_> = (self.cheats_sent != Some(cheats))
                                .then(|| {
                                    self.cheats_sent = Some(cheats);
                                    world::MissionCommand::Settings(world::Settings { cheats })
                                })
                                .into_iter()
                                .collect();
                            // The Debug menu's AI test settings, from this tick.
                            if let Some(wings) = self.world.ai_wings.as_mut() {
                                wings.set_ai_test(self.flight_ui.ai_test);
                            }
                            let stepped = self.world.step_with(
                                &mission,
                                std::slice::from_ref(&input),
                                &mut output,
                                |_, _| Ok(()),
                            );
                            if let Some(tape) = &mut self.combat_tape {
                                tape.write_all(self.world.combat.take_tape());
                            }
                            if let Some(wings) = &mut self.world.ai_wings {
                                formation_trace::drain(&mut self.formation_trace, wings);
                            }
                            if let Err(error) = stepped {
                                self.error = Some(error);
                                event_loop.exit();
                                return;
                            }
                            let presented = TickPresenter {
                                world: &mut self.world,
                                scenery: &mut self.scenery,
                                flight_ui: &mut self.flight_ui,
                                input: &mut self.input,
                                audio: self.audio.as_ref(),
                                recorder: self.replay_recorder.as_mut(),
                                instruments: &mut self.instruments,
                                flight_view: &mut self.flight_view,
                                view_rig: &mut self.view_rig,
                                hornet: &self.hornet,
                                combat_view: &self.combat_view,
                                head_look: self.head_look,
                                g_effects: &mut self.g_effects,
                                perf_blackout: self.performance.veil_level(),
                                vapor: &mut self.vapor,
                                flight_music: &mut self.flight_music,
                                rwr_warnings: &mut self.rwr_warnings,
                                launch_watch: &mut self.launch_watch,
                                engine_sounds: &mut self.engine_sounds,
                                spatial: &mut self.spatial_steps,
                            }
                            .present(&input, &output);
                            if !presented {
                                break;
                            }
                        }
                        drop(sim_log);
                        // The ticks' spatial sound, on the sources and loops
                        // of the last of them.
                        if let Some(audio) = &self.audio
                            && !self.spatial_steps.is_empty()
                        {
                            let own = seat_cockpit(&self.world, SEAT);
                            audio.spatial_frame(
                                &self.spatial_steps,
                                &audio::spatial_sources(
                                    &self.world.combat.state,
                                    own.plane.0,
                                    &own.flight,
                                ),
                                &audio::loop_sources(
                                    self.world.combat.render_snapshot(),
                                    self.engine_sounds.of(self
                                        .world
                                        .combat
                                        .dummy_types()
                                        .iter()
                                        .map(|kind| &**kind)
                                        .chain([&*self.hornet])),
                                ),
                            );
                        }
                        self.spatial_steps.clear();
                        self.performance.ticks(steps, self.flight_ui.time_scale);
                        // Slice S9: the shared workers fan out only while the
                        // flight is compressed, a tick ran over its budget or
                        // a frame ran long (over 100 ms that the pacer did not
                        // ask to wait: an unfocused window drawing ten frames
                        // a second is not late).
                        tore_workers::observe(tore_workers::Load {
                            compression: if steps > 0 {
                                self.flight_ui.time_scale
                            } else {
                                1.
                            },
                            tick_cost: ticks_started.elapsed() / steps.max(1) as u32,
                            late: busy.as_secs_f64() > LATE_FRAME_SECONDS,
                        });
                        self.input.feedback_flush();
                        // In the sensor console the look keys and axes fly
                        // the console's autopilot (Input::frame), not the view.
                        if !self.flight_ui.frozen() && !self.input.console.open {
                            let analog_look = self.input.resolver.look();
                            look::step_axes(
                                &mut self.flight_ui.look,
                                analog_look,
                                elapsed,
                                matches!(self.flight_view, 1 | 2),
                            );
                            self.head_look = self.input.head_look().unwrap_or([0.; 2]);
                        }
                        if !session {
                            self.combat_view.present(
                                &self.world.combat,
                                if self.flight_ui.frozen() {
                                    1.0
                                } else {
                                    self.flight_clock.remainder / flight::DT
                                },
                            );
                        }
                        // Everything combat draws this frame, shared by every camera.
                        let picture = self.combat_view.presented(&self.world.combat);
                        let standing = render_snapshot::standing(&self.world.combat.state.targets);
                        // Surface units turn toward what their weapons hold
                        // (docs/spec/surface-defenses.md, "Aiming"); a client
                        // knows only the bursts.
                        {
                            let surface = &self.world.terrain.surface;
                            let mut tracks = tore_world::surface::aim::tracks(
                                surface,
                                &self.world.combat.surface,
                            );
                            if let Some(net) = &self.net {
                                tracks.extend(net.surface_aim_tracks(picture.tick));
                            }
                            self.scenery.surface.step_aim(
                                &picture,
                                surface,
                                &tracks,
                                Some(&self.world.combat.surface),
                            );
                        }
                        // The flight frame every screen below draws: the seat's
                        // plane at this frame's instant, the picture, the smoke
                        // and the devices. A networked flight's comes from the
                        // client session.
                        let net_frame = self.net_flight.as_ref().and_then(|f| f.frame.as_ref());
                        let frame = match (net_frame, self.net.as_ref()) {
                            (Some(client), Some(net)) => {
                                let world = &self.world;
                                client.flight_frame(
                                    [&net.effects.smoke, &net.effects.contrails],
                                    &net.effects.devices,
                                    || {
                                        world
                                            .cockpit_readout(
                                                SEAT,
                                                combat::launcher(&client.presented),
                                            )
                                            .expect("the seat flies a plane")
                                    },
                                )
                            }
                            _ => self
                                .world
                                .flight_frame(
                                    SEAT,
                                    (!self.flight_ui.frozen())
                                        .then(|| {
                                            self.world.presented_flight(
                                                SEAT,
                                                self.flight_clock.remainder / flight::DT,
                                            )
                                        })
                                        .flatten(),
                                    &picture,
                                    &[],
                                )
                                .expect("the presented seat flies a plane"),
                        };
                        let presented = frame.presented();
                        // AC-130 muzzle flashes: the gunship rounds new in this
                        // picture, drawn on the barrels as they are posed now.
                        let flash_now = picture.tick as f64
                            + if session || self.flight_ui.frozen() {
                                0.
                            } else {
                                self.flight_clock.remainder / flight::DT
                            };
                        let flash_player = (self.hornet.profile.id
                            == tore_formats::aircraft::AircraftId::Ac130)
                            .then_some((frame.plane.0, presented));
                        self.gun_flash
                            .set_fire_widths(Arc::clone(&self.scenery.fire_widths));
                        self.gun_flash.observe(
                            &picture,
                            flash_now,
                            gun_flash::mounts(&picture, flash_player),
                        );
                        let gun_flashes = self
                            .gun_flash
                            .draw(flash_now, gun_flash::mounts(&picture, flash_player));
                        // The target camera whites out for a moment on the player's
                        // own 105 mm shot.
                        self.instruments.sight_bloom = if flash_player.is_some() {
                            self.gun_flash.bloom(frame.plane.0, flash_now)
                        } else {
                            0.
                        };
                        // The AC-130 gunsight's camera this frame: the sim's look
                        // carried smoothly between ticks, from the sensor turret.
                        let sight_frame = frame.readout.gunsight.as_ref().map(|gunsight| {
                            let tracked = gunsight.sight.tracked().and_then(|_| {
                                self.combat_view
                                    .display_position(&self.world.combat, &frame.readout)
                            });
                            let alpha = if self.flight_ui.frozen() {
                                1.
                            } else {
                                self.flight_clock.remainder / flight::DT
                            };
                            self.sight.frame(
                                gunsight.look,
                                frame.flight.ticks,
                                alpha,
                                &combat::launcher(presented),
                                self.input.sight().1,
                                tracked,
                            )
                        });
                        // The targeting page's camera: the sight's own, from its
                        // sensor, or an aircraft with no sight looking at a buddy's
                        // laser spot. The look is carried between ticks even while
                        // the page is not showing, so it never snaps in.
                        let targeting_form =
                            instruments::targeting::form(&frame.readout, frame.config);
                        // A laser weapon's picture marks the spot it homes on.
                        let laser_spot = frame
                            .readout
                            .laser
                            .and_then(|laser| laser.spot)
                            .map(|spot| spot.spot.position);
                        self.seeker.set_grace(if self.net_flight.is_some() {
                            seeker_view::NET_GRACE_TICKS
                        } else {
                            0
                        });
                        let seeker_shot = self.seeker.observe(
                            frame.picture,
                            frame.plane.0,
                            presented.position,
                            laser_spot,
                        );
                        let seeker_hidden = seeker_shot.as_ref().map(|shot| shot.id);
                        // A launch opens the target camera window if it is
                        // not open; it closes again when the view ends.
                        self.seeker.sync_window(&mut self.instruments);
                        let sight_shot = if let Some(sight) = frame.readout.sight.as_ref() {
                            let tracked = sight.mode.tracked().and_then(|_| {
                                self.combat_view
                                    .display_position(&self.world.combat, &frame.readout)
                            });
                            let alpha = if self.flight_ui.frozen() {
                                1.
                            } else {
                                self.flight_clock.remainder / flight::DT
                            };
                            let shot = self.sight.sensor_frame(
                                sight.sensor.eye,
                                sight.look,
                                frame.flight.ticks,
                                alpha,
                                &combat::launcher(presented),
                                self.input.sight().1,
                                tracked,
                            );
                            (targeting_form == Some(instruments::targeting::Form::Sight)).then_some(
                                gunsight_view::TargetingShot::Sight {
                                    frame: shot,
                                    mount: sight.sensor.eye,
                                    tracked: tracked.is_some(),
                                },
                            )
                        } else if targeting_form == Some(instruments::targeting::Form::Buddy) {
                            frame
                                .readout
                                .laser
                                .and_then(|laser| laser.spot)
                                .map(|spot| gunsight_view::TargetingShot::Buddy {
                                    view: gunsight_view::buddy_view(
                                        &combat::launcher(presented),
                                        spot.spot.position,
                                    ),
                                })
                        } else {
                            None
                        };
                        // A seeker weapon in flight takes the target camera: its
                        // nose view, until it lands or the pilot clears it.
                        let targeting_shot = seeker_shot
                            .as_ref()
                            .map(|shot| gunsight_view::TargetingShot::Seeker { view: shot.view })
                            .or(sight_shot);
                        if self.performance.measuring() {
                            if let Some(client) = net_frame {
                                self.performance.network_tick(client.tick);
                            }
                            let readout = &*frame.readout;
                            let weapon = frame.config.stations.get(readout.stores.selected());
                            self.performance.weapon(
                                weapon.map_or("UNARMED", |station| station.weapon.source.as_str()),
                                readout.targets.designated.is_some(),
                                readout.estimates.max_range.is_some(),
                            );
                        }
                        if presented.escape.is_some() {
                            self.flight_view = 1;
                            self.view_rig.select(flight_views::Reference::Player);
                        }
                        let scene = if session {
                            flight_views::Scene::from_frame(
                                &frame,
                                presented,
                                self.world.ai_wings.as_ref(),
                            )
                        } else {
                            flight_views::Scene::new(
                                &frame,
                                presented,
                                &self.world.combat,
                                self.world.ai_wings.as_ref(),
                                Some(&self.combat_view),
                            )
                        };
                        let scene =
                            self.view_rig
                                .prepare(scene, self.flight_view, &self.world.terrain);
                        let camera_keys = std::mem::take(&mut self.camera.keys);
                        let base =
                            self.hornet
                                .camera(presented, self.flight_view, camera_keys.clone());
                        self.camera = match self.view_rig.camera(
                            self.flight_view,
                            &scene,
                            base,
                            look::combine(
                                self.flight_ui.look,
                                self.head_look,
                                matches!(self.flight_view, 1 | 2),
                            ),
                            self.flight_ui.zoom,
                        ) {
                            Ok(camera) => camera,
                            Err(reason) => {
                                self.flight_ui
                                    .message(format!("{reason}; returning to Forward view"));
                                self.flight_view = 0;
                                self.view_rig.select(flight_views::Reference::Player);
                                self.flight_ui.look = [0.; 2];
                                self.flight_ui.zoom = 1.;
                                self.hornet.camera(presented, 0, camera_keys)
                            }
                        };
                        if !self.flight_ui.cheats.no_screen_shake && !presented.crashed {
                            let seconds = frame.flight.ticks as f64 * flight::DT
                                + self.flight_clock.remainder;
                            // High-G shake is the cockpit's; the overspeed shake
                            // shakes every view (requested by John, 2026-09-29).
                            let mut shake = if self.view_rig.cockpit(self.flight_view) {
                                tore_sim::g_effects::shake(presented.g, seconds)
                            } else {
                                [0.; 2]
                            };
                            if let Some(ratio) = presented.overspeed_ratio() {
                                let over = tore_sim::g_effects::overspeed_shake(ratio, seconds);
                                shake = [shake[0] + over[0], shake[1] + over[1]];
                            }
                            // A helicopter's rotor buffets in the vortex ring
                            // state and in retreating blade stall: the cockpit
                            // shakes (no message, as in the real aircraft).
                            if self.view_rig.cockpit(self.flight_view) {
                                let buffet = tore_sim::g_effects::rotor_buffet_shake(
                                    presented.rotor_buffet(),
                                    seconds,
                                );
                                shake = [shake[0] + buffet[0], shake[1] + buffet[1]];
                            }
                            if shake != [0.; 2] {
                                look::apply(
                                    &mut self.camera,
                                    presented.view_position(),
                                    [shake[0] as f32, shake[1] as f32],
                                    false,
                                );
                            }
                        }
                        self.camera.zoom = self.flight_ui.zoom;
                        // The AC-130 sensor console takes the whole screen: the
                        // world seen thermally from the sensor dome, along the
                        // sight's line at its zoom step (sensor_console.rs).
                        let console_on = sensor_console::console_active(&frame.readout, presented);
                        if console_on && let Some(sight_frame) = &sight_frame {
                            let view = gunsight_view::free_view(
                                &combat::launcher(presented),
                                sight_frame.look,
                                self.input.sight().1,
                            );
                            self.camera = gunsight_view::camera(&view);
                            self.camera.weather_slot = 0;
                            self.camera.thermal = self.console.mode.thermal();
                            self.camera.sensor = true;
                            self.camera.near_clip = 1.;
                        }
                        let console_on = console_on && sight_frame.is_some();
                        // The console's radar is the A-G radar: opening the
                        // console turns it on once (as Ctrl+R would) when it
                        // is off and the radar works.
                        if console_on
                            && !std::mem::replace(&mut self.console.was_open, true)
                            && !frame.readout.sensors.air_ground
                            && frame
                                .readout
                                .sensors
                                .available(tore_sim::sensors::Channel::Radar)
                            && !frame.readout.damage.radar_failed
                        {
                            self.instruments.channel = 0;
                            if self.net_flight.is_none() && !self.world.cockpits[OWN].flight.radar {
                                self.input.queue(tore_input::PilotCommand::Toggle(
                                    tore_input::Switch::Radar,
                                ));
                            }
                            queue_command(
                                &mut self.seat_commands,
                                seats::SeatCommand::Combat(
                                    tore_sim::combat::live::Command::ToggleAirGround,
                                ),
                            );
                            self.flight_ui.message("Air-Ground Radar on");
                        }
                        self.console.was_open = console_on;
                        // One resolved instant per frame, shared by the main view,
                        // the mirrors and the camera panels.
                        self.scenery
                            .resolve_palette(&self.world.terrain, self.camera.position[1]);
                        self.scenery.set_origin(self.camera.position);
                        let vapor = vapor_vertices(
                            &self.vapor,
                            &self.world.terrain,
                            presented,
                            self.hornet.streamer_points(presented),
                        );
                        renderer.vapor(&vapor);
                        renderer.smoke(&self.combat_view.art.smoke, frame.smoke, frame.devices);
                        renderer.effects(
                            &self.combat_view.art.effects,
                            &picture.effects,
                            &picture.marks,
                        );
                        renderer.emitters(
                            frame.devices,
                            &self
                                .combat_view
                                .afterburner_glows(&self.world.combat, &frame),
                            &gun_flashes,
                        );
                        match renderer.poll_previews() {
                            Ok(previews) => {
                                self.performance.completed_previews += previews.len();
                                for (page, pixels) in previews {
                                    if page == 4 {
                                        let requested = self.instruments.target_preview.take();
                                        if requested
                                            != frame.readout.targets.display.as_ref().map(|t| t.id)
                                        {
                                            continue;
                                        }
                                        self.instruments.camera_target = requested;
                                    }
                                    if page == 3
                                        && !std::mem::take(&mut self.view_rig.other_pending)
                                    {
                                        continue;
                                    }
                                    if page == 2 {
                                        self.instruments.front_shown =
                                            self.instruments.front_pending.take();
                                    }
                                    if page == 2 || page == 3 {
                                        match self.instruments.aim_pending.remove(&page) {
                                            Some(layout) => {
                                                self.instruments.aim_shown.insert(page, layout);
                                            }
                                            None => {
                                                self.instruments.aim_shown.remove(&page);
                                            }
                                        }
                                    }
                                    self.instruments.cameras.insert(page, pixels);
                                }
                            }
                            Err(e) => {
                                self.error = Some(e);
                                event_loop.exit();
                                return;
                            }
                        }
                        renderer.airports(self.scenery.static_geometry(
                            &self.world.combat.state.targets,
                            &self.world.combat.surface,
                        ));
                        let target_due = self.target_refresh.due(now) || self.smoke_test;
                        let other_due = now.duration_since(self.instrument_time).as_millis() >= 100
                            || self.smoke_test;
                        if target_due || other_due {
                            if other_due {
                                self.instrument_time = now;
                            }
                            for page in [2, 3, 4] {
                                if self.instruments.pages.contains(&page)
                                    && if page == 4 { target_due } else { other_due }
                                {
                                    let camera = if page == 4
                                        && let Some(sight_frame) = &sight_frame
                                    {
                                        // The gunsight looks along the sight; a tracked
                                        // object keeps the automatic framing, from the
                                        // sensor turret.
                                        let mut from_eye = presented.clone();
                                        from_eye.position =
                                            gunsight_view::eye(&combat::launcher(presented));
                                        let framed = (frame
                                            .readout
                                            .gunsight
                                            .as_ref()
                                            .is_some_and(|g| g.sight.tracked().is_some()))
                                        .then(|| {
                                            self.combat_view.framed_target_camera(
                                                &self.world.combat,
                                                &frame.readout,
                                                &from_eye,
                                                &self.hornet,
                                                &self.world.terrain,
                                                &self.scenery,
                                            )
                                        })
                                        .flatten();
                                        match framed {
                                            Some(camera) => {
                                                self.sight.fitted_zoom = camera.zoom;
                                                camera
                                            }
                                            None => gunsight_view::camera(&sight_frame.view),
                                        }
                                    } else if page == 4
                                        && let Some(shot) = &targeting_shot
                                    {
                                        match shot {
                                            gunsight_view::TargetingShot::Sight {
                                                frame: sight_frame,
                                                mount,
                                                tracked,
                                            } => {
                                                // As the AC-130's gunsight: a tracked
                                                // object keeps the automatic framing,
                                                // from the sensor's eye.
                                                let mut from_eye = presented.clone();
                                                from_eye.position = gunsight_view::sensor_eye(
                                                    &combat::launcher(presented),
                                                    *mount,
                                                );
                                                let framed = tracked
                                                    .then(|| {
                                                        self.combat_view.framed_target_camera(
                                                            &self.world.combat,
                                                            &frame.readout,
                                                            &from_eye,
                                                            &self.hornet,
                                                            &self.world.terrain,
                                                            &self.scenery,
                                                        )
                                                    })
                                                    .flatten();
                                                match framed {
                                                    Some(camera) => {
                                                        self.sight.fitted_zoom = camera.zoom;
                                                        camera
                                                    }
                                                    None => {
                                                        gunsight_view::camera(&sight_frame.view)
                                                    }
                                                }
                                            }
                                            gunsight_view::TargetingShot::Buddy { view } => {
                                                gunsight_view::camera(view)
                                            }
                                            gunsight_view::TargetingShot::Seeker { view } => {
                                                let mut camera = gunsight_view::camera(view);
                                                camera.hidden_projectile = seeker_hidden;
                                                camera
                                            }
                                        }
                                    } else if page == 4 {
                                        let Some(camera) = self.combat_view.framed_target_camera(
                                            &self.world.combat,
                                            &frame.readout,
                                            presented,
                                            &self.hornet,
                                            &self.world.terrain,
                                            &self.scenery,
                                        ) else {
                                            self.instruments.cameras.remove(&4);
                                            self.instruments.camera_target = None;
                                            continue;
                                        };
                                        camera
                                    } else if page == 3 {
                                        let base = self.hornet.camera(
                                            presented,
                                            self.view_rig.other_view(),
                                            Default::default(),
                                        );
                                        let Ok(camera) = self.view_rig.other_camera(&scene, base)
                                        else {
                                            self.instruments.cameras.remove(&3);
                                            continue;
                                        };
                                        camera
                                    } else {
                                        self.hornet.panel_camera(presented, page)
                                    };
                                    let front = (page == 2).then(|| {
                                        instruments::front_view::Symbology::new(
                                            presented,
                                            self.world.terrain.air_data(presented).ok().as_ref(),
                                        )
                                    });
                                    // The AC-130's aim-point marks on the Front View and
                                    // Other View pictures, placed with this camera.
                                    let aim_layout = (page == 2 || page == 3)
                                        .then(|| {
                                            aim_box::marks(&frame.readout, frame.config, false)
                                                .and_then(|marks| {
                                                    aim_box::layout(
                                                        &camera,
                                                        [138, 114],
                                                        &marks,
                                                        aim_box::PAGE_BOX,
                                                    )
                                                })
                                        })
                                        .flatten();
                                    renderer.dummies(render_snapshot::aircraft_batches(
                                        &picture,
                                        &self.combat_view.models,
                                        &self.combat_view.art,
                                        &camera,
                                        &self.world.terrain,
                                        &self.scenery,
                                    ));
                                    renderer.combat(&render_snapshot::combat_geometry(
                                        &picture,
                                        &self.combat_view.art,
                                        &self.hornet,
                                        presented,
                                        &camera,
                                        &self.world.terrain,
                                        &self.scenery,
                                    ));
                                    renderer.surface_units(&self.scenery.surface_vertices(
                                        &picture,
                                        &|id| standing.contains(&id),
                                        &camera,
                                    ));
                                    renderer.aircraft(
                                        &self.hornet,
                                        presented,
                                        &pylons::for_flown(
                                            &picture.player.stores,
                                            presented,
                                            &self.hornet,
                                            &self.combat_view.art,
                                            &self.scenery,
                                        ),
                                        page == 3 && self.view_rig.other_shows_player(),
                                        &camera,
                                        &self.world.terrain,
                                        &self.scenery,
                                    );
                                    let result = if self.smoke_test {
                                        renderer
                                            .scene_pixels(
                                                sim_renderer::ViewKind::Panel(page),
                                                &camera,
                                                &self.world.terrain,
                                                &self.scenery,
                                                138,
                                                114,
                                                false,
                                            )
                                            .map(|p| {
                                                self.instruments.cameras.insert(page, p);
                                                if page == 2 {
                                                    self.instruments.front_shown = front;
                                                }
                                                if let Some(layout) = aim_layout {
                                                    self.instruments.aim_shown.insert(page, layout);
                                                } else {
                                                    self.instruments.aim_shown.remove(&page);
                                                }
                                                if page == 4 {
                                                    self.instruments.camera_target = frame
                                                        .readout
                                                        .targets
                                                        .display
                                                        .as_ref()
                                                        .map(|t| t.id);
                                                }
                                                true
                                            })
                                    } else {
                                        renderer
                                            .request_preview(
                                                page,
                                                &camera,
                                                &self.world.terrain,
                                                &self.scenery,
                                            )
                                            .inspect(|submitted| {
                                                if page == 3 && *submitted {
                                                    self.view_rig.other_pending = true;
                                                }
                                                if page == 2 && *submitted {
                                                    self.instruments.front_pending = front;
                                                }
                                                if (page == 2 || page == 3) && *submitted {
                                                    match aim_layout {
                                                        Some(layout) => {
                                                            self.instruments
                                                                .aim_pending
                                                                .insert(page, layout);
                                                        }
                                                        None => {
                                                            self.instruments
                                                                .aim_pending
                                                                .remove(&page);
                                                        }
                                                    }
                                                }
                                                if page == 4 && *submitted {
                                                    self.instruments.target_preview = frame
                                                        .readout
                                                        .targets
                                                        .display
                                                        .as_ref()
                                                        .map(|t| t.id);
                                                }
                                            })
                                    };
                                    if let Err(e) = result {
                                        self.error = Some(e);
                                        event_loop.exit();
                                        return;
                                    }
                                }
                            }
                        }
                        if let Some(art) = &self.combat_view.art.escape {
                            renderer.escapees(
                                art,
                                &art.vertices_for(
                                    picture
                                        .pilots
                                        .iter()
                                        .map(|p| (p.position, p.heading, p.phase)),
                                    &self.hornet.palette,
                                    self.camera.position,
                                    self.scenery.origin,
                                ),
                            );
                        }
                        renderer.dummies(render_snapshot::aircraft_batches(
                            &picture,
                            &self.combat_view.models,
                            &self.combat_view.art,
                            &self.camera,
                            &self.world.terrain,
                            &self.scenery,
                        ));
                        renderer.combat(&render_snapshot::combat_geometry(
                            &picture,
                            &self.combat_view.art,
                            &self.hornet,
                            presented,
                            &self.camera,
                            &self.world.terrain,
                            &self.scenery,
                        ));
                        renderer.surface_units(&self.scenery.surface_vertices(
                            &picture,
                            &|id| standing.contains(&id),
                            &self.camera,
                        ));
                        renderer.aircraft(
                            &self.hornet,
                            presented,
                            &pylons::for_flown(
                                &picture.player.stores,
                                presented,
                                &self.hornet,
                                &self.combat_view.art,
                                &self.scenery,
                            ),
                            self.view_rig.shows_player(self.flight_view) && !console_on,
                            &self.camera,
                            &self.world.terrain,
                            &self.scenery,
                        );
                        simulation_ms = frame_start.elapsed().as_secs_f64() * 1000.;
                        self.instruments.combat = Some(combat_view::readout(
                            &self.world.combat,
                            frame.config,
                            &frame.readout,
                            frame.flight,
                            self.instruments.controls(),
                            self.instruments.rcs_scale_nmi(),
                        ));
                        if let (Some(page), Some(sight_frame)) = (
                            self.instruments
                                .combat
                                .as_mut()
                                .and_then(|c| c.gunsight.as_mut()),
                            &sight_frame,
                        ) {
                            page.present(
                                sight_frame.look,
                                self.input.sight().1,
                                sight_frame.view,
                                frame
                                    .readout
                                    .gunsight
                                    .as_ref()
                                    .and_then(|g| g.sight.tracked())
                                    .and_then(|_| {
                                        self.combat_view
                                            .display_position(&self.world.combat, &frame.readout)
                                    }),
                            );
                        }
                        if let (Some(combat), Some(seeker)) =
                            (self.instruments.combat.as_mut(), &seeker_shot)
                        {
                            combat.targeting = Some(
                                instruments::targeting::Page::seeker(
                                    &seeker.weapon,
                                    seeker.target,
                                    seeker.range_ft,
                                    frame.readout.tick,
                                    seeker.view,
                                )
                                .closing(seeker.closing),
                            );
                        } else if let (Some(combat), Some(seed)) =
                            (self.instruments.combat.as_mut(), self.seeker.lost_feed())
                        {
                            // The weapon ended: a second of static.
                            combat.targeting = Some(instruments::targeting::Page::lost_feed(seed));
                        }
                        if let (Some(page), Some(shot)) = (
                            self.instruments
                                .combat
                                .as_mut()
                                .and_then(|c| c.targeting.as_mut()),
                            &targeting_shot,
                        ) {
                            // The lasing pilot's callsign names a buddy's spot: a
                            // player's in a networked game, an AI aircraft's wing and
                            // member ("Friendly 2-1") where this machine runs the AI.
                            let owner = frame
                                .readout
                                .laser
                                .and_then(|laser| laser.spot.filter(|s| !s.own).or(laser.buddy))
                                .and_then(|spot| {
                                    let player = self
                                        .net_flight
                                        .as_ref()
                                        .and(self.net.as_ref())
                                        .and_then(|net| net.client.lobby())
                                        .and_then(|lobby| {
                                            lobby
                                                .players
                                                .iter()
                                                .find(|player| player.slot == Some(spot.spot.owner))
                                                .map(|player| {
                                                    crate::replay::panels::ascii(&player.callsign)
                                                })
                                        });
                                    let ai = self
                                        .world
                                        .ai_wings
                                        .as_ref()
                                        .and_then(|wings| wings.callsign(spot.spot.owner));
                                    instruments::targeting::spot_owner_name(player, ai)
                                });
                            match shot {
                                gunsight_view::TargetingShot::Sight {
                                    frame: sight_frame,
                                    tracked,
                                    ..
                                } => page.present(
                                    sight_frame.look,
                                    self.input.sight().1,
                                    sight_frame.view,
                                    tracked
                                        .then(|| {
                                            self.combat_view.display_position(
                                                &self.world.combat,
                                                &frame.readout,
                                            )
                                        })
                                        .flatten(),
                                    owner,
                                ),
                                gunsight_view::TargetingShot::Buddy { view } => {
                                    page.present([0.; 2], 3, *view, None, owner)
                                }
                                // Built whole by `Page::seeker`.
                                gunsight_view::TargetingShot::Seeker { .. } => {}
                            }
                        }
                        if let (Some(readout), Some(wings)) = (
                            self.instruments.combat.as_mut(),
                            self.world.ai_wings.as_ref(),
                        ) {
                            for emitter in &mut readout.rwr.emitters {
                                if emitter.kind == scope::EmitterKind::EnemyAircraft
                                    && let Some(slot) = wings.slot(emitter.id)
                                    && slot.side == tore_sim::ai::launch::Side::Friendly
                                {
                                    emitter.kind = scope::EmitterKind::FriendlyAircraft;
                                }
                            }
                        }
                        // Hover feedback uses the same projection as the click,
                        // so the selector marks the contact a click would take.
                        let window = renderer.window.inner_size();
                        self.instruments.weapon_debug = self.view_rig.cockpit(self.flight_view)
                            && self.flight_ui.weapon_diagnostics_shown(self.flight_view);
                        // A debug panel or menu over an instrument takes the
                        // pointer, so the scope's crosshair stays away.
                        let respawn_open = self
                            .net_flight
                            .as_ref()
                            .is_some_and(|flight| flight.respawn_menu.is_some());
                        let covered = (self.flight_ui.debug_panels && self.live_debug.covers())
                            || respawn_open;
                        self.instruments.hover(
                            self.pointer.filter(|_| !covered),
                            [f64::from(window.width), f64::from(window.height)],
                        );
                        renderer.window.set_cursor_visible(
                            self.instruments.crosshair.is_none()
                                || respawn_open
                                || self.flight_ui.menu
                                || self.flight_ui.map.open
                                || self.live_debug.menu.is_some(),
                        );
                        let cockpit_palette = self.hornet.cockpit_palette(
                            &self.world.terrain,
                            &self.scenery,
                            self.camera.position[1],
                            0,
                        );
                        let green = self.hornet.cockpit_palette(
                            &self.world.terrain,
                            &self.scenery,
                            self.camera.position[1],
                            self.flight_ui.brightness,
                        )[usize::from(self.hornet.hud.primary_color)];
                        let hud_ink = self
                            .flight_ui
                            .lighting
                            .hud(green, self.flight_ui.brightness);
                        let ambient = cockpit_lighting::ambient(
                            &self.world.terrain,
                            &self.scenery,
                            self.camera.position[1],
                        );
                        let flood = self.flight_ui.lighting.flood_strength(ambient);
                        self.instruments.hud_color = hud_ink;
                        self.instruments.palette = cockpit_palette;
                        self.instruments.light_gain = self.flight_ui.lighting.instrument_gain();
                        self.instruments.flood = flood;
                        self.instruments.ambient = ambient;
                        renderer.cockpit_lighting(self.instruments.light_gain, flood, ambient);
                        // The map's opaque letterbox and the sensor console's
                        // blank canvas, both below, cover every instrument window:
                        // those frames start from a clear canvas without them.
                        let map_open = self.flight_ui.map.open;
                        let console_blank = console_on
                            && self
                                .instruments
                                .combat
                                .as_ref()
                                .is_some_and(|c| c.gunsight.is_some());
                        if map_open || console_blank {
                            self.flight_canvas.clear(renderer.flight_size());
                        } else {
                            self.flight_canvas.begin(
                                renderer.flight_size(),
                                &self.hornet,
                                presented,
                                &self.instruments,
                            );
                        }
                        self.menu.pixels.fill(0);
                        let gyro_bank = self.flight_ui.bank_gyro.follow(
                            presented.bank,
                            if self.flight_ui.frozen() { 0. } else { elapsed },
                        );
                        if self.flight_ui.hud && self.view_rig.cockpit(self.flight_view) {
                            let airport_aircraft = airport_aircraft(
                                &self.world.terrain,
                                presented,
                                frame.readout.airport.nav_mode,
                                self.world.roster.redfor(frame.plane),
                            );
                            let guidance = frame
                                .readout
                                .airport
                                .service
                                .as_ref()
                                .and_then(|service| {
                                    service.guidance(
                                        &self.world.terrain.airport_scene,
                                        airport_aircraft,
                                    )
                                })
                                .filter(|_| frame.readout.airport.nav_mode);
                            let ils = guidance.as_ref().and_then(|g| {
                                let airport = self
                                    .world
                                    .terrain
                                    .airport_scene
                                    .airports
                                    .iter()
                                    .find(|a| a.id == g.airport)?;
                                let runway = self.world.terrain.airport_scene.runway(g.runway)?;
                                Some((g, airport.name.as_str(), runway.name.as_str()))
                            });
                            hud::draw(
                                &mut self.menu.pixels,
                                presented,
                                &self.hornet.hud_font,
                                self.world.terrain.height(
                                    presented.position[0] as f32,
                                    presented.position[2] as f32,
                                ) as f64,
                                self.world.terrain.air_data(presented).ok().as_ref(),
                                self.flight_ui.ladder,
                                !frame.readout.airport.nav_mode
                                    && (weapon_hud::active(&frame) || self.gun_key_sent),
                                hud_ink,
                                self.flight_canvas.hud_zoom(1.),
                                ils,
                                airport_wind(&self.world.terrain, presented, guidance.as_ref())
                                    .as_ref(),
                                (gyro_bank, self.flight_ui.time_scale),
                            );
                        }
                        // The X marks a target on the presented plane's own side, so
                        // a player flying for the enemy sees it on the enemy's aircraft.
                        let sides = target_info::Sides {
                            roster: &self.world.roster,
                            wings: self.world.ai_wings.as_ref(),
                            scene: &self.world.terrain.airport_scene,
                        };
                        let target_friendly = frame
                            .readout
                            .targets
                            .display
                            .as_ref()
                            .is_some_and(|target| sides.friendly_to(frame.plane, target.id));
                        // The AC-130's aim-point box: always one, on the tracked
                        // target, the pinned point or the free-slew point, in every
                        // view while the HUD shows.
                        let in_cockpit = self.view_rig.cockpit(self.flight_view);
                        let aim = aim_box::marks(&frame.readout, frame.config, target_friendly)
                            .filter(|_| self.flight_ui.hud);
                        let aim_layout = aim.as_ref().and_then(|marks| {
                            aim_box::layout(
                                &self.camera,
                                self.flight_canvas.size,
                                marks,
                                aim_box::WIDTH
                                    * self.flight_canvas.hud_pixel(f64::from(self.flight_ui.zoom)),
                            )
                        });
                        // Easy targeting draws the square wherever the target is on
                        // screen, in place of the HUD's square or edge arrow. The
                        // AC-130 does the same with the aim-point box.
                        let hud_zoom = f64::from(self.flight_canvas.hud_zoom(1.));
                        let easy_square: Option<[f64; 2]> = if let Some(layout) = &aim_layout {
                            match (layout.boxed, &aim) {
                                (aim_box::Cue::On(point), Some(marks))
                                    if !(in_cockpit
                                        && weapon_hud::point_in_hud(
                                            &frame, marks.aim, hud_zoom,
                                        )) =>
                                {
                                    Some(point)
                                }
                                _ => None,
                            }
                        } else {
                            (self.flight_ui.cheats.easy_targeting
                                && self.flight_ui.hud
                                && in_cockpit
                                && aim.is_none()
                                && !weapon_hud::target_in_hud(&frame, hud_zoom))
                            .then_some(frame.readout.targets.display.as_ref())
                            .flatten()
                            .and_then(|target| {
                                self.camera
                                    .project(self.flight_canvas.size, target.position)
                            })
                        };
                        if self.flight_ui.hud && in_cockpit {
                            weapon_hud::draw(
                                &mut self.menu.pixels,
                                &frame,
                                &self.hornet.hud_font,
                                hud_ink,
                                hud_zoom,
                                target_friendly,
                                easy_square.is_none(),
                                aim.as_ref(),
                                self.gun_key_sent,
                            );
                        }
                        renderer.cockpit(
                            presented,
                            &self.camera,
                            self.flight_ui.cockpit
                                && !console_on
                                && !presented.wreck_gone()
                                && self.view_rig.cockpit(self.flight_view),
                            self.flight_ui.hud
                                && !console_on
                                && !presented.wreck_gone()
                                && self.view_rig.cockpit(self.flight_view),
                            &self.menu.pixels,
                            &cockpit_palette,
                        );
                        // Only the HUD drawings above left anything in the layer.
                        if self.flight_ui.hud && self.view_rig.cockpit(self.flight_view) {
                            self.menu.pixels.fill(0);
                        }
                        if self.view_rig.cockpit(self.flight_view)
                            && self.flight_ui.weapon_diagnostics_shown(self.flight_view)
                        {
                            weapon_hud::debug(
                                &mut self.menu.pixels,
                                &frame,
                                &self.world.combat.state,
                                &self.hornet.hud_font,
                                hud_ink,
                            );
                            self.flight_canvas.weapon_debug(&self.menu.pixels);
                            self.menu.pixels.fill(0);
                        }
                        let aim_color = hud_ink;
                        if let Some(point) = easy_square {
                            self.flight_canvas.target_square(
                                point,
                                f64::from(self.flight_ui.zoom),
                                aim_color,
                                aim.as_ref().map_or(
                                    aim_box::Style::Tracked {
                                        friendly: target_friendly,
                                    },
                                    |marks| marks.style,
                                ),
                            );
                        }
                        if let Some(layout) = &aim_layout {
                            // A box off screen is the HUD's edge arrow in a cockpit
                            // view and a screen-edge arrow in the rest.
                            if let aim_box::Cue::Edge(direction) = layout.boxed
                                && !in_cockpit
                            {
                                self.flight_canvas.edge_chevron(
                                    direction,
                                    f64::from(self.flight_ui.zoom),
                                    aim_color,
                                    &self
                                        .instruments
                                        .window_rects(self.flight_canvas.size.map(f64::from)),
                                );
                            }
                            if let Some(point) = layout.diamond {
                                self.flight_canvas.aim_diamond(
                                    point,
                                    f64::from(self.flight_ui.zoom),
                                    aim_color,
                                );
                            }
                        }
                        // The sensor console replaces the cockpit, the HUD and the
                        // instrument windows with its own symbology; the map,
                        // menus and messages still draw over it below.
                        if console_blank
                            && let Some(page) = self
                                .instruments
                                .combat
                                .as_ref()
                                .and_then(|c| c.gunsight.as_ref())
                        {
                            let seconds = self.console.seconds();
                            let symbology = sensor_console::Symbology::live(
                                page,
                                self.instruments
                                    .combat
                                    .as_ref()
                                    .and_then(|c| c.target.as_ref()),
                                &frame.readout,
                                frame.config,
                                presented,
                                self.world.terrain.air_data(presented).ok().as_ref(),
                                f64::from(self.world.terrain.height(
                                    presented.position[0] as f32,
                                    presented.position[2] as f32,
                                )),
                                self.console.mode,
                                seconds,
                                self.combat_view
                                    .display_position(&self.world.combat, &frame.readout),
                                self.instruments.radar_range,
                            );
                            let size = self.flight_canvas.size;
                            let overlay = sensor_console::draw(&symbology, &self.camera, size);
                            // How dark it is (night vision's gain).
                            let light =
                                sensor_console::night(&self.world.terrain, self.camera.position[1]);
                            renderer.set_console(
                                self.console.crt_frame(seconds, light),
                                overlay,
                                size,
                            );
                            // Only rows drawn on since the clear hold anything.
                            self.flight_canvas.clear(size);
                        }
                        use tore_sim::g_effects::GEffects;
                        // The canvas shader veils the finished frame on the GPU.
                        let ui_log = frame_log::span(frame_log::Phase::Ui);
                        // The map, menus and panels are drawn after this point,
                        // so while they show the veil goes on the canvas instead,
                        // under them (the pause freezes the levels).
                        let veil = [self.g_effects.redout, self.g_effects.blackout];
                        let under_overlay = self.flight_ui.frozen() || self.flight_ui.map.open;
                        renderer.set_veil(if under_overlay {
                            [0.; 2]
                        } else {
                            veil.map(|level| level as f32)
                        });
                        if under_overlay {
                            for (color, level) in [([150, 0, 0], veil[0]), ([0, 0, 0], veil[1])] {
                                if level > 0. {
                                    self.flight_canvas
                                        .veil(color, |radius| GEffects::coverage(level, radius));
                                }
                            }
                        }
                        if map_open {
                            self.flight_ui.map.draw(
                                &mut self.menu.pixels,
                                &self.world.terrain,
                                &self.scenery,
                                presented,
                                &frame.readout,
                                self.world
                                    .combat
                                    .state
                                    .ownship(frame.plane.0)
                                    .map_or(tore_sim::combat::live::DEFAULT_OWNSHIP_SIDE, |own| {
                                        own.side
                                    }),
                                &self.hornet.font,
                                &self.menu.quick_sprites,
                                self.mission_markup.as_ref(),
                            );
                            // Opaque letterbox prevents the cockpit leaking around the map.
                            self.flight_canvas.fill_opaque([72, 72, 72]);
                            self.flight_canvas.legacy_layer(&self.menu.pixels, 1.);
                            self.menu.pixels.fill(0);
                        }
                        // F7 alone carries the bearing compass, while it has a target.
                        if self.flight_view == flight_views::TARGET
                            && self.view_rig.reference == flight_views::Reference::Player
                            && !self.flight_ui.map.open
                            && let Some(target) = &frame.readout.targets.view
                        {
                            view_compass::draw(
                                &mut self.flight_canvas,
                                &self.instruments,
                                &self.hornet.hud_font,
                                view_compass::bearing(presented.position, target.position),
                            );
                        }
                        // The debug panels show what the recording writes:
                        // its comms entries and, while they show, its trees.
                        self.live_debug.recording = self.replay_recorder.is_some();
                        if let Some(recording) = &mut self.replay_recorder {
                            recording.set_live(self.flight_ui.debug_panels);
                            if self.flight_ui.debug_panels {
                                for entry in recording.take_comms() {
                                    self.live_debug.note(entry.tick, entry.event);
                                }
                                for (tick, tree) in recording.take_trees() {
                                    self.live_debug.sample(tick, tree);
                                }
                            }
                        }
                        if self.flight_ui.debug_panels && !self.flight_ui.menu {
                            self.live_debug.frame(
                                &mut self.flight_canvas,
                                &replay::live::Flight {
                                    frame: &picture,
                                    airframe: &self.hornet,
                                    player: frame.plane.0,
                                    mission: self.world.setup.mission.is_some(),
                                    wings: self.world.ai_wings.as_ref(),
                                    combat: &self.world.combat,
                                    reference: self.view_rig.reference,
                                    cockpit: self.view_rig.cockpit(self.flight_view),
                                    camera: &self.camera,
                                },
                            );
                        } else if !self.live_debug.idle() {
                            self.live_debug.reset();
                        }
                        // Show Target Info (Pref, Ctrl+T): each visible aircraft and
                        // object's identity below it, in the HUD's font and size.
                        if self.flight_ui.target_info && !self.flight_ui.map.open {
                            let size = self.flight_canvas.size.map(f64::from);
                            let scale =
                                (size[0] / 640.).min(size[1] / 480.) * flight_canvas::HUD_SCALE;
                            let lobby = self
                                .net_flight
                                .as_ref()
                                .and(self.net.as_ref())
                                .and_then(|net| net.client.lobby());
                            let labels = target_info::labels(
                                &target_info::View {
                                    picture: &picture,
                                    camera: &self.camera,
                                    size: self.flight_canvas.size,
                                    font: &self.hornet.hud_font,
                                    scale,
                                    brief: frame.readout.target_window.as_ref(),
                                },
                                &|pose| {
                                    target_info::identity(
                                        pose,
                                        self.world.combat.ground_name(pose.id),
                                    )
                                },
                                &|plane| {
                                    lobby.and_then(|lobby| {
                                        lobby
                                            .players
                                            .iter()
                                            .find(|player| player.slot == Some(plane))
                                            .map(|player| player.callsign.clone())
                                    })
                                },
                            );
                            target_info::draw(
                                &mut self.flight_canvas,
                                &self.hornet.hud_font,
                                scale,
                                &labels,
                            );
                        }
                        self.flight_ui.draw_notices(
                            &mut self.flight_canvas,
                            &self.hornet.hud_font,
                            self.instruments.hud_color,
                        );
                        net::chat::draw_window(
                            self.net_flight.as_ref().and(self.net.as_ref()),
                            &mut self.flight_canvas,
                            &self.hornet.hud_font,
                            self.instruments.layout,
                        );
                        let mission_timer = self
                            .net_flight
                            .as_ref()
                            .and(self.net.as_ref())
                            .and_then(|net| mission_timer::Timer::from_client(&net.client));
                        if !self.flight_ui.menu
                            && !map_open
                            && let Some(timer) = mission_timer
                        {
                            let size = self.flight_canvas.size;
                            let mut blocks: Vec<[f64; 4]> = self
                                .instruments
                                .window_rects(size.map(f64::from))
                                .into_iter()
                                .map(|(x, y, w, h)| [x, y, w, h])
                                .collect();
                            blocks.push(mission_timer::counter_rect(
                                size,
                                frame_rate::Surface::Flight {
                                    right_panel: self.live_debug.panels.right_open(),
                                },
                            ));
                            if self.live_debug.panels.right_open() {
                                let [w, h] = size.map(f64::from);
                                let scale = (w / 640.).min(h / 480.);
                                blocks.push([
                                    w / 2. + 60. * scale,
                                    28. * scale,
                                    254. * scale,
                                    h - 28. * scale,
                                ]);
                            }
                            if self.instruments.weapon_debug {
                                let layer =
                                    (f64::from(size[0]) / 640.).min(f64::from(size[1]) / 480.);
                                blocks.push([
                                    f64::from(size[0]) - 258. * layer,
                                    8. * layer,
                                    250. * layer,
                                    96. * layer,
                                ]);
                            }
                            timer.draw_corner(
                                &mut self.flight_canvas,
                                &self.hornet.hud_font,
                                &blocks,
                            );
                        }
                        net::scoreboard::draw(
                            self.net_flight.as_ref(),
                            self.net.as_ref(),
                            &mut self.flight_canvas,
                            &self.hornet.hud_font,
                        );
                        // The layer is clear here unless one of these draws on it.
                        let layer_drawn = self.flight_ui.menu
                            || self.flight_ui.paused
                            || self
                                .net_flight
                                .as_ref()
                                .is_some_and(|f| f.respawn_menu.is_some())
                            || self.controls.is_some()
                            || self.graphics_screen.is_some()
                            || self.sound_screen.is_some();
                        self.flight_ui.draw(
                            &mut self.menu.pixels,
                            &self.hornet.font,
                            net::play::menu(&self.hornet, &self.net_flight),
                        );
                        if self.flight_ui.menu
                            && let Some(timer) = mission_timer
                        {
                            timer.draw_menu(&mut self.menu.pixels, &self.hornet.font);
                        }
                        if let Some(menu) = self
                            .net_flight
                            .as_ref()
                            .and_then(|f| f.respawn_menu.as_ref())
                        {
                            let prompt = self
                                .net
                                .as_ref()
                                .and_then(|s| s.client.revival_prompt())
                                .unwrap_or_default();
                            menu.draw(
                                &mut self.menu.pixels,
                                &self.hornet.font,
                                &prompt,
                                &self.menu.quick_sprites,
                            );
                            if menu.brief_open
                                && let Some(brief) = &self.flight_ui.map.brief
                            {
                                brief.draw(&mut self.menu.pixels, &self.menu.quick_sprites);
                            }
                        }
                        if let Some(form) = &self.feedback {
                            form.draw(&mut self.menu.pixels, &self.hornet.font);
                        }
                        if let Some(editor) = &self.controls {
                            editor.draw(&mut self.menu.pixels, &self.hornet.font);
                            overlay_moving |= editor.tooltip_pending();
                        }
                        if let Some(editor) = &self.graphics_screen {
                            editor.draw(&mut self.menu.pixels, &self.hornet.font);
                        }
                        if let Some(screen) = &mut self.sound_screen {
                            overlay_moving |= screen.animate();
                            if screen.take_switch_sound()
                                && let Some(audio) = &self.audio
                            {
                                audio.action(Action::Toggle);
                            }
                            screen.draw(&mut self.menu.pixels, &self.menu.sprites);
                        }
                        if layer_drawn {
                            self.flight_canvas.legacy_layer(&self.menu.pixels, 1.);
                        }
                        if let Some(audio) = &self.audio {
                            // The tone follows the flight as the last tick left
                            // it, not the blended one the screen draws.
                            if !session {
                                audio
                                    .seeker(tick_frame(&self.world, SEAT, &[]).readout.seeker.tone);
                            }
                            audio.pause_flight(self.flight_ui.stopped());
                            audio.flight(Some((
                                &self.hornet.profile,
                                frame.flight,
                                f64::from(self.world.terrain.height(
                                    frame.flight.position[0] as f32,
                                    frame.flight.position[2] as f32,
                                )),
                            )));
                        }
                        drop(ui_log);
                        true
                    }
                    Screen::Viewer => {
                        let now = Instant::now();
                        // Captures draw the explicitly prepared state. Startup
                        // and GPU initialization time must not advance it.
                        let elapsed = if self.capture_terrain.is_some() {
                            0.
                        } else {
                            (now - self.frame_time).as_secs_f64().min(0.25)
                        };
                        self.camera.step(
                            elapsed as f32,
                            self.modifiers.shift_key(),
                            &self.world.terrain,
                        );
                        for _ in 0..self.flight_clock.steps(elapsed) {
                            self.world.terrain.weather.step();
                            self.scenery
                                .step_view_weather(&self.world.terrain, &self.camera, 0.);
                        }
                        self.scenery
                            .resolve_palette(&self.world.terrain, self.camera.position[1]);
                        self.scenery.set_origin(self.camera.position);
                        self.frame_time = now;
                        quick_mission::hud(
                            &mut self.menu.pixels,
                            &self.menu.quick_sprites,
                            &self.camera,
                            &self.world.terrain,
                        );
                        true
                    }
                    // Drawn by the replay viewer before reaching here.
                    Screen::Replay => return,
                };
                if self.screen != Screen::Flight {
                    renderer.window.set_cursor_visible(true);
                    renderer.aircraft(
                        &self.hornet,
                        &self.world.cockpits[OWN].flight,
                        &[],
                        false,
                        &self.camera,
                        &self.world.terrain,
                        &self.scenery,
                    );
                    if let Some(audio) = &self.audio {
                        audio.pause_flight(false);
                        audio.flight(None);
                    }
                }
                if self.screen == Screen::Viewer {
                    renderer.airports(self.scenery.static_geometry(
                        &self.world.combat.state.targets,
                        &self.world.combat.surface,
                    ));
                }
                let compose_ms = frame_start.elapsed().as_secs_f64() * 1000. - simulation_ms;
                let present_start = Instant::now();
                if matches!(self.screen, Screen::Main | Screen::Quick)
                    && (self.direct.screen.is_some()
                        || self.internet.screen.is_some()
                        || self.lobby.screen.is_some())
                    && let Some(kit) = self.direct.kit()
                {
                    ui_text::begin();
                    if let Some(layer) = &menu_text {
                        ui_text::replay(layer);
                    }
                    self.multiplayer_menu
                        .draw(&mut menu::Canvas(&mut self.menu.pixels), &kit);
                    menu_text = ui_text::finish();
                }
                renderer.set_menu_text(menu_text.as_ref());
                renderer.show_menu_text(menu_text.is_some());
                let readout = if self.screen == Screen::Flight {
                    frame_rate::Surface::Flight {
                        right_panel: self.live_debug.panels.right_open(),
                    }
                } else {
                    frame_rate::Surface::Menu
                };
                if self.screen == Screen::Flight {
                    self.flight_canvas.prepare_upload();
                }
                match renderer.draw(
                    if self.screen == Screen::Flight {
                        &self.flight_canvas.pixels
                    } else {
                        &self.menu.pixels
                    },
                    (matches!(self.screen, Screen::Viewer | Screen::Flight)).then_some((
                        &self.camera,
                        &self.world.terrain,
                        &self.scenery,
                    )),
                    (self.screen == Screen::Flight).then_some(self.flight_canvas.size),
                    (self.screen == Screen::Flight).then_some(&mut self.flight_canvas.pending),
                    readout,
                ) {
                    Ok(true) if self.smoke_test => {
                        if let Some(path) = &self.capture_terrain
                            && let Err(error) = renderer.capture_sim(
                                path,
                                &self.camera,
                                &self.world.terrain,
                                &self.scenery,
                                self.screen == Screen::Flight,
                            )
                        {
                            self.error = Some(error);
                        }

                        log::info!("Smoke test: requested screen presented successfully");
                        println!("Smoke test: requested screen presented successfully");
                        self.finished = true;
                        event_loop.exit();
                    }
                    Ok(presented) => {
                        animating &= presented;
                    }
                    Err(error) => {
                        self.error = Some(error);
                        event_loop.exit();
                    }
                }
                if matches!(self.screen, Screen::Flight | Screen::Viewer)
                    && self.performance.record(
                        frame_start,
                        simulation_ms,
                        compose_ms,
                        present_start.elapsed().as_secs_f64() * 1000.,
                        self.flight_ui.frozen(),
                    )
                {
                    println!(
                        "  rear mirror renders: {} (every other visible frame; no readback)",
                        renderer.mirror_frames
                    );
                    self.finished = true;
                    event_loop.exit();
                }
                // Live views are paced by the frame rate choice: V-sync by
                // presentation, the capped rates by a timer (`frame_pacing`).
                let want = match self.screen {
                    _ if !animating => frame_pacing::Want::Nothing,
                    Screen::Flight => self.flight_want(overlay_moving),
                    Screen::Viewer => frame_pacing::Want::Live,
                    _ => frame_pacing::Want::Menu,
                };
                self.next_frame = self.pace(want);
                return;
            }
            _ => return,
        };
        self.action(event_loop, action);
        let paused = self.input_paused();
        self.input.context(paused, self.focused);
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // A session ends politely when the game does, handing hosting over
        // first when a standby is ready.
        if let Some(session) = &mut self.net {
            session.disconnect();
        }
        self.net = None;
        // Release GPU backends while the event loop's display connection is alive.
        self.input.stop();
        if self.world.0.is_some() {
            self.finish_replay_recording("exit");
        }
        // Graphics changes still settling are saved before the game closes.
        self.settle_graphics(true);
        self.save_preferences();
        if let Some(recording) = &mut self.input_recording {
            use std::io::Write;
            if let Err(error) = recording.flush() {
                self.error = Some(error.into());
            }
        }
        if self.world.0.is_some()
            && let Err(e) = self.finish_combat_tape()
        {
            self.error = Some(e);
        }
        self.renderer = None;
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.screen == Screen::Main && self.updates.poll() {
            let version = self
                .updates
                .available
                .as_ref()
                .map(|r| r.version.to_string());
            self.menu.update_link(version.as_deref());
            if let Some(renderer) = &self.renderer {
                renderer.window.request_redraw();
            }
        }
        // The Graphics screen's changes apply once they settle; this also
        // keeps its window size current after a resize.
        self.settle_graphics(false);
        // A session outside flight is pumped here, on its own wake, not at
        // the start of a redraw (net::pace).
        self.pump_in_wait(event_loop);
        // Measure event-loop service, not the arrival of window events: a
        // minimized or untouched window can legitimately receive none.
        let preparing = self.loading.is_some()
            || (self.net_built.is_some() && self.network_loading_stage.is_some())
            || self.pending_join.is_some()
            || self.pending_host.is_some()
            || self.net.as_ref().is_some_and(|s| {
                s.client.is_building()
                    || matches!(
                        s.client.phase(),
                        tore_session::ClientPhase::Connecting | tore_session::ClientPhase::Loading
                    )
            });
        let now = Instant::now();
        if let Some(last) = self.loading_heartbeat.take() {
            let gap = now - last;
            self.loading_max_gap = self.loading_max_gap.max(gap);
            if gap > Duration::from_millis(100) {
                log::info!(
                    "loading event-loop service gap: {:.3} ms",
                    gap.as_secs_f64() * 1000.
                );
            }
            if !preparing {
                log::info!(
                    "loading maximum event-loop service gap: {:.3} ms",
                    self.loading_max_gap.as_secs_f64() * 1000.
                );
            }
        }
        if preparing {
            self.loading_heartbeat = Some(now);
        } else {
            self.loading_max_gap = Duration::ZERO;
            self.network_loading_stage = None;
        }

        if self.pending_host.is_some() {
            let ready = net::session::content_ready(&self.theater_resources);
            if ready {
                let (options, lobby) = self.pending_host.take().unwrap();
                let name = options.name.clone();
                match self.begin_hosting(options, lobby) {
                    Ok(()) if lobby && self.net.is_some() => self.open_lobby(&name, true),
                    Ok(()) => {}
                    Err(error) => self.message(error),
                }
            }
            self.loading_wait(event_loop, ready);
            return;
        }

        if self.pending_join.is_some() {
            let ready = net::session::content_ready(&self.theater_resources);
            if ready {
                let (join, label) = self.pending_join.take().unwrap();
                self.start_join(join, &label);
            }
            self.loading_wait(event_loop, ready);
            return;
        }

        if let Some(job) = &self.loading {
            let mut finished = false;
            if let Some(result) = job.poll() {
                finished = true;
                let continuation = job.progress.clone();
                self.loading = None;
                match result {
                    Ok(loading::Prepared::Mission(parts)) => {
                        // The load that flew is kept for this mission.
                        if self.quick.draft.values[18] != 0
                            && let Some(memory) = &mut self.quick_loadouts
                            && let Some(o) = &self.quick.ordnance
                        {
                            memory.flown(
                                &self.quick.mission_fingerprint(),
                                mission::LoadoutSpec::of(&o.loadout),
                            );
                        }
                        let (built, view, scenery, graphics, recording, brief_sentences) = *parts;
                        self.mission_brief_sentences = Some(brief_sentences);
                        self.flight_ui.map.brief = None;
                        self.prepared_recording = Some(recording);
                        self.prepared_scenery = Some((scenery, graphics));
                        self.begin_flight(event_loop, Some((built, view)));
                        self.finish_creator_launch(event_loop);
                    }
                    Ok(loading::Prepared::Selection(parts)) => {
                        self.hornet = *parts;
                        // The loadout preview needs only aircraft art. Terrain,
                        // combat and scenery are built once for the final mission.
                        if self.quick.draft.values[18] != 0
                            && let Some(renderer) = &mut self.renderer
                        {
                            renderer.prepare_aircraft(&self.hornet);
                        }
                        self.mission_selection_ready = true;
                        self.action(event_loop, Action::Mission);
                        if self.loading.is_none() && self.creator_loading_view.is_some() {
                            self.finish_creator_launch(event_loop);
                        }
                    }
                    Ok(loading::Prepared::Restart(parts)) => {
                        let (world, restarted, recording) = *parts;
                        self.prepared_recording = Some(recording);
                        self.world.0 = Some(world);
                        self.prepared_restart = Some(restarted);
                        self.begin_flight(event_loop, None);
                    }
                    Ok(loading::Prepared::NetRebuilt(built)) => {
                        self.network_loading_stage = Some(continuation);
                        if self.net.is_some() {
                            self.net_built = Some(*built);
                        }
                    }
                    Ok(loading::Prepared::Net(parts)) => {
                        if self.net.is_some() {
                            self.finish_session_loading(*parts);
                        }
                    }
                    Ok(loading::Prepared::Replay(viewer, graphics)) => {
                        let retired = if let (Some(renderer), Some(graphics)) =
                            (&mut self.renderer, graphics)
                        {
                            renderer.install_loading(*graphics)
                        } else {
                            None
                        };
                        if let Some(handle) = retired {
                            self.shelve_background(handle);
                        }
                        self.start_replay(*viewer, None);
                    }
                    Err(message) => {
                        log::warn!("Loading failed: {message}");
                        if self.net.is_some() {
                            self.net_ending = Some(message);
                            self.end_session(event_loop);
                            return;
                        }
                        if self.world.0.is_none() || self.creator_loading_view.is_some() {
                            self.error = Some(message.into());
                            event_loop.exit();
                            return;
                        }
                        if let Some(ordnance) = &mut self.quick.ordnance {
                            ordnance.message = Some(message.clone());
                        }
                        self.menu.state.toast = Some((message, Instant::now()));
                    }
                }
                self.frame_time = Instant::now();
            }
            // The photo and text are drawn at 20 Hz; the job is still
            // polled every 16 ms so a finished load starts at once.
            self.loading_wait(event_loop, finished);
            return;
        }
        self.run_script(event_loop);
        if self.loading.is_some() || self.pending_join.is_some() || self.pending_host.is_some() {
            self.loading_wait(event_loop, true);
            return;
        }
        // The replay viewer shows instead of the Replays screen, which stays
        // open underneath and reads the list again when it is back.
        if self.screen != Screen::Main
            && let Some(screen) = &mut self.replays_screen
        {
            screen.covered();
        }
        let paused = self.input_paused();
        self.input.context(paused, self.focused);
        // Controllers are polled every 8 ms in flight, the replay viewer and
        // the controls screen; the menus poll less often, and so does an
        // unfocused or hidden window, whose bindings ignore the controllers
        // anyway (a multiplayer flight keeps flying: its session is not
        // paced by input polls).
        let window = self.pace_window();
        self.input.set_pace(
            (window.automated || (window.focused && !window.hidden))
                && (self.screen == Screen::Replay
                    || self.controls.is_some()
                    || (self.screen == Screen::Flight && !paused)),
        );
        let cockpit = self
            .world
            .cockpits
            .get(OWN)
            .filter(|_| self.screen == Screen::Flight);
        let available =
            |axis| cockpit.is_some_and(|cockpit| cockpit.flight.flight_axis_available(axis));
        let gunship = cockpit
            .and_then(|cockpit| self.world.combat.state.ownship(cockpit.plane.0))
            .is_some_and(|own| own.gunship.is_some());
        self.input.map_navigation = self.flight_ui.map.open || self.waiting_to_respawn();
        self.input.aircraft_controls(
            available(tore_input::FlightAxis::VectorPitch),
            available(tore_input::FlightAxis::Conversion),
            available(tore_input::FlightAxis::Collective),
            gunship,
        );
        if Instant::now() >= self.input.next_poll {
            let (actions, lost, warnings) = self.input.poll();
            let mut changed = lost || !actions.is_empty();
            let mut captured = false;
            if let Some(editor) = &mut self.controls {
                let status = self.input.head_status();
                changed |= status != editor.head_status;
                editor.head_status = status;
                if !editor.capturing() {
                    editor.devices = self.input.devices.values().cloned().collect();
                } else {
                    editor
                        .devices
                        .retain(|d| self.input.devices.contains_key(&d.id));
                    for d in self.input.devices.values() {
                        if !editor.devices.iter().any(|old| old.id == d.id) {
                            editor.devices.push(d.clone());
                        }
                    }
                }
                for event in &self.input.observed {
                    captured |= editor.observe(event);
                }
                changed |= captured;
                if changed && let Some(renderer) = &self.renderer {
                    renderer.window.request_redraw();
                }
            }
            for warning in warnings {
                log::warn!("Input: {warning}");
            }
            if self.controls.is_none()
                && let Some(replay) = &mut self.replay
            {
                if lost {
                    replay.viewer.release();
                }
                replay
                    .viewer
                    .drone_devices(|id| self.input.devices.contains_key(id));
                for event in &self.input.observed {
                    if let Some(device) = self.input.devices.get(&event.device) {
                        replay
                            .viewer
                            .drone_event(crate::input::normalize(device, event.clone()));
                    }
                }
            }
            if lost && self.screen == Screen::Flight {
                self.net_controller_lost();
                self.flight_ui.pause_for_focus();
                self.flight_ui.message(if self.flight_ui.session {
                    "Active controller disconnected; controls are neutral"
                } else {
                    "Active controller disconnected; resume explicitly"
                });
                self.camera.keys.clear();
                self.release_trigger();
                self.input.context(true, self.focused);
                self.flight_clock.remainder = 0.;
                self.frame_time = Instant::now();
            } else {
                for action in actions {
                    if let Some(form) = &mut self.feedback {
                        if let tore_input::Action::Ui(name) = &action {
                            let result = form.controller(name);
                            self.feedback_result(result);
                        }
                        continue;
                    }
                    // Controller menu buttons navigate the controls screen,
                    // except in the poll that completed a capture with them.
                    if let Some(editor) = &mut self.controls {
                        let key = match &action {
                            tore_input::Action::Ui(name) => match name.as_str() {
                                "menu-up" => Some("ArrowUp"),
                                "menu-down" => Some("ArrowDown"),
                                "menu-left" => Some("ArrowLeft"),
                                "menu-right" => Some("ArrowRight"),
                                "menu-accept" => Some("Enter"),
                                "menu-back" => Some("Escape"),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(key) = key.filter(|_| !captured && !editor.capturing()) {
                            let result = editor.key(key, false, false, false);
                            let action = self.controls_result(result);
                            self.action(event_loop, action);
                        }
                        continue;
                    }
                    let was_frozen = self.flight_ui.frozen();
                    let was_respawn = self.respawn_open();
                    if self.replay.is_some() && self.controls.is_none() {
                        continue;
                    }
                    let result = self.input_action(action);
                    self.action(event_loop, result);
                    if self.loading.is_some() {
                        return;
                    }
                    if was_frozen != self.flight_ui.frozen() || was_respawn != self.respawn_open() {
                        self.camera.keys.clear();
                        self.release_trigger();
                        self.flight_clock.remainder = 0.;
                        let own = &mut self.world.cockpits[OWN];
                        own.previous_flight.clone_from(&own.flight);
                        self.frame_time = Instant::now();
                        self.input.context(self.input_paused(), self.focused);
                        break; // Remaining events belong to the previous context.
                    }
                }
            }
            if changed && let Some(renderer) = &self.renderer {
                renderer.window.request_redraw();
            }
            if changed {
                self.menu_pace.activity(Instant::now());
            }
        }
        // A menu with a session or a network screen is drawn when the pump
        // brought news, after input, and on a slow heartbeat (net::pace).
        // The frame-rate counter draws every screen continuously instead.
        let paced = self.menu_paced();
        if paced
            && !self.counter_shown()
            && self
                .menu_pace
                .redraw_due(Instant::now(), self.net_preparing())
            && let Some(renderer) = &self.renderer
        {
            renderer.window.request_redraw();
        }
        // A session takes its turns between frames too: drawing may wait (a
        // capped rate, an unfocused window, or a hidden one, which with
        // V-sync on Wayland gets no redraws at all), the session's ticks,
        // controls and keepalives do not. Menus are pumped above on the menu
        // pace (net::pace), so this covers flight and the replay viewer.
        if !self.net_pumped_in_wait()
            && self.next_frame.is_none_or(|next| Instant::now() < next)
            && self
                .net
                .as_ref()
                .is_some_and(|session| session.next_wake() < Duration::from_micros(500))
        {
            self.net_tick(event_loop);
        }
        if self.next_frame.is_some_and(|next| Instant::now() >= next) {
            if let Some(renderer) = &self.renderer {
                renderer.window.request_redraw();
            }
            self.next_frame = None;
        }
        let mut next = self
            .next_frame
            .map_or(self.input.next_poll, |n| n.min(self.input.next_poll));
        if paced {
            next = next.min(self.menu_pace.next_due(
                Instant::now(),
                self.net_preparing(),
                self.net_pumped_in_wait(),
            ));
        } else if self.net_pumped_in_wait() {
            next = next.min(self.menu_pace.next_pump());
        } else if let Some(session) = &self.net {
            next = next.min(Instant::now() + session.next_wake());
        }
        if self.script.is_some() {
            next = next.min(Instant::now() + Duration::from_millis(10));
        }
        // Wake when the Graphics screen's pending changes are due.
        if let Some(at) = self.graphics_screen.as_ref().and_then(|e| e.apply_at()) {
            next = next.min(at);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }
}
/// The combat state for a run: the PT default load (or the range's), or the Load
/// Ordnance page's edits without the page: every store off (`none`), or every
/// external store off with the internal gun kept (`guns`).
fn build_combat(
    hornet: &aircraft::Airframe,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    live_fire: bool,
    stripped: Option<&str>,
) -> AppResult<combat::Combat> {
    let Some(choice) = stripped else {
        return combat::Combat::new(hornet, resources, live_fire);
    };
    let mut load = tore_sim::combat::loadout::Loadout::new(&hornet.profile, |name| {
        resources
            .get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("missing loadout resource {name}")))
    })?;
    if choice == "none" {
        load.quantities.fill(0);
        load.clear_tanks()?;
    } else {
        load.restrict_to_guns();
    }
    combat::Combat::with_loadout(hornet, &load)
}

/// The map-edge rule for a probe's flight: lost 105 nautical miles beyond the
/// theater. (The game host also warns the player from 100; see `terrain.rs`.)
fn apply_edge_loss(flight: &mut flight::State, world: &terrain::Terrain) {
    if flight.crashed {
        return;
    }
    let [x, _, z] = flight.position;
    if world.edge_distance_nm(x, z) >= terrain::EDGE_DESTROY_NM {
        flight
            .systems
            .destroy(tore_sim::aircraft_systems::LossCause::OutOfBounds);
        flight.crashed = true;
    }
}

/// The `--maneuver devices` script: each device is set down one after another,
/// then up again, so the run shows every travel.
fn device_schedule(tick: u64) -> Option<Vec<flight::PilotCommand>> {
    use flight::{PilotCommand::Set, Switch::*};
    let (device, down) = match tick {
        120 => (Gear, true),
        240 => (Flaps, true),
        360 => (Airbrake, true),
        480 => (Hook, true),
        1800 => (Gear, false),
        1920 => (Flaps, false),
        2040 => (Airbrake, false),
        2160 => (Hook, false),
        _ => return None,
    };
    Some(vec![Set(device, down)])
}

/// The `--flight-cheat` names, for headless probes and live-fire captures.
const PROBE_CHEATS: [&str; 9] = [
    "extra-g",
    "no-g-effects",
    "no-spins",
    "no-crashes",
    "unlimited-fuel",
    "unlimited-ammo",
    "invulnerable",
    "realistic-damage",
    "easy-physics",
];

fn apply_probe_cheat(cheats: &mut tore_sim::cheats::Cheats, name: &str) {
    use tore_sim::cheats::Damage;
    match name {
        "extra-g" => cheats.extra_g = true,
        "no-g-effects" => cheats.no_g_effects = true,
        "no-spins" => cheats.no_spins = true,
        "no-crashes" => cheats.no_crashes = true,
        "unlimited-fuel" => cheats.unlimited_fuel = true,
        "unlimited-ammo" => cheats.unlimited_ammo = true,
        "easy-physics" => cheats.easy_physics = true,
        "invulnerable" => cheats.damage = Damage::Invulnerable,
        _ => cheats.damage = Damage::Realistic,
    }
}

/// What the headless AI probe scripts for the human leader
/// (`--maneuver takeoff`, `--probe-wing-size`, `--probe-wing-order`,
/// `--probe-player-home`, `--probe-lose-player`, `--probe-attack`,
/// `--probe-player-lock`). Development harness only.
#[derive(Clone, Debug, Default)]
struct ProbeScript {
    /// `--probe-render DIR` with `--probe-render-at TICK,...`,
    /// `--probe-render-plane ID` and `--probe-render-view chase|wide|ground`
    /// (slice H2): offscreen frames of the world around one AI plane.
    render: Option<std::path::PathBuf>,
    render_at: Vec<u64>,
    render_plane: u32,
    render_view: String,
    enemy_aircraft: Option<tore_formats::aircraft::AircraftId>,
    enemy_skill: Option<usize>,
    geometry: ProbeGeometry,
    guns: bool,
    ai_guns_only: bool,
    researched: bool,
    /// The mission setting for the AI's flight model
    /// (`--probe-ai-flight-model`).
    ai_flight_model: ai_wings::AiFlightModel,
    threats: Vec<(u64, ProbeThreat)>,
    matrix: Option<PathBuf>,
    /// Take off from the ground start, climb and cruise on the autopilot.
    takeoff: bool,
    /// Aircraft in the player's wing, the player included.
    wing_size: Option<usize>,
    /// Reproduce an isolated player wing without the usual probe opponents.
    wing_only: bool,
    /// `--probe-fight FRIENDLY:ENEMY`: total aircraft per side, 1..15 each
    /// (the player counts as one friendly), filled five to a wing.
    fight: Option<(usize, usize)>,
    /// Aircraft for the friendly AI wings, overriding the creator default.
    friendly_aircraft: Option<tore_formats::aircraft::AircraftId>,
    /// Wing orders at a tick, to all wingmen or to one member of the wing
    /// (`TICK:ORDER@MEMBER`, the leader being member 0).
    orders: Vec<(u64, tore_sim::ai::wing::PlayerOrder, Option<u8>)>,
    /// `--probe-player-lock TICK:ID`: the player designates aircraft `ID` at
    /// this tick, and the sensors acquire it as they would for a human.
    player_locks: Vec<(u64, u32)>,
    /// `--probe-blind-wing`: the player's wingmen have no sensors of their
    /// own, so only the flight data link can show them an enemy.
    blind_wing: bool,
    /// Print a `data link:` line for each change of the flight data link's
    /// picture. Set by `--probe-data-link` and by `--probe-player-lock`, so
    /// the output of every other probe is unchanged.
    data_link: bool,
    /// `--probe-wing-route`: the player's wing's waypoints, as nautical-mile
    /// offsets east and north of the player's start and an altitude in feet.
    /// Quick Mission has no mission route, so this stands in for one.
    wing_route: Vec<[f64; 3]>,
    /// Ticks between `AI fuel:` lines (`--probe-fuel-trace`), 0 for none.
    fuel_trace_ticks: u64,
    /// Ticks between `AI rotor:` lines (`--probe-rotor-trace`), 0 for none:
    /// each AI helicopter and V-22's flight state and what the rotor
    /// adapter commanded (slice HF1).
    rotor_trace_ticks: u64,
    /// `--probe-fuel POUNDS`: every AI aircraft starts with this much
    /// internal fuel (slice H3), so a return on low fuel can be probed
    /// without a half-hour mission.
    fuel_lbs: Option<f64>,
    /// `--probe-lose-player TICK`: the player's aircraft crashes at this tick,
    /// so that a probe can test what the wing does without its human leader.
    /// Test harness only.
    lose_player: Option<u64>,
    /// `--probe-group GROUP:CHOICE[:survive]`: a creator group objective
    /// (group 1..6, friendly then enemy; choice index in that group's
    /// objective list) and whether the group must survive.
    groups: Vec<(usize, usize, bool)>,
    /// From the first tick to the second the player flies gear down over the
    /// departure airfield, the configuration that gives it landing priority.
    home: Option<(u64, u64)>,
    /// Ticks between trace lines for the player's wing, 0 for none.
    trace_ticks: u64,
    /// The leader fires its own weapons from this tick.
    attack: Option<ProbeAttack>,
    /// Manual AIM-120 shot at tick 60, empty sky or a target 0.2 nm ahead.
    missile_release: Option<u8>,
    /// `--probe-pavement`: print how many ground ticks of the AI aircraft at
    /// a redrawn airport were off its drawn pavement (an off-pavement tick is
    /// an anomaly either way).
    pavement: bool,
    /// `--probe-track FILE`: write the AI aircraft's ground tracks at
    /// redrawn airports (see `probe_invariants::ProbeInvariants::track`).
    track: Option<PathBuf>,
    /// `--probe-redfor`: the scripted leader and its wing fly for Redfor,
    /// as Enemy Wing 1 of an open mission whose lead the probe's seat takes
    /// by the handoff a networked Redfor player uses, parked at the Redfor
    /// field `--ground-start` names (slice CL-R).
    redfor: bool,
    /// `--ai-observe-hz N` and `--ai-lean-sensors [on|off]` (slices S7b and
    /// S9): the AI test settings, 60 Hz with lean sensors unless given. They also start a windowed
    /// flight's Debug menu with these values.
    ai_test: tore_sim::ai::test_settings::AiTestSettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProbeGeometry {
    #[default]
    Head,
    Rear,
    Side,
}

#[derive(Default)]
struct ProbeEncounter {
    visual: Option<u64>,
    engage: Option<u64>,
    defense: Option<u64>,
    bank: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeThreat {
    Fault(usize),
    Hit,
    Gun,
    Aaa,
}

/// `--record-mission PATH` on an AI probe, with `--verify-render`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProbeRecord {
    /// The new recording's path; the probe refuses to overwrite a file.
    path: PathBuf,
    /// After the run, rebuild every tick from the file and compare it with
    /// the live picture, printing one summary line.
    verify: bool,
}

/// The mission's human pilots as a recording's roster registers them: the
/// plane `seat` flies, `You`, and then every other human-flown plane.
fn recorded_humans(
    world: &world::World,
    seat: seats::SeatId,
    aircraft_name: &str,
) -> (replay::recorder::Human, Vec<replay::recorder::Human>) {
    let numbered = world.setup.mission.is_some();
    let human = |plane: &seats::Plane, label: String| replay::recorder::Human {
        id: plane.id.0,
        label,
        name: aircraft_name.to_owned(),
        side: match plane.slot.wing.side {
            tore_sim::ai::launch::Side::Friendly => tore_replay::Side::Friendly,
            tore_sim::ai::launch::Side::Enemy => tore_replay::Side::Enemy,
        },
        wing: if numbered {
            u16::from(plane.slot.wing.index) + 1
        } else {
            0
        },
        member: if numbered {
            u16::from(plane.slot.member) + 1
        } else {
            0
        },
    };
    let mut own = None;
    let mut others = Vec::new();
    for plane in world.roster.planes() {
        match plane.pilot {
            seats::Pilot::Human(who) if who == seat => own = Some(human(plane, "You".into())),
            seats::Pilot::Human(who) => {
                others.push(human(plane, format!("Seat {}", u16::from(who.0) + 1)))
            }
            seats::Pilot::Ai | seats::Pilot::Lost => {}
        }
    }
    (
        own.unwrap_or_else(|| replay::recorder::Human::single_player(aircraft_name, numbered)),
        others,
    )
}

/// The human-flown planes other than plane `own`, for the
/// recorder's tick. Only the app's own seat sends controls, so the others' read
/// as idle.
fn other_crews<'a>(
    cockpits: &'a [world::Cockpit],
    own: seats::PlaneId,
    idle: &'a flight::PilotInput,
) -> Vec<replay::recorder::Crewed<'a>> {
    cockpits
        .iter()
        .filter(|cockpit| cockpit.plane != own)
        .map(|cockpit| replay::recorder::Crewed {
            plane: cockpit.plane.0,
            flight: &cockpit.flight,
            pilot: idle,
        })
        .collect()
}

/// A recording's footer: why the flight ended, and for a mission the
/// debrief's outcome, the player's fate and kills.
fn replay_footer(
    combat: &combat::Combat,
    player: u32,
    report: Option<&debrief::Report>,
    reason: &str,
) -> tore_replay::Footer {
    let mut result = vec![("end".to_owned(), reason.to_owned())];
    match report {
        Some(report) => {
            result.push((
                "outcome".into(),
                format!("{:?}", report.outcome).to_lowercase(),
            ));
            result.push((
                "player".into(),
                format!("{:?}", report.player.status).to_lowercase(),
            ));
            result.push((
                "kills".into(),
                report.player.kills.iter().sum::<u32>().to_string(),
            ));
        }
        None => result.push((
            "kills".into(),
            combat
                .state
                .ownship(player)
                .map_or(0, |own| own.kills)
                .to_string(),
        )),
    }
    tore_replay::Footer {
        end_tick: combat.state.tick(),
        result,
    }
}

/// `--probe-attack TICK[:REPEAT_SECONDS]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProbeAttack {
    /// First tick on which the leader looks for a shot.
    from: u64,
    /// Ticks from one shot to the next attack, 0 for a single attack.
    repeat: u64,
}

impl ProbeScript {
    /// `TICK:bug-out`, `TICK:land-selected`, `TICK:attack-on-contact` or
    /// `TICK:engage-my-target`, each with an optional `@MEMBER` that gives the
    /// order to that one member of the wing (the leader is member 0, so the
    /// first wingman is 1), as Alt+Shift+1 to 4 does.
    fn parse_order(text: &str) -> AppResult<(u64, tore_sim::ai::wing::PlayerOrder, Option<u8>)> {
        use tore_sim::ai::wing::PlayerOrder;
        let usage = "--probe-wing-order needs TICK:bug-out, TICK:land-selected, TICK:attack-on-contact, TICK:engage-my-target or TICK:sort, each optionally followed by @MEMBER";
        let (tick, order) = text.split_once(':').ok_or(usage)?;
        let (order, member) = match order.split_once('@') {
            Some((order, member)) => {
                let member: u8 = member.parse().map_err(|_| usage)?;
                if !(1..=4).contains(&member) {
                    return Err("--probe-wing-order @MEMBER needs 1..4".into());
                }
                (order, Some(member))
            }
            None => (order, None),
        };
        let order = match order {
            "bug-out" => PlayerOrder::BugOut,
            "land-selected" => PlayerOrder::LandAtSelected,
            "attack-on-contact" => PlayerOrder::AttackOnContact,
            "engage-my-target" => PlayerOrder::EngageMyTarget,
            "sort" => PlayerOrder::Sort,
            _ => return Err(usage.into()),
        };
        Ok((option_number("--probe-wing-order", tick)?, order, member))
    }

    /// `TICK:ID`: the player designates aircraft `ID` at `TICK`.
    fn parse_player_lock(text: &str) -> AppResult<(u64, u32)> {
        let usage = "--probe-player-lock needs TICK:ID";
        let (tick, id) = text.split_once(':').ok_or(usage)?;
        Ok((
            option_number("--probe-player-lock", tick)?,
            id.parse().map_err(|_| usage)?,
        ))
    }

    /// `TICK` for a single attack, or `TICK:REPEAT_SECONDS`.
    fn parse_attack(text: &str) -> AppResult<ProbeAttack> {
        let usage = "--probe-attack needs TICK or TICK:REPEAT_SECONDS";
        let (tick, repeat) = match text.split_once(':') {
            Some((tick, seconds)) => {
                let seconds: f64 = seconds.parse().map_err(|_| usage)?;
                if !(seconds > 0. && seconds <= 3600.) {
                    return Err("--probe-attack needs 0..3600 repeat seconds".into());
                }
                (tick, (seconds * 120.).round().max(1.) as u64)
            }
            None => (text, 0),
        };
        Ok(ProbeAttack {
            from: tick.parse().map_err(|_| usage)?,
            repeat,
        })
    }
}

/// The scripted human leader of the AI probe. `fitted` test harness (agent
/// decision, 2026-09-23, extended 2026-09-30), not game behaviour: the same
/// full-power, 0.35 pitch rotation as `--maneuver takeoff` until 50 ft above
/// the ground, then gear up, flaps up once the clean wing carries the aircraft,
/// and a 10 degree nose-up hold to 3,000 ft above the ground, where the
/// player's own heading-and-altitude autopilot takes over. It flies as the AI's
/// own departure does, so that every aircraft reaches a safe cruise: it steers
/// back to the runway line if the roll drifts, eases its pitch to keep 1 G
/// flight above the aircraft's own minimum speed for the current flaps, pulls up
/// to clear terrain ahead, and climbs again from the cruise if the ground
/// ahead rises to meet it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbePhase {
    Roll,
    /// Airborne with the gear up; `flaps_up` once the flaps have been raised.
    Climb {
        flaps_up: bool,
    },
    Cruise,
    Home,
    Away,
}

/// The scripted leader: its phase and its last look at the terrain ahead.
#[derive(Clone, Debug)]
struct ProbePilot {
    phase: ProbePhase,
    terrain: ProbeTerrain,
    /// A helicopter or V-22 leader's controls (slice H2): the AI's own
    /// rotor laws fly the scripted liftoff and cruise.
    rotor: tore_sim::ai::rotor_adapter::RotorAdapter,
    /// Where a rotorcraft leader lifted off, world X/Z.
    lift_point: Option<[f64; 2]>,
}

/// A rotorcraft leader's scripted flight (slice H2): straight up to this
/// wheel height over its spot at 25 ft/s, then on along the runway heading
/// at [`PROBE_ROTOR_KT`], [`PROBE_ROTOR_AGL_FT`] over the highest ground
/// ahead.
const PROBE_ROTOR_LIFT_FT: f64 = 500.;
const PROBE_ROTOR_KT: f64 = 100.;
const PROBE_ROTOR_AGL_FT: f64 = 1_500.;

/// What the terrain along the heading ahead asks of the scripted leader.
#[derive(Clone, Copy, Debug, Default)]
struct ProbeTerrain {
    /// Tick of the last look, so that it is taken a few times a second.
    looked_at: Option<u64>,
    /// The steepest flight path, in degrees above the horizon, that clears
    /// every sample by [`PROBE_TERRAIN_CLEARANCE_FT`].
    need_deg: f64,
    /// The highest surface sampled, feet above sea level.
    highest_ft: f64,
}

/// The scripted gear-up for the `takeoff-gear-early` (at 80 knots with the
/// wheels still down, too slow to fly) and `takeoff-gear-airborne` (once 50 ft
/// above the runway) headless takeoffs. Returns the input for that tick.
/// The pilot of `--maneuver settle:SINK`: the collective (the throttle on a
/// vectoring jet) held to a steady sink of `sink` ft/s until the wheels are
/// down, then lowered so it sits.
fn settle_keys(state: &flight::State, sink: f64) -> flight::PilotInput {
    let on_ground = state.research.as_ref().is_some_and(|r| r.on_ground);
    let error = -sink - state.vertical_speed;
    let mut keys = flight::PilotInput::default();
    let jet = state
        .model()
        .powered_lift()
        .is_some_and(|lift| lift.kind == tore_sim::models::variety::LiftKind::VectorJet);
    if jet {
        let throttle = if on_ground {
            0.
        } else {
            state.throttle + flight::DT * 0.05 * error
        };
        keys.throttle = Some(throttle.clamp(0., 1.));
    } else {
        let collective = if on_ground {
            0.
        } else {
            state.lift_controls.collective + flight::DT * 0.05 * error
        };
        keys.collective = Some(collective.clamp(0., 1.));
    }
    keys
}

fn gear_pull(
    maneuver: &str,
    state: &flight::State,
    pulled: &mut bool,
    keys: &flight::PilotInput,
    world: &Option<terrain::Terrain>,
) -> Option<flight::PilotInput> {
    if *pulled {
        return None;
    }
    let due = match maneuver {
        "takeoff-gear-early" => state.speed >= 80. * 1.68781,
        "takeoff-gear-airborne" => world.as_ref().is_some_and(|w| {
            state.position[1] - w.surface(state.position[0], state.position[2]).height > 50.
        }),
        _ => false,
    };
    if !due {
        return None;
    }
    *pulled = true;
    let mut input = keys.clone();
    input.commands = vec![flight::PilotCommand::Set(flight::Switch::Gear, false)];
    Some(input)
}

/// Height above the ground at which the scripted leader cleans up.
const PROBE_CLEAN_AGL_FT: f64 = 50.;
/// Nose-up attitude the scripted leader holds in the climb.
const PROBE_CLIMB_PITCH_DEG: f64 = 10.;
/// Height above the ground at which the scripted leader levels off.
const PROBE_CRUISE_AGL_FT: f64 = 3_000.;
/// The steepest nose-up attitude the scripted leader asks for over terrain.
const PROBE_MAX_CLIMB_PITCH_DEG: f64 = 25.;
/// How far ahead, in seconds of flight, the scripted leader reads the terrain.
const PROBE_TERRAIN_LOOKAHEAD_S: f64 = 45.;
/// The least distance, in feet, it reads ahead at any speed.
const PROBE_TERRAIN_MIN_REACH_FT: f64 = 12_000.;
/// Spacing of the terrain samples along the heading, feet.
const PROBE_TERRAIN_STEP_FT: f64 = 500.;
/// Ticks between two looks at the terrain (four a second).
const PROBE_TERRAIN_EVERY_TICKS: u64 = 30;
/// Clearance the climb aims to keep over the highest ground ahead, feet.
const PROBE_TERRAIN_CLEARANCE_FT: f64 = 500.;
/// The cruise altitude must be this far over the ground ahead to level off.
const PROBE_LEVEL_OFF_MARGIN_FT: f64 = 1_000.;
/// In the cruise, ground closer than this under the aircraft ahead means climb.
const PROBE_RECLIMB_MARGIN_FT: f64 = 600.;
/// Speed over the aircraft's 1 G minimum at which the climb holds its full
/// pitch; below it the pitch eases off to level flight at
/// [`PROBE_LEVEL_SPEED_FLOOR`].
const PROBE_SPEED_MARGIN: f64 = 1.12;
const PROBE_LEVEL_SPEED_FLOOR: f64 = 1.02;
/// Speed over the clean wing's 1 G minimum at which the flaps come up.
const PROBE_FLAPS_UP_MARGIN: f64 = 1.05;
/// The roll steers back to the runway line when it is this far off the
/// heading, degrees, or off the line, feet.
const PROBE_ROLL_HEADING_DEADBAND_DEG: f64 = 2.;
const PROBE_ROLL_LINE_DEADBAND_FT: f64 = 15.;
/// The nosewheel's steering authority, as the AI's takeoff roll assumes it.
const PROBE_NOSEWHEEL_YAW_RATE_RAD_S: f64 = 0.3;
const PROBE_NOSEWHEEL_FULL_SPEED_FPS: f64 = 40.;

/// The rudder for a takeoff roll, from the heading error (degrees, runway
/// minus nose, positive turns right), the distance right of the runway line
/// (feet) and the speed (feet per second). Zero while the roll is within
/// [`PROBE_ROLL_HEADING_DEADBAND_DEG`] and [`PROBE_ROLL_LINE_DEADBAND_FT`], so a
/// roll that stays true is untouched. Otherwise the heading rule and nosewheel
/// authority the AI's own roll uses: aim up to 5 degrees back toward the line.
fn roll_rudder(heading_error_deg: f64, cross_ft: f64, speed_fps: f64) -> f64 {
    let wrap = |degrees: f64| (degrees + 180.).rem_euclid(360.) - 180.;
    let off = wrap(heading_error_deg);
    if off.abs() <= PROBE_ROLL_HEADING_DEADBAND_DEG && cross_ft.abs() <= PROBE_ROLL_LINE_DEADBAND_FT
    {
        return 0.;
    }
    let toward_line = (-cross_ft).atan2(400.).to_degrees().clamp(-5., 5.);
    let error = wrap(off + toward_line);
    let limit = tore_sim::ai::steering::GROUND_TURN_RATE_FLOOR_DEG_PER_S;
    let rate = error.clamp(-limit, limit).to_radians();
    let authority =
        PROBE_NOSEWHEEL_YAW_RATE_RAD_S * (speed_fps / PROBE_NOSEWHEEL_FULL_SPEED_FPS).clamp(0., 1.);
    (rate / authority.max(0.05)).clamp(-1., 1.)
}

/// The nose-up attitude of the climb, degrees: 10, raised to clear ground that
/// needs `need_deg` of flight path (plus the angle of attack and two degrees),
/// to at most 25, and eased toward level as `speed_ratio` (speed over the
/// aircraft's 1 G minimum with its present flaps) falls from
/// [`PROBE_SPEED_MARGIN`] to [`PROBE_LEVEL_SPEED_FLOOR`].
fn climb_pitch_for(need_deg: f64, angle_of_attack_deg: f64, speed_ratio: f64) -> f64 {
    let target = PROBE_CLIMB_PITCH_DEG
        .max(need_deg + angle_of_attack_deg + 2.)
        .min(PROBE_MAX_CLIMB_PITCH_DEG);
    let ease = ((speed_ratio - PROBE_LEVEL_SPEED_FLOOR)
        / (PROBE_SPEED_MARGIN - PROBE_LEVEL_SPEED_FLOOR))
        .clamp(0., 1.);
    target * ease
}

impl ProbePilot {
    fn new() -> Self {
        Self {
            phase: ProbePhase::Roll,
            terrain: ProbeTerrain::default(),
            rotor: tore_sim::ai::rotor_adapter::RotorAdapter::new(),
            lift_point: None,
        }
    }

    /// A helicopter or V-22 leader (slice H2): the rotor adapter lifts it
    /// straight up, then flies it on along the runway heading. The wing
    /// orders and the rest of the script are as for an aeroplane.
    fn fly_rotor(
        &mut self,
        tick: u64,
        flight: &flight::State,
        keys: &mut flight::PilotInput,
        world: &terrain::Terrain,
        ground: Option<&mission_layout::GroundLayout>,
    ) {
        use tore_sim::ai::rotor_adapter::{HoverGuidance, RotorRequest};
        let [x, y, z] = flight.position;
        let here = world.surface(x, z);
        let clearance = flight.model().configuration().equipment.ground_clearance_ft;
        let wheel = y - here.height - clearance;
        let course = ground.map_or(flight.yaw, |g| g.heading).to_degrees();
        let point = *self.lift_point.get_or_insert([x, z]);
        let (input, _) = if self.phase == ProbePhase::Roll {
            if wheel >= PROBE_ROTOR_LIFT_FT - 25. {
                println!("t={tick} player: up {wheel:.0} ft, flying on");
                self.phase = ProbePhase::Cruise;
            }
            let hover = HoverGuidance {
                point,
                velocity_fps: [0.; 2],
                height_agl_ft: PROBE_ROTOR_LIFT_FT,
                heading_deg: course,
                max_climb_fps: 25.,
                max_descent_fps: 10.,
                max_speed_fps: 10.,
            };
            self.rotor.controls(
                flight,
                RotorRequest::Hover(hover),
                here.height,
                here.wind,
                30.,
            )
        } else {
            self.look(tick, flight, world);
            let wanted = self.terrain.highest_ft.max(here.height) + PROBE_ROTOR_AGL_FT;
            let path = ((wanted - y) / 3_000.).atan().to_degrees().clamp(-5., 10.);
            let intent = tore_sim::ai::rotor_probe::steer(course, path, PROBE_ROTOR_KT * 1.687_81);
            self.rotor.controls(
                flight,
                RotorRequest::Steer {
                    intent: &intent,
                    terrain_pitch_floor_deg: None,
                },
                here.height,
                here.wind,
                30.,
            )
        };
        let commands = std::mem::take(&mut keys.commands);
        *keys = input;
        keys.commands = commands;
    }

    /// Read the terrain along the heading, four times a second.
    fn look(&mut self, tick: u64, flight: &flight::State, world: &terrain::Terrain) {
        if self
            .terrain
            .looked_at
            .is_some_and(|at| tick < at + PROBE_TERRAIN_EVERY_TICKS)
        {
            return;
        }
        let [x, y, z] = flight.position;
        let [vx, _, vz] = flight.velocity;
        let along = vx.hypot(vz);
        // The track once it is moving; the nose before that.
        let (dx, dz) = if along > 100. {
            (vx / along, vz / along)
        } else {
            (flight.yaw.sin(), flight.yaw.cos())
        };
        let reach = (along * PROBE_TERRAIN_LOOKAHEAD_S).max(PROBE_TERRAIN_MIN_REACH_FT);
        let mut need = f64::MIN;
        let mut highest = f64::MIN;
        let mut d = PROBE_TERRAIN_STEP_FT;
        while d <= reach {
            let height = world.surface(x + dx * d, z + dz * d).height;
            highest = highest.max(height);
            need = need.max(((height + PROBE_TERRAIN_CLEARANCE_FT - y) / d).atan());
            d += PROBE_TERRAIN_STEP_FT;
        }
        self.terrain = ProbeTerrain {
            looked_at: Some(tick),
            need_deg: need.to_degrees().max(0.),
            highest_ft: highest,
        };
    }

    /// The rudder that brings a drifting takeoff roll back to the runway line,
    /// zero while it holds it.
    fn roll_yaw(flight: &flight::State, ground: &mission_layout::GroundLayout) -> f64 {
        let course = ground.heading;
        let [x, _, z] = flight.position;
        let start = ground
            .slots
            .first()
            .copied()
            .unwrap_or(ground.runway.center);
        roll_rudder(
            course.to_degrees() - flight.yaw.to_degrees(),
            (x - start[0]) * course.cos() - (z - start[2]) * course.sin(),
            flight
                .speed
                .max(flight.velocity[0].hypot(flight.velocity[2])),
        )
    }

    /// The pitch attitude the climb asks for: 10 degrees, more where the ground
    /// ahead needs it, less where the speed is short of the aircraft's own
    /// 1 G minimum with its present flaps.
    fn climb_pitch_deg(&self, flight: &flight::State) -> f64 {
        let [vx, vy, vz] = flight.velocity;
        let path = vy.atan2(vx.hypot(vz)).to_degrees();
        let minimum = flight.minimum_level_speed(flight.position[1], flight.flaps);
        climb_pitch_for(
            self.terrain.need_deg,
            flight.pitch.to_degrees() - path,
            if minimum > 0. {
                flight.speed / minimum
            } else {
                f64::INFINITY
            },
        )
    }

    fn fly(
        &mut self,
        tick: u64,
        flight: &mut flight::State,
        keys: &mut flight::PilotInput,
        world: &terrain::Terrain,
        ground: Option<&mission_layout::GroundLayout>,
        script: &ProbeScript,
    ) {
        use flight::{PilotCommand::*, Switch};
        if tore_sim::ai::rotor_adapter::flies(flight) {
            self.fly_rotor(tick, flight, keys, world, ground);
            return;
        }
        let [x, y, z] = flight.position;
        let agl = y - world.surface(x, z).height;
        if !matches!(self.phase, ProbePhase::Roll | ProbePhase::Home) {
            self.look(tick, flight, world);
        }
        match self.phase {
            ProbePhase::Roll => {
                keys.pitch = 0.35;
                if let Some(ground) = ground {
                    keys.yaw = Self::roll_yaw(flight, ground);
                }
                if agl > PROBE_CLEAN_AGL_FT {
                    // The flaps wait until the clean wing carries the aircraft.
                    let clean = flight.minimum_level_speed(y, 0.) * PROBE_FLAPS_UP_MARGIN;
                    let flaps_up = flight.speed >= clean;
                    keys.commands = vec![Set(Switch::Gear, false)];
                    if flaps_up {
                        keys.commands.push(Set(Switch::Flaps, false));
                        println!("t={tick} player: airborne, gear and flaps up");
                    } else {
                        println!("t={tick} player: airborne, gear up, flaps held for speed");
                    }
                    self.phase = ProbePhase::Climb { flaps_up };
                }
            }
            ProbePhase::Climb { flaps_up } => {
                if !flaps_up
                    && flight.speed >= flight.minimum_level_speed(y, 0.) * PROBE_FLAPS_UP_MARGIN
                {
                    keys.commands = vec![Set(Switch::Flaps, false)];
                    self.phase = ProbePhase::Climb { flaps_up: true };
                    println!("t={tick} player: flaps up");
                }
                let error = self.climb_pitch_deg(flight) - flight.pitch.to_degrees();
                keys.pitch =
                    (0.08 * error - 0.3 * flight.pitch_rate.to_degrees() / 10.).clamp(-1., 1.);
                keys.roll = (-flight.bank.to_degrees() / 30.).clamp(-1., 1.);
                if agl > PROBE_CRUISE_AGL_FT
                    && y >= self.terrain.highest_ft + PROBE_LEVEL_OFF_MARGIN_FT
                {
                    keys.pitch = 0.;
                    keys.roll = 0.;
                    keys.commands = vec![
                        Set(Switch::Burner, false),
                        Throttle(0.85),
                        Set(Switch::Autopilot, true),
                    ];
                    self.phase = ProbePhase::Cruise;
                    println!("t={tick} player: levelling off at {agl:.0} ft AGL, autopilot on");
                }
            }
            ProbePhase::Cruise => {
                if let (Some((from, until)), Some(ground)) = (script.home, ground)
                    && (from..until).contains(&tick)
                {
                    let centre = ground.runway.center;
                    flight.autopilot.set_navigation_target(Some(
                        tore_sim::autopilot::NavigationTarget {
                            number: 1,
                            position: [centre[0], centre[2]],
                        },
                    ));
                    keys.commands = vec![
                        Set(Switch::Gear, true),
                        Throttle(0.6),
                        Set(Switch::WaypointAutopilot, true),
                    ];
                    self.phase = ProbePhase::Home;
                    println!("t={tick} player: gear down, flying over the departure airfield");
                } else {
                    self.climb_over_terrain(tick, flight, keys, y);
                }
            }
            ProbePhase::Home => {
                if script.home.is_some_and(|(_, until)| tick >= until) {
                    keys.commands = vec![
                        Set(Switch::Gear, false),
                        Throttle(0.85),
                        Set(Switch::Autopilot, true),
                    ];
                    self.phase = ProbePhase::Away;
                    println!("t={tick} player: gear up, leaving the airfield");
                }
            }
            ProbePhase::Away => self.climb_over_terrain(tick, flight, keys, y),
        }
    }

    /// In the cruise on the autopilot's held altitude: if the ground ahead
    /// rises to within reach of it, hand back to the climb, at full power.
    fn climb_over_terrain(
        &mut self,
        tick: u64,
        flight: &flight::State,
        keys: &mut flight::PilotInput,
        y: f64,
    ) {
        use flight::{PilotCommand::*, Switch};
        if y >= self.terrain.highest_ft + PROBE_RECLIMB_MARGIN_FT {
            return;
        }
        let afterburner = flight
            .model()
            .configuration()
            .propulsion
            .afterburner_thrust_lbf
            > 0.;
        keys.commands = vec![Set(Switch::Autopilot, false), Throttle(1.)];
        if afterburner {
            keys.commands.push(Set(Switch::Burner, true));
        }
        self.phase = ProbePhase::Climb { flaps_up: true };
        println!(
            "t={tick} player: ground ahead at {:.0} ft, climbing from {y:.0} ft",
            self.terrain.highest_ft
        );
    }
}

/// Per-actor transitions for the AI probe: airfield phase and activity for
/// the player's wing, deaths for everyone, and ground hazards (off the
/// landable surface, inside a building, two aircraft within
/// [`PROBE_CLOSE_FT`], or stopped mid-taxi for [`PROBE_STUCK_S`]).
#[derive(Default)]
struct ProbeWatch {
    last: std::collections::BTreeMap<u32, String>,
    player: Option<String>,
    priority: Option<u32>,
    hazards: std::collections::BTreeSet<(u32, u32, &'static str)>,
    stopped_since: std::collections::BTreeMap<u32, u64>,
    firsts: std::collections::BTreeMap<u32, (String, Vec<(String, u64)>)>,
    events: usize,
    /// Ticks between trace lines for the player's wing, 0 for none.
    trace: u64,
    go_arounds: std::collections::BTreeMap<u32, u32>,
    /// Distance past each final's aim point last tick, to catch the
    /// threshold crossing.
    final_past: std::collections::BTreeMap<u32, f64>,
    /// A vertical type's wheels last tick (slice H2): on the ground, sink
    /// ft/s and ground speed kt, for its `TOUCHDOWN` line.
    wheels: std::collections::BTreeMap<u32, (bool, f64, f64)>,
    /// `--probe-redfor`: the player's wing is Enemy Wing 1 and the player
    /// flies this plane, not plane 0.
    redfor: Option<u32>,
}

/// Aircraft on the ground closer than this are reported as touching.
const PROBE_CLOSE_FT: f64 = 30.;
/// A taxiing aircraft stopped this long is reported as possibly stuck.
const PROBE_STUCK_S: f64 = 60.;
/// Height above the surface below which an aircraft counts as on the ground.
const PROBE_GROUND_AGL_FT: f64 = 15.;
/// Id used for the player in hazard pairs.
const PROBE_PLAYER: u32 = 0;

impl ProbeWatch {
    fn observe(
        &mut self,
        tick: u64,
        bridge: &ai_wings::AiWings,
        player: &flight::State,
        world: &terrain::Terrain,
    ) {
        use tore_sim::ai::airfield::Phase;
        let seconds = tick as f64 / 120.;
        let line = |f: &flight::State| {
            let [x, y, z] = f.position;
            format!(
                "agl={:.0} kt={:.0} x={x:.0} z={z:.0} hdg={:.0} terrain_agl={:.0}",
                y - world.surface(x, z).height,
                f.speed / 1.68781,
                f.yaw.to_degrees().rem_euclid(360.),
                y - f64::from(world.height(x as f32, z as f32))
            )
        };
        let [px, py, pz] = player.position;
        let player_ground = world.surface(px, pz);
        let player_on_ground = player.research.as_ref().is_some_and(|r| r.on_ground);
        let key = format!(
            "on_ground={player_on_ground} gear={} crashed={}",
            player.gear_down, player.crashed
        );
        if self.player.as_ref() != Some(&key) {
            println!("t={tick} ({seconds:.1}s) player: {key} {}", line(player));
            self.player = Some(key);
        }
        let priority = bridge
            .mission()
            .priority_landing(self.redfor.unwrap_or(ai_wings::PLAYER_ID));
        if priority != self.priority {
            println!("t={tick} ({seconds:.1}s) player landing priority: {priority:?}");
            self.priority = priority;
        }
        let mut grounded = Vec::new();
        if !player.crashed && py - player_ground.height < PROBE_GROUND_AGL_FT {
            grounded.push((PROBE_PLAYER, player.position));
        }
        for slot in bridge.slots() {
            let Some(actor) = bridge.mission().actor(slot.id) else {
                continue;
            };
            let f = actor.flight();
            let phase = actor.airfield_phase();
            let own_side = if self.redfor.is_some() {
                tore_sim::ai::launch::Side::Enemy
            } else {
                tore_sim::ai::launch::Side::Friendly
            };
            let own_wing = slot.side == own_side && slot.wing_number == 1;
            if own_wing && self.trace > 0 && tick.is_multiple_of(self.trace) {
                println!(
                    "t={tick} ({seconds:.1}s) trace {}: {:?} leg={:?} {} thr={:.2} brake={}",
                    slot.label(),
                    phase,
                    actor.airfield().map(|s| s.leg()),
                    line(f),
                    f.throttle,
                    f.brake_out
                );
            }
            let key = if own_wing {
                format!(
                    "phase={} activity=\"{}\" alive={}",
                    phase.map_or("-".into(), |p| format!("{p:?}")),
                    actor.activity().label(),
                    actor.alive()
                )
            } else {
                format!("alive={}", actor.alive())
            };
            if self.last.get(&slot.id) != Some(&key) {
                println!(
                    "t={tick} ({seconds:.1}s) {} {:?}: {key} {}",
                    slot.label(),
                    slot.aircraft,
                    line(f)
                );
                self.last.insert(slot.id, key);
                self.events += 1;
                if own_wing {
                    let entry = self
                        .firsts
                        .entry(slot.id)
                        .or_insert_with(|| (slot.label(), Vec::new()));
                    let name = phase.map_or("Airborne".into(), |p| format!("{p:?}"));
                    if !entry.1.iter().any(|(n, _)| *n == name) {
                        entry.1.push((name, tick));
                    }
                }
            }
            if let Some(sequence) = actor.airfield() {
                let count = sequence.go_arounds();
                if count > *self.go_arounds.get(&slot.id).unwrap_or(&0) {
                    let point = sequence.landing_point();
                    let [x, _, z] = f.position;
                    let [vx, _, vz] = f.velocity;
                    // Signed distance past the landing point along the track.
                    let past = ((x - point[0]) * vx + (z - point[2]) * vz) / vx.hypot(vz).max(1.);
                    println!(
                        "t={tick} ({seconds:.1}s) GO-AROUND {} #{count}: {} vs={:.0} {:.0} ft past the landing point, {:.0} ft above it, landable below={}",
                        slot.label(),
                        line(f),
                        f.velocity[1],
                        past,
                        f.position[1] - point[1],
                        world.surface(x, z).landable
                    );
                }
                self.go_arounds.insert(slot.id, count);
                // The wheel height over the threshold on final: the ILS
                // path crosses it about 52 ft up.
                if sequence.phase() == tore_sim::ai::airfield::Phase::Final
                    && actor.alive()
                    && !f.crashed
                {
                    let point = sequence.landing_point();
                    let [x, y, z] = f.position;
                    let [vx, _, vz] = f.velocity;
                    let past = ((x - point[0]) * vx + (z - point[2]) * vz) / vx.hypot(vz).max(1.);
                    let at = -tore_sim::airport::AIM_PAST_THRESHOLD_FT;
                    if let Some(&before) = self.final_past.get(&slot.id)
                        && before < at
                        && past >= at
                    {
                        let clearance = actor
                            .flight()
                            .model()
                            .configuration()
                            .equipment
                            .ground_clearance_ft;
                        println!(
                            "t={tick} ({seconds:.1}s) THRESHOLD {} wheels={:.0} ft (path {:.0}) kt={:.0} end={:?}",
                            slot.label(),
                            y - clearance - point[1],
                            tore_sim::airport::threshold_crossing_height_ft(),
                            f.speed * 3600. / 6076.12,
                            sequence.landing_end()
                        );
                    }
                    self.final_past.insert(slot.id, past);
                } else {
                    self.final_past.remove(&slot.id);
                }
            }
            // A vertical type's touchdowns (slice H2): the sink and ground
            // speed it came down at, and whether the gear was damaged.
            if let Some(sequence) = actor
                .airfield()
                .filter(|s| s.lift() != tore_sim::ai::airfield::Lift::Conventional)
            {
                let wheels = f.research.as_ref().is_some_and(|r| r.on_ground);
                let now = (
                    wheels,
                    -f.velocity[1],
                    f.velocity[0].hypot(f.velocity[2]) / 1.68781,
                );
                if !self.wheels.contains_key(&slot.id) && !sequence.is_departure() {
                    println!(
                        "t={tick} ({seconds:.1}s) VERTICAL LANDING {} lift={:?} phase={:?} kt={:.0} nacelle={:.0} nozzle={:.0}",
                        slot.label(),
                        sequence.lift(),
                        sequence.phase(),
                        f.speed / 1.68781,
                        f.nacelle_degrees(),
                        f.nozzle_degrees()
                    );
                }
                if let Some((false, sink, knots)) = self.wheels.get(&slot.id).copied()
                    && wheels
                    && !sequence.is_departure()
                {
                    println!(
                        "t={tick} ({seconds:.1}s) TOUCHDOWN {} sink_fps={sink:.1} ground_kt={knots:.1} phase={:?} lift={:?} nacelle={:.0} gear_damaged={} crashed={}",
                        slot.label(),
                        sequence.phase(),
                        sequence.lift(),
                        f.nacelle_degrees(),
                        f.systems.has(16),
                        f.crashed
                    );
                }
                self.wheels.insert(slot.id, now);
            } else {
                self.wheels.remove(&slot.id);
            }
            if !actor.alive() || f.crashed {
                continue;
            }
            let [x, y, z] = f.position;
            let surface = world.surface(x, z);
            if y - surface.height >= PROBE_GROUND_AGL_FT {
                self.stopped_since.remove(&slot.id);
                continue;
            }
            grounded.push((slot.id, f.position));
            let hazard = |this: &mut Self, what: &'static str, on: bool| {
                let entry = (slot.id, slot.id, what);
                if on && this.hazards.insert(entry) {
                    println!(
                        "t={tick} ({seconds:.1}s) HAZARD {what}: {} {}",
                        slot.label(),
                        line(f)
                    );
                } else if !on && this.hazards.remove(&entry) {
                    println!("t={tick} ({seconds:.1}s) clear {what}: {}", slot.label());
                }
            };
            hazard(self, "off landable surface", !surface.landable);
            let probe = [x, surface.height + 6., z];
            let inside = world
                .solid_contact(
                    probe,
                    probe,
                    world.airport_scene.objects.iter().map(|o| o.id),
                )
                .is_some();
            hazard(self, "inside building", inside);
            let taxiing = matches!(phase, Some(Phase::Taxi | Phase::LineUp | Phase::TaxiClear));
            if taxiing && f.speed < 1. {
                let since = *self.stopped_since.entry(slot.id).or_insert(tick);
                hazard(
                    self,
                    "stopped while taxiing",
                    (tick - since) as f64 / 120. >= PROBE_STUCK_S,
                );
            } else {
                self.stopped_since.remove(&slot.id);
                hazard(self, "stopped while taxiing", false);
            }
        }
        for (i, (a, pa)) in grounded.iter().enumerate() {
            for (b, pb) in &grounded[i + 1..] {
                let close = (pa[0] - pb[0]).hypot(pa[2] - pb[2]) < PROBE_CLOSE_FT;
                let entry = (*a, *b, "aircraft within 30 ft");
                if close && self.hazards.insert(entry) {
                    println!(
                        "t={tick} ({seconds:.1}s) HAZARD aircraft within 30 ft: ids {a} and {b}"
                    );
                } else if !close {
                    self.hazards.remove(&entry);
                }
            }
        }
    }

    fn summary(&self) {
        let mut climbs = Vec::new();
        for (label, phases) in self.firsts.values() {
            let text: Vec<_> = phases
                .iter()
                .map(|(name, tick)| format!("{name}@{:.1}s", *tick as f64 / 120.))
                .collect();
            println!("AI probe phases: {label}: {}", text.join(" "));
            if let Some((_, tick)) = phases.iter().find(|(n, _)| n == "ClimbOut") {
                climbs.push(*tick);
            }
        }
        climbs.sort_unstable();
        let gaps: Vec<_> = climbs
            .windows(2)
            .map(|w| format!("{:.1}s", (w[1] - w[0]) as f64 / 120.))
            .collect();
        println!(
            "AI probe liftoff gaps: [{}] hazards_open={} transitions={}",
            gaps.join(", "),
            self.hazards.len(),
            self.events
        );
    }
}

/// Longest gun burst the scripted leader holds, in ticks.
const PROBE_BURST_TICKS: u64 = 120;
/// Ticks a selected missile may go without READY before the scripted leader
/// changes to the gun, when the gun reaches the target.
const PROBE_LOCK_TICKS: u64 = 240;
/// The scripted leader fires the gun with the target this close to the pipper.
const PROBE_PIPPER_DEG: f64 = 2.;

/// Adds a command to the seat's queue, unless the queue is full.
fn queue_command(queue: &mut Vec<seats::SeatCommand>, command: seats::SeatCommand) {
    if queue.len() < MAX_SEAT_COMMANDS {
        queue.push(command);
    }
}

/// The gun key (Tab, `fire-gun`) going down or up since the last tick, queued
/// as a combat command in order with the seat's others. `down` is false while
/// paused, in a menu or out of focus, as the trigger is.
fn queue_gun_key(commands: &mut Vec<seats::SeatCommand>, sent: &mut bool, down: bool) {
    if down != *sent {
        *sent = down;
        queue_command(
            commands,
            seats::SeatCommand::Combat(if down {
                tore_sim::combat::live::Command::GunKeyDown
            } else {
                tore_sim::combat::live::Command::GunKeyUp
            }),
        );
    }
}

/// The Space key going down or up, as the probe's leader presses it.
fn trigger_key(down: bool) -> seats::SeatCommand {
    seats::SeatCommand::TriggerKey {
        down,
        repeat: false,
        blocked: false,
    }
}

/// The scripted leader's own weapons for `--probe-attack`, and what the probe
/// reports about the fight that follows.
///
/// `fitted` test harness (agent decision, 2026-09-26), not game behaviour. The
/// leader uses only the player's own controls, queued as seat commands the way
/// key and mouse input is: a scope click designates the nearest hostile aircraft among
/// the player's current sensor contacts, `]` steps through NAV and the
/// stations to the chosen weapon, and Space fires. Rule: the weapon is the
/// longest-reaching air-to-air store (missile or gun) whose employment zone
/// holds the contact's observed range. A missile is one press once its readout
/// says READY; after [`PROBE_LOCK_TICKS`] without READY the leader changes to
/// the gun if the gun reaches. The gun fires while the target sits within
/// [`PROBE_PIPPER_DEG`] of the HUD's gun pipper, for at most
/// [`PROBE_BURST_TICKS`]. The flying is left to the rest of the probe script.
/// After each shot the next attack waits the repeat interval, choosing the
/// nearest target again; a single attack ends with its first shot.
struct ProbeAttacker {
    repeat: u64,
    /// Tick of the next attack, `None` once a single attack has fired.
    next: Option<u64>,
    /// The attack in progress has not yet chosen its target.
    fresh: bool,
    /// The attack in progress has given up on missiles.
    guns: bool,
    force_guns: bool,
    /// The selected missile station and the tick it began waiting for READY.
    waiting: Option<(usize, u64)>,
    /// A missile press, released on the next tick.
    pressed: bool,
    /// The station the leader has just stepped toward, and the contact's range,
    /// to report once the step has applied the command.
    selecting: Option<(usize, f64)>,
    /// First tick of the gun burst in progress.
    burst: Option<u64>,
    /// The leader's scope clicks, `]` presses, trigger presses, missiles,
    /// gun bursts and gun rounds.
    clicks: u32,
    steps: u32,
    presses: u32,
    missiles: u32,
    bursts: u32,
    rounds: u32,
    /// Hits on and destructions of AI aircraft by anyone, and hits on the
    /// player, from the combat events.
    hits: u32,
    destroyed: u32,
    player_damaged: u32,
    /// Each AI aircraft's engagement gate last tick.
    neutral: std::collections::BTreeMap<u32, bool>,
    /// AI aircraft that have perceived an attack, or defended against a missile.
    attacked: std::collections::BTreeSet<u32>,
    defending: std::collections::BTreeSet<u32>,
    /// Decoys the AI aircraft carried at the start.
    decoys: u32,
    ejections: u32,
}

impl ProbeAttacker {
    fn new(
        attack: ProbeAttack,
        combat: &combat::Combat,
        bridge: &ai_wings::AiWings,
        force_guns: bool,
    ) -> Self {
        let state = &combat.state;
        let stations: Vec<_> = state
            .own()
            .configuration()
            .stations
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let zone = s.weapon.seeker.zones[1];
                format!(
                    "{} x{} {}..{} ft{}",
                    s.weapon.hud_name,
                    state.own().rounds(i),
                    zone.minimum_range,
                    zone.maximum_range,
                    if probe_air_to_air(&s.weapon) {
                        ""
                    } else {
                        " (not air to air)"
                    }
                )
            })
            .collect();
        println!(
            "AI probe attack: from=t{} repeat={:.1}s stations=[{}]",
            attack.from,
            attack.repeat as f64 / 120.,
            stations.join(", ")
        );
        let actors = bridge.mission().actors();
        Self {
            repeat: attack.repeat,
            next: Some(attack.from),
            fresh: true,
            guns: force_guns,
            force_guns,
            waiting: None,
            pressed: false,
            selecting: None,
            burst: None,
            clicks: 0,
            steps: 0,
            presses: 0,
            missiles: 0,
            bursts: 0,
            rounds: 0,
            hits: 0,
            destroyed: 0,
            player_damaged: 0,
            neutral: actors.iter().map(|a| (a.id(), a.is_neutral())).collect(),
            attacked: Default::default(),
            defending: Default::default(),
            decoys: probe_decoys(bridge),
            ejections: 0,
        }
    }

    /// The player's controls for this tick, queued as seat commands the way a
    /// key or a mouse click queues them. One control per tick.
    fn aim(
        &mut self,
        tick: u64,
        combat: &combat::Combat,
        flight: &flight::State,
        bridge: &ai_wings::AiWings,
        commands: &mut Vec<seats::SeatCommand>,
    ) {
        use tore_sim::combat::live::{Command, Readiness, is_gun};
        if std::mem::take(&mut self.pressed) {
            commands.push(trigger_key(false));
        }
        if self.next.is_none_or(|next| tick < next) {
            return;
        }
        let launcher = combat::launcher(flight);
        if !launcher.alive
            || flight.escape.is_some()
            || flight.systems.pilot.dead
            || combat.state.own().hp <= 0
        {
            self.end_burst(tick, commands);
            return;
        }
        let seconds = tick as f64 / 120.;
        let state = &combat.state;
        let hostile = |id: u32| {
            bridge
                .slot(id)
                .is_some_and(|s| s.side == tore_sim::ai::launch::Side::Enemy)
                && state.targets.iter().any(|t| t.id == id && t.hp > 0)
        };
        let current = state
            .own_view()
            .designated()
            .filter(|id| hostile(*id) && state.own().sensors.contact(*id).is_some());
        if self.fresh || current.is_none() {
            let nearest = state
                .own()
                .sensors
                .contacts()
                .iter()
                .filter(|c| !c.destroyed && hostile(c.id))
                .min_by(|a, b| {
                    a.distance_ft
                        .total_cmp(&b.distance_ft)
                        .then(a.id.cmp(&b.id))
                })
                .map(|c| (c.id, c.distance_ft));
            let Some((id, range)) = nearest else {
                self.end_burst(tick, commands);
                return;
            };
            self.fresh = false;
            if current != Some(id) {
                self.end_burst(tick, commands);
                self.guns = self.force_guns;
                self.waiting = None;
                commands.push(seats::SeatCommand::Combat(Command::DesignateTarget(id)));
                self.clicks += 1;
                println!(
                    "t={tick} ({seconds:.1}s) attack: designates {} at {range:.0} ft",
                    probe_label(bridge, id)
                );
                return;
            }
        }
        let Some((target, contact)) = combat
            .state
            .own_view()
            .designated()
            .and_then(|id| Some((id, *combat.state.own().sensors.contact(id)?)))
        else {
            return;
        };
        let range = contact.distance_ft;
        let station = if self.force_guns {
            probe_station(&combat.state, range, true)
        } else {
            self.guns
                .then(|| probe_station(&combat.state, range, true))
                .flatten()
                .or_else(|| probe_station(&combat.state, range, false))
        };
        let Some(station) = station else {
            self.end_burst(tick, commands);
            return;
        };
        if !combat.state.own().armed || combat.state.own().selected != station {
            self.end_burst(tick, commands);
            commands.push(seats::SeatCommand::CycleWeapon { forward: true });
            self.steps += 1;
            // Reported once the command has run: see `commands_applied`.
            self.selecting = Some((station, range));
            return;
        }
        let ready = combat.state.own().release_readiness == Readiness::Ready;
        if is_gun(&combat.state.own().configuration().stations[station].weapon) {
            let on = ready && probe_on_pipper(&combat.state, &launcher, station, &contact);
            match self.burst {
                Some(from) if !on || tick - from >= PROBE_BURST_TICKS => {
                    self.end_burst(tick, commands);
                }
                Some(_) => {}
                None if on => {
                    commands.push(trigger_key(false));
                    commands.push(trigger_key(true));
                    self.burst = Some(tick);
                    self.presses += 1;
                    println!(
                        "t={tick} ({seconds:.1}s) attack: gun at {} range={range:.0} ft",
                        probe_label(bridge, target)
                    );
                }
                None => {}
            }
        } else if ready {
            commands.push(trigger_key(false));
            commands.push(trigger_key(true));
            self.pressed = true;
            self.presses += 1;
        } else {
            let since = match self.waiting {
                Some((waiting, since)) if waiting == station => since,
                _ => {
                    self.waiting = Some((station, tick));
                    tick
                }
            };
            if tick - since >= PROBE_LOCK_TICKS
                && !self.guns
                && probe_station(&combat.state, range, true).is_some()
            {
                self.guns = true;
                println!(
                    "t={tick} ({seconds:.1}s) attack: no {} shot ({}), changes to the gun",
                    combat.state.own().configuration().stations[station]
                        .weapon
                        .hud_name,
                    combat.state.own().release_readiness.label()
                );
            }
        }
    }

    /// Reports a weapon selection once the step has applied it.
    fn commands_applied(&mut self, tick: u64, combat: &combat::Combat) {
        let Some((station, range)) = self.selecting.take() else {
            return;
        };
        if combat.state.own().armed && combat.state.own().selected == station {
            println!(
                "t={tick} ({:.1}s) attack: selects {} at {range:.0} ft",
                tick as f64 / 120.,
                combat.state.own().configuration().stations[station]
                    .weapon
                    .hud_name
            );
        }
    }

    /// Release a gun burst in progress; the burst counts as this attack's shot.
    fn end_burst(&mut self, tick: u64, commands: &mut Vec<seats::SeatCommand>) {
        if self.burst.take().is_some() {
            commands.push(trigger_key(false));
            self.bursts += 1;
            self.shot(tick);
        }
    }

    fn shot(&mut self, tick: u64) {
        self.next = (self.repeat > 0).then_some(tick + self.repeat);
        self.fresh = true;
        self.guns = false;
        self.waiting = None;
    }

    /// This tick's combat events, after `Combat::step`.
    fn events(
        &mut self,
        tick: u64,
        events: &[tore_sim::combat::live::Event],
        combat: &combat::Combat,
        bridge: &ai_wings::AiWings,
    ) {
        use tore_sim::combat::live::Event;
        let seconds = tick as f64 / 120.;
        for event in events {
            match event {
                Event::Fired { station, .. } => {
                    let weapon = &combat.state.own().configuration().stations[*station].weapon;
                    if tore_sim::combat::live::is_gun(weapon) {
                        self.rounds += 1;
                        continue;
                    }
                    self.missiles += 1;
                    let target = combat.state.own_view().designated();
                    println!(
                        "t={tick} ({seconds:.1}s) attack: fires {} at {} range={:.0} ft",
                        weapon.hud_name,
                        target.map_or("-".into(), |id| probe_label(bridge, id)),
                        target
                            .and_then(|id| combat.state.own().sensors.observation(id))
                            .map_or(0., |c| c.distance_ft)
                    );
                    self.shot(tick);
                }
                Event::Hit(id) if bridge.slot(*id).is_some() => self.hits += 1,
                Event::Destroyed(id) if bridge.slot(*id).is_some() => {
                    self.destroyed += 1;
                    println!(
                        "t={tick} ({seconds:.1}s) destroyed: {}",
                        probe_label(bridge, *id)
                    );
                }
                Event::OwnshipDamaged { .. } => self.player_damaged += 1,
                Event::OwnshipDestroyed { .. } => {
                    println!("t={tick} ({seconds:.1}s) destroyed: player");
                }
                _ => {}
            }
        }
    }

    /// After the tick: perceived attacks, releases, missile defence and the
    /// ejections the tick reported.
    fn observe(&mut self, tick: u64, bridge: &ai_wings::AiWings, ejections: &[&str]) {
        let seconds = tick as f64 / 120.;
        for slot in bridge.slots() {
            let Some(actor) = bridge.mission().actor(slot.id) else {
                continue;
            };
            if let Some(attack) = actor.perceived_attacks().first()
                && self.attacked.insert(slot.id)
            {
                println!(
                    "t={tick} ({seconds:.1}s) {} perceives an attack on {} by {}",
                    slot.label(),
                    probe_label(bridge, attack.report.defended_id),
                    attack
                        .report
                        .attacker_id
                        .map_or("an unknown attacker".into(), |id| probe_label(bridge, id))
                );
            }
            let neutral = actor.is_neutral();
            if self.neutral.insert(slot.id, neutral) == Some(true) && !neutral {
                println!(
                    "t={tick} ({seconds:.1}s) {} released to engage",
                    slot.label()
                );
            }
            if actor.defense_decision().is_some_and(|d| d.motion.is_some())
                && self.defending.insert(slot.id)
            {
                println!(
                    "t={tick} ({seconds:.1}s) {} defends against a missile",
                    slot.label()
                );
            }
        }
        for message in ejections {
            println!("t={tick} ({seconds:.1}s) {message}");
            self.ejections += 1;
        }
    }

    fn summary(&self, combat: &combat::Combat, flight: &flight::State, bridge: &ai_wings::AiWings) {
        use tore_sim::ai::launch::Side;
        let lost = |side: Side| {
            let slots: Vec<_> = bridge.slots().iter().filter(|s| s.side == side).collect();
            let down = slots
                .iter()
                .filter(|s| bridge.mission().actor(s.id).is_none_or(|a| !a.alive()))
                .count();
            format!("{down}/{}", slots.len())
        };
        let released = self.neutral.values().filter(|n| !**n).count();
        println!(
            "AI probe attack: clicks={} steps={} presses={} missiles={} gun_bursts={} gun_rounds={} player_hits={} player_kills={} hits={} destroyed={} lost_friendly={} lost_enemy={} player_alive={} player_damaged={} ejections={} perceived={} released={released} defending={} decoys_used={}",
            self.clicks,
            self.steps,
            self.presses,
            self.missiles,
            self.bursts,
            self.rounds,
            combat.state.own().hits,
            combat.state.own().kills,
            self.hits,
            self.destroyed,
            lost(Side::Friendly),
            lost(Side::Enemy),
            !flight.crashed && combat.state.own().hp > 0,
            self.player_damaged,
            self.ejections,
            self.attacked.len(),
            self.defending.len(),
            self.decoys.saturating_sub(probe_decoys(bridge)),
        );
    }
}

/// A missile with an air-to-air seeker profile, or a gun.
fn probe_air_to_air(weapon: &tore_formats::weapons::Weapon) -> bool {
    use tore_sim::combat::missiles::{Profile, TargetRole};
    tore_sim::combat::live::is_gun(weapon)
        || Profile::for_weapon(weapon).is_some_and(|p| p.role == TargetRole::Aircraft)
}

/// The scripted leader's weapon at this range: the longest-reaching loaded
/// air-to-air station, or gun, whose employment zone holds it.
fn probe_station(state: &tore_sim::combat::live::State, range_ft: f64, gun: bool) -> Option<usize> {
    state
        .own()
        .configuration()
        .stations
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            let zone = s.weapon.seeker.zones[1];
            state.own().rounds(*i) > 0
                && state.own().ammo[*i] & 0x8000 == 0
                && probe_air_to_air(&s.weapon)
                && (!gun || tore_sim::combat::live::is_gun(&s.weapon))
                && range_ft >= f64::from(zone.minimum_range)
                && range_ft <= f64::from(zone.maximum_range)
        })
        .max_by(|(a, x), (b, y)| {
            x.weapon.seeker.zones[1]
                .maximum_range
                .cmp(&y.weapon.seeker.zones[1].maximum_range)
                .then(b.cmp(a))
        })
        .map(|(i, _)| i)
}

/// Whether the contact sits within [`PROBE_PIPPER_DEG`] of the gun pipper,
/// solved as the HUD solves it, and inside the gun's reach.
fn probe_on_pipper(
    state: &tore_sim::combat::live::State,
    launcher: &tore_sim::combat::live::Launcher,
    station: usize,
    contact: &tore_sim::sensors::Contact,
) -> bool {
    use tore_sim::{
        combat::{gunsight, missiles},
        sensors::Channel,
    };
    let station = &state.own().configuration().stations[station];
    let radar = (contact.channel == Channel::Radar
        && state.own().sensors.operating(Channel::Radar)
        && launcher.radar)
        .then_some(gunsight::TargetObservation {
            position: contact.position,
            velocity: contact.velocity,
        });
    let Ok(Some(solution)) = gunsight::solve(&station.weapon, launcher, station.gun_mount(), radar)
    else {
        return false;
    };
    let pipper = missiles::sub(solution.point, launcher.position);
    let target = missiles::sub(contact.position, launcher.position);
    let range = missiles::length(target);
    range <= solution.maximum_range_ft
        && tore_sim::attitude::dot(pipper, target) / (missiles::length(pipper) * range).max(1.)
            >= PROBE_PIPPER_DEG.to_radians().cos()
}

/// Decoys every AI aircraft still carries.
fn probe_decoys(bridge: &ai_wings::AiWings) -> u32 {
    bridge
        .mission()
        .actors()
        .iter()
        .flat_map(|a| a.dispensers())
        .map(|d| d.count)
        .sum()
}

/// "Enemy 1-2" for an AI aircraft, "player" for the human leader.
fn probe_label(bridge: &ai_wings::AiWings, id: u32) -> String {
    bridge.slot(id).map_or_else(
        || {
            if id == 0 {
                "player".into()
            } else {
                format!("id {id}")
            }
        },
        ai_wings::Slot::label,
    )
}

/// Print what each wing that lost its human leader is doing on its mission
/// of opportunity, whenever that changes (John, 2026-09-30).
fn print_opportunity_notes(tick: u64, wings: &ai_wings::AiWings, notes: &mut Vec<String>) {
    let label = |id: u32| {
        wings
            .slots()
            .iter()
            .find(|slot| slot.id == id)
            .map_or_else(|| format!("aircraft {id}"), |slot| slot.label())
    };
    let mission = wings.mission();
    for (index, opportunity) in mission.opportunities().iter().enumerate() {
        let leader = mission
            .wing_leader(opportunity.side, opportunity.wing)
            .map_or_else(|| "nobody".to_owned(), label);
        // Contact on the latest step (the mission's tick has moved past it).
        let in_contact = opportunity.last_contact_tick + 1 == mission.tick()
            && opportunity.last_contact_tick > opportunity.started_tick;
        let doing = match (
            opportunity.home,
            opportunity.searching,
            opportunity.search_over,
        ) {
            (Some((_, reason)), ..) => format!("returning to base ({})", reason.label()),
            _ if in_contact => "enemy in contact".to_owned(),
            (None, Some(id), _) => format!("searching where {} was last seen", label(id)),
            (None, None, Some((_, end))) => format!(
                "flying its waypoints, {} left ({})",
                opportunity.route.len(),
                end.label()
            ),
            // Only before its first step: the lead has just passed.
            (None, None, None) => "taking the lead".to_owned(),
        };
        let note = format!("mission of opportunity: {leader} leads, {doing}");
        if notes.get(index) != Some(&note) {
            println!("t={tick} ({:.1}s) {note}", tick as f64 / 120.);
            if index < notes.len() {
                notes[index] = note;
            } else {
                notes.push(note);
            }
        }
    }
}

/// Deterministic test projectile, using the selected aircraft's imported gun.
/// AAA is a stationary ground-source firing fixture, not a ground AI actor.
fn inject_probe_threat(
    kind: ProbeThreat,
    ordinal: usize,
    bridge: &mut ai_wings::AiWings,
    combat: &mut combat::Combat,
    world: &terrain::Terrain,
) -> AppResult<()> {
    use tore_sim::combat::live;
    let slot = bridge
        .slots()
        .iter()
        .find(|s| s.side == tore_sim::ai::launch::Side::Enemy)
        .ok_or("probe threat needs an enemy")?;
    let target = combat
        .state
        .targets
        .iter_mut()
        .find(|t| t.id == slot.id)
        .ok_or("probe target missing")?;
    if let ProbeThreat::Fault(index) = kind {
        target.faults.counts[index] = target.faults.counts[index].saturating_add(1);
        return Ok(());
    }
    if kind == ProbeThreat::Hit {
        target.hp = (target.hp - 1).max(1);
        bridge.report_weapon_hits(&[live::Event::Hit(target.id)]);
        return Ok(());
    }
    let own = bridge
        .mission()
        .actor(slot.id)
        .ok_or("probe actor missing")?
        .flight();
    let basis = attitude::Basis::new(own.yaw, own.pitch, own.bank);
    let mut origin: [f64; 3] = std::array::from_fn(|i| own.position[i] - basis.forward[i] * 1500.);
    if kind == ProbeThreat::Aaa {
        origin[1] = f64::from(world.height(origin[0] as f32, origin[2] as f32)) + 10.;
    }
    let station = combat
        .state
        .own()
        .configuration()
        .stations
        .iter()
        .position(|s| live::is_gun(&s.weapon))
        .ok_or("probe aircraft has no gun")?;
    // Fixed 3,000 ft/s ballistic firing fixture. Lead the initial measured
    // target velocity; a 50 ft offset yields a threatening near pass.
    let speed = 3000.;
    let mut time = 0.;
    let mut delta = [0.; 3];
    for _ in 0..8 {
        delta = std::array::from_fn(|i| {
            own.position[i] + own.velocity[i] * time + basis.right[i] * 50. - origin[i]
        });
        time = delta.iter().map(|v| v * v).sum::<f64>().sqrt() / speed;
    }
    let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
    combat.state.projectiles.push(live::Projectile {
        id: 900_000 + combat.state.tick() as u32 * 64 + ordinal as u32,
        owner: 900_001,
        weapon: None,
        guidance: None,
        motion: None,
        guidance_ticks: None,
        age: 0,
        incoming: None,
        station,
        position: origin,
        previous: origin,
        direction: delta.map(|v| v / length),
        speed_f8: (speed * 256.) as i32,
        launched_t: (combat.state.tick() / 30) as u16,
        target: None,
        fall: Default::default(),
        gun_round: Some(0),
        tracer: true,
    });
    Ok(())
}

fn verify_probe_attitudes(
    snapshot: &snapshot::RenderSnapshot,
    bridge: &ai_wings::AiWings,
) -> AppResult<()> {
    for actor in bridge.mission().actors().iter().filter(|a| a.alive()) {
        let pose = snapshot
            .target(actor.id())
            .ok_or("AI probe render snapshot omitted an actor")?;
        let body = actor.flight();
        let expected = attitude::Basis::new(body.yaw, body.pitch, body.bank);
        let drawn = attitude::Basis::new(pose.attitude[0], pose.attitude[1], pose.attitude[2]);
        if attitude::dot(expected.forward, drawn.forward) < 1. - 1e-10
            || attitude::dot(expected.up, drawn.up) < 1. - 1e-10
        {
            return Err(format!(
                "AI probe actor {} lost its simulated attitude in the recording",
                actor.id()
            )
            .into());
        }
    }
    Ok(())
}

fn probe_case_name(
    player: tore_formats::aircraft::AircraftId,
    enemy: tore_formats::aircraft::AircraftId,
    skill: usize,
    geometry: ProbeGeometry,
    researched: bool,
) -> String {
    format!(
        "{}-{}-skill{skill}-{geometry:?}-{}",
        player
            .selection_key()
            .trim_end_matches(".PT")
            .to_ascii_uppercase(),
        enemy
            .selection_key()
            .trim_end_matches(".PT")
            .to_ascii_uppercase(),
        if researched { "researched" } else { "legacy" }
    )
}

/// The probe builds its AI bridge even when it holds no aircraft.
const PROBE_BRIDGE: &str = "the probe always builds its AI bridge";

/// One `data link:` line of the AI probe: a change of the flight data link's
/// picture, with the combat tick it happened on. Scenarios assert on these.
fn data_link_line(entry: &tore_world::datalink::Entry) -> String {
    use tore_world::datalink::Entry;
    match entry {
        Entry::Member { tick, plane, radar } => {
            format!("t={tick} data link: member plane={plane} radar={radar}")
        }
        Entry::Lock {
            tick,
            plane,
            target,
        } => {
            format!("t={tick} data link: lock plane={plane} target={target}")
        }
        Entry::Unlock {
            tick,
            plane,
            target,
        } => {
            format!("t={tick} data link: unlock plane={plane} target={target}")
        }
        Entry::Assign {
            tick,
            plane,
            target,
            by,
            order,
        } => {
            format!(
                "t={tick} data link: assign plane={plane} target={target} by={by} order={order:?}"
            )
        }
        Entry::Clear {
            tick,
            plane,
            target,
            why,
        } => {
            format!(
                "t={tick} data link: clear plane={plane} target={target} why={}",
                why.name()
            )
        }
        Entry::Acknowledge {
            tick,
            plane,
            target,
        } => {
            format!("t={tick} data link: acknowledge plane={plane} target={target}")
        }
        Entry::SortWarning {
            tick,
            plane,
            other,
            target,
        } => {
            format!("t={tick} data link: sort warning plane={plane} other={other} target={target}")
        }
    }
}

/// Deterministic headless AI probe (`--ai-probe-ticks`).
///
/// It builds the same chain a flown Quick Mission builds: the existing spawner
/// places the wings, `Combat::reset` puts them in the world, and the AI bridge
/// takes over from the targets it finds. Probe-only geometry, adapter and
/// threat overrides are explicit and recorded; normal mission defaults stay
/// separate. Each tick is `World::step`, the live game's whole tick, driven
/// by the scripted pilot and attack; the probe presents what it needs from the
/// tick's output, in the live order. It has no audio, HUD or rumble, and the
/// header says so. Returns the terrain, so a matrix reuses it.
///
/// `opinionated` (agent decision, 2026-09-17): the probe overwrites four setup
/// fields so both sides always have aircraft. Rule: the default draft populates
/// only enemy wing 1, which would leave the friendly-side and wingman paths
/// untested, and a probe that exercises one side is not evidence for two.
/// With `--ground-start` it also gives the player two wingmen (agent
/// decision, 2026-09-23), so the parked-wing path is exercised too.
#[allow(clippy::too_many_arguments)]
fn ai_probe_run(
    ticks: usize,
    quick: &mut quick_mission::QuickMission,
    hornet: &aircraft::Airframe,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    mut terrain: terrain::Terrain,
    enemy_skill: Option<tore_sim::ai::experience::EnemySkillOverride>,
    ai_mission: ai_wings::Preset,
    script: &ProbeScript,
    record: Option<&ProbeRecord>,
) -> AppResult<terrain::Terrain> {
    if script.redfor {
        if record.is_some() {
            return Err("--probe-redfor does not record a mission".into());
        }
        return redfor_probe_run(ticks, quick, resources, terrain, ai_mission, script);
    }
    // A flight starts on a fresh weather clock, as `reset_weather` gives a
    // flown one, so a terrain reused by the next matrix case starts clean.
    terrain.weather =
        tore_sim::environment::Environment::new(terrain.weather.configuration().clone());
    // The setup reads the terrain; the mission owns it once the loop starts.
    let world = &terrain;
    quick.draft.values[7] = if script.wing_only { 0 } else { 2 };
    quick.draft.values[8] = 1;
    quick.draft.values[21] = if script.wing_only { 0 } else { 2 };
    if quick.ground_runway().is_some() {
        quick.draft.values[4] = 3;
    }
    if let Some(size) = script.wing_size {
        quick.draft.values[4] = size;
    }
    quick.draft.values[22] = script.enemy_skill.unwrap_or(2);
    if script.ai_guns_only {
        quick.draft.values[19] = 0;
    }
    if let Some(enemy) = script.enemy_aircraft {
        let index = quick
            .ai_choice(enemy)
            .ok_or("probe enemy aircraft is not imported, or the AI cannot fly it")?;
        for field in [23, 26, 29] {
            quick.draft.values[field] = index;
        }
    }
    if let Some(friend) = script.friendly_aircraft {
        let index = quick
            .ai_choice(friend)
            .ok_or("probe friendly aircraft is not imported, or the AI cannot fly it")?;
        for field in [9, 12] {
            quick.draft.values[field] = index;
        }
    }
    if let Some((friendly, enemy)) = script.fight {
        // Three wings of up to five a side, the creator's own limit.
        for (fields, total) in [([4, 7, 10], friendly), ([21, 24, 27], enemy)] {
            for (n, field) in fields.into_iter().enumerate() {
                quick.draft.values[field] = total.saturating_sub(n * 5).min(5);
            }
        }
        quick.draft.values[11] = quick.draft.values[8];
        for field in [25, 28] {
            quick.draft.values[field] = quick.draft.values[22];
        }
    }
    for &(group, choice, survive) in &script.groups {
        let choices = quick_mission::QuickMission::objective_choices(group);
        let (label, objective) = choices
            .get(choice)
            .cloned()
            .ok_or("--probe-group choice is outside that group's objective list")?;
        quick.group_objectives[group] = objective;
        quick.group_must_survive[group] = survive;
        println!(
            "AI probe group: {} objective={label:?} survive={survive}",
            group + 1
        );
    }
    let wings = quick
        .wing_launches(enemy_skill)
        .map_err(|e| e.to_string())?;
    let mut combat = combat::Combat::new(hornet, resources, false)?;
    let mut combat_view = combat_view::CombatView::new(&combat, combat.own_id(), resources)?;
    combat.add_scene_targets(world)?;
    // The same launch layout a flown mission uses, including a ground start
    // when `--ground-start` chose a runway.
    let mut flight = hornet.start(world);
    let parked = match quick.ground_runway() {
        Some(object) => {
            flight.enable_research(1)?;
            Some(mission_layout::ground_layout(
                world,
                object,
                quick.player_wing_size(),
            )?)
        }
        None => None,
    };
    let layout = mission_layout::MissionLayout::plan(
        world,
        &flight,
        parked.clone(),
        &wings,
        quick.separation_feet(),
        quick.situation(),
    );
    if let Some(problem) = tore_world::mission_layout::availability::surface_problem(
        &world.surface,
        quick.separation_nm() as u32,
        parked.is_none(),
        false,
    ) {
        return Err(problem.into());
    }
    if layout.enemy.shortened() {
        return Err(tore_world::mission_layout::availability::distance_problem(
            quick.separation_nm() as u32,
        )
        .into());
    }
    combat_view.mission_aircraft(&mut combat, &wings, &layout, resources)?;
    let heading = match &parked {
        Some(ground) => {
            flight.position[0] = ground.slots[0][0];
            flight.position[2] = ground.slots[0][2];
            ground.heading
        }
        None => flight.yaw + layout.player_turn,
    };
    if heading != flight.yaw {
        flight.yaw = heading;
        let basis = attitude::Basis::new(heading, 0., 0.);
        flight.velocity =
            std::array::from_fn(|i| basis.forward[i] * flight.speed + world.wind()[i]);
    }
    combat.reset(&mut flight)?;
    combat.raise_airborne_spawns(world);
    if script.attack.is_some() {
        // A flown mission's weapon startup: the gun selected and armed in the
        // air, navigation mode on a ground start.
        combat.apply_startup_weapons();
        combat.state.own_mut().armed = parked.is_none();
    }
    if let Some(ground) = &parked {
        mission_layout::place_on_runway(world, &mut flight, ground, 0)?;
    }
    // An enemy ground start (`--enemy-ground-start`, slice CL-R) parks
    // Enemy Wing 1 at its Redfor field, as a flown mission does.
    let enemy_parked = match quick.enemy_ground_runway() {
        Some(object) => Some(mission_layout::ground_layout(
            world,
            object,
            quick.draft.values[21],
        )?),
        None => None,
    };
    let airfields = ai_wings::Airfields::from_world(
        world,
        parked.as_ref().map(mission_layout::GroundLayout::departure),
    )
    .with_enemy_departure(
        enemy_parked
            .as_ref()
            .map(mission_layout::GroundLayout::departure),
    );
    let mut bridge = ai_wings::AiWings::build_mission(
        &wings,
        &combat.state.targets,
        quick.guns_only(),
        resources,
        &airfields,
    )?;
    let formation_trace = formation_trace::start(&mut bridge)?;
    if !script.wing_route.is_empty() {
        let route = script
            .wing_route
            .iter()
            .map(|[east, north, altitude]| {
                [
                    flight.position[0] + east * 6_076.12,
                    *altitude,
                    flight.position[2] + north * 6_076.12,
                ]
            })
            .collect();
        bridge.set_wing_route(ai_wings::PLAYER_ID, route);
    }
    // The last line printed for each wing's mission of opportunity.
    let opportunity_notes: Vec<String> = Vec::new();
    combat.ai_poses = !bridge.is_empty();
    if script.researched && flight.research.is_none() {
        flight.enable_research(1)?;
    }
    let enemy_heading = match script.geometry {
        ProbeGeometry::Head => None,
        ProbeGeometry::Rear => Some(flight.yaw),
        ProbeGeometry::Side => Some(flight.yaw + std::f64::consts::FRAC_PI_2),
    };
    if let Some(pounds) = script.fuel_lbs {
        bridge.set_internal_fuel_all(pounds);
    }
    bridge.configure_probe(script.researched, enemy_heading, world.wind())?;
    bridge.set_flight_model(script.ai_flight_model)?;
    bridge.set_ai_test(script.ai_test);
    if !script.ai_test.is_default() {
        // Only a probe that asked for them says so: the default output is
        // the probe's output of before the settings existed.
        println!("AI probe test settings: {}", script.ai_test.describe());
    }
    if script.blind_wing {
        bridge.blind_wingmen(ai_wings::PLAYER_ID);
    }
    bridge.apply_mission_preset(ai_mission, flight.position);
    bridge.apply_group_objectives(&quick.group_objectives, flight.position);
    bridge.apply_group_survival(&quick.group_must_survive);
    bridge.mirror_pose_out(&mut combat.state.targets);
    println!(
        "AI probe: aircraft={} actors={} ticks={ticks} enemy_skill={enemy_skill:?} mission={ai_mission}",
        hornet.profile.name,
        bridge.len()
    );
    let comms = comms::Comms::new(1);
    let radio = radio_calls::Radio::default();
    let mut airfield_radio = airfield_radio::AirfieldRadio::default();
    airfield_radio.reset(parked.as_ref().map(|g| g.runway));
    let phrases = comms::phrases(resources);
    let heard = Vec::new();
    println!(
        "AI probe layout: airport={:?} ground={:?} runway_ft={:?} anchored={:?} spacing_ft={:?} enemy_nm={:.1} requested_nm={:.1} enemy_turn_deg={:.0}",
        parked.as_ref().and_then(|g| world
            .airport_scene
            .airports
            .iter()
            .find(|a| a.id == g.airport)
            .map(|a| a.name.as_str())),
        parked.as_ref().map(|g| g.object),
        parked.as_ref().map(|g| g.runway.length_ft.round()),
        parked.as_ref().map(|g| g.anchored),
        parked.as_ref().and_then(|g| g.spacing_ft),
        layout.enemy.distance_ft / mission_layout::FEET_PER_NM,
        layout.enemy.requested_ft / mission_layout::FEET_PER_NM,
        layout.enemy.turn.to_degrees()
    );
    if let Some(notice) = layout.notice() {
        println!("AI probe notice: {notice}");
    }
    let bounds = mission_layout::map_bounds(world);
    for slot in bridge.slots() {
        let Some(actor) = bridge.mission().actor(slot.id) else {
            continue;
        };
        if let Some(home) = actor.home_runway()
            && slot.side == tore_sim::ai::launch::Side::Friendly
            && slot.wing_number == 1
        {
            println!(
                "AI probe home: {} runway={} airport={:?} length_ft={:.0} vertical_pad={}",
                slot.label(),
                home.object,
                world
                    .airport_scene
                    .airports
                    .iter()
                    .find(|a| a.id == home.airport)
                    .map(|a| a.name.as_str()),
                home.length_ft,
                world.airport_scene.vertical_pad(home.object)
            );
        }
        let [x, y, z] = actor.flight().position;
        if !(bounds.min[0]..=bounds.max[0]).contains(&x)
            || !(bounds.min[1]..=bounds.max[1]).contains(&z)
        {
            println!("AI probe OFF-MAP start: {} x={x:.0} z={z:.0}", slot.label());
        }
        let ground = world.surface(x, z).height;
        if actor.ground_start().is_none() && y < ground {
            println!(
                "AI probe UNDERGROUND start: {} y={y:.0} ground={ground:.0}",
                slot.label()
            );
        }
    }
    // The tower service the player's Shift-A would use, with the departure
    // airport selected, so "land at selected airport" and the landing
    // priority go through the same rules as a flown mission.
    let mut service =
        tore_sim::airport::Service::new(&world.airport_scene).map_err(std::io::Error::other)?;
    // As a flown ground start sets it: navigation mode is on until the pilot
    // arms a weapon.
    let airport_nav_mode = parked.is_some();
    if let Some(ground) = &parked {
        service.command(
            &world.airport_scene,
            airport_aircraft(world, &flight, airport_nav_mode, false),
            tore_sim::airport::Command::SelectAirport(ground.airport),
        );
    }
    let scripted = script.takeoff && parked.is_some();
    if scripted {
        flight.brake_out = false;
        flight.throttle = 1.;
        flight.burner = flight
            .model()
            .configuration()
            .propulsion
            .afterburner_thrust_lbf
            > 0.;
    }
    let pilot = ProbePilot::new();
    let watch = ProbeWatch {
        trace: script.trace_ticks,
        ..Default::default()
    };
    let attacker = script.attack.map(|attack| {
        // As a flown mission does each frame: T and Enter skip friendlies.
        combat.state.own_mut().friendlies =
            bridge.friendly_ids(tore_sim::ai::launch::Side::Friendly);
        ProbeAttacker::new(attack, &combat, &bridge, script.guns)
    });
    if let Some(mode) = script.missile_release {
        if mode == 5 {
            flight.sensors.range_index = 5;
        }
        let source = if matches!(mode, 2 | 4) {
            "AIM9M.JT"
        } else {
            "AIM120.JT"
        };
        let selected = combat
            .state
            .own()
            .configuration()
            .stations
            .iter()
            .position(|s| s.weapon.source == source && s.count > 0)
            .ok_or("missile release probe needs a loaded AIM-120")?;
        combat.state.own_mut().selected = selected;
        combat.state.own_mut().armed = true;
        combat.state.own_mut().launch_mode = tore_sim::combat::missiles::LaunchMode::Boresight;
        if mode != 0 {
            let target = combat
                .state
                .targets
                .iter()
                .find(|t| t.hp > 0)
                .ok_or("close missile probe needs an enemy")?;
            let angle = match mode {
                3 => 15f64.to_radians(),
                4 => 6f64.to_radians(),
                _ => 0.,
            };
            let distance = match mode {
                2 => 3. * 6076.,
                3 => 8. * 6076.,
                4 => 3. * 6076.,
                5 => 100. * 6076.,
                _ => 0.2 * 6076.,
            };
            let forward = attitude::Basis::new(flight.yaw + angle, 0., 0.).forward;
            flight.position = std::array::from_fn(|i| target.position[i] - forward[i] * distance);
            if mode == 1 {
                flight.velocity = target.velocity;
            }
            println!(
                "missile release probe: target={} initial_range_ft={} min_ft={}",
                target.id,
                distance,
                combat.state.own().configuration().stations[selected]
                    .weapon
                    .seeker
                    .zones[1]
                    .minimum_range
            );
        }
    }
    // A mission recording of the probe: the picture is taken the way live
    // flight takes it, and nothing it reads feeds back into the run.
    let recording = match record {
        Some(record) => {
            combat.restart_render(combat.own_id(), &flight, Some(&bridge));
            let mut recording = start_probe_recording(
                record, &combat, &flight, &bridge, hornet, world, ticks, ai_mission, script,
            )?;
            recording.end(None, &mut combat);
            Some(recording)
        }
        None => None,
    };
    let verify = record.is_some_and(|r| r.verify);
    let mut pictures = Vec::new();
    // The released chaff and flares as each tick left them, once everything
    // released after its step had left, which is what a replay flies.
    let devices = Vec::new();
    if verify {
        pictures.push(combat.render_snapshot().clone());
    }
    let encounter: std::collections::BTreeMap<u32, ProbeEncounter> = Default::default();
    // The same seed the live game gives its turbulence when a flight starts.
    let mut turbulence_rng = tore_formats::flight_model::clock_rng::NativeRng::seeded(1)?;
    turbulence_rng.reseed_word(1);
    // The probe runs the live game's whole tick; only the presentation is its
    // own, below. It has no audio, HUD or rumble, so those cues are ignored.
    let flight_researched = flight.research.is_some();
    // The probe flies an accepted Quick Mission's start, so its mission result
    // and home checks run and send their calls as in the live game.
    let mission_start = (flight.position[1], flight.fuel);
    let result = ai_wings::outcome::Tracker::new(ai_wings::outcome::home_base(
        &terrain,
        parked.as_ref().map(|ground| ground.airport),
    ));
    let mission = world::World {
        choices: Default::default(),
        terrain,
        roster: seats::Roster::single_player(
            comms::crew(&hornet.profile),
            world::ai_planes(&bridge).collect::<Vec<_>>(),
        ),
        cockpits: vec![world::Cockpit {
            plane: seats::PlaneId(0),
            previous_flight: flight.clone(),
            flight,
            airport_service: service,
            airport_nav_mode,
            turbulence: Default::default(),
            turbulence_rng,
            airfield_radio,
            crew_voice: crew_voice::CrewVoice::new(&hornet.profile),
            result,
            overspeed_message_at: None,
            edge_message_at: None,
        }],
        combat,
        ai_wings: Some(bridge),
        comms,
        wing_status: Default::default(),
        datalink: Default::default(),
        score: None,
        revival: Default::default(),
        radio,
        phrases,
        // The probe builds its own mission; only a restart reads the setup,
        // and the tick reads whether there is a mission for its result calls.
        setup: world::Setup {
            mission: Some(mission_start),
            ground_start: parked.as_ref().map(|ground| ground.object),
            enemy_ground_start: None,
            researched_flight: flight_researched,
            native_tables: None,
            ai: None,
        },
    };
    probe_fly(
        ticks,
        script,
        ProbeFlight {
            mission,
            parked,
            scripted,
            pilot,
            watch,
            attacker,
            recording,
            verify,
            pictures,
            devices,
            encounter,
            formation_trace,
            opportunity_notes,
            heard,
        },
    )
}

/// `--probe-redfor`: the AI probe with its scripted leader on Redfor (slice
/// CL-R). The creator's draft (the player's aircraft and wing size, the
/// Redfor field `--ground-start` chose) becomes Enemy Wing 1 of an open
/// mission with `enemy-start ground` at that field, as a lobby's Redfor
/// ground start; a one-aircraft Blue wing holds its weapons at the
/// mission's start point. The probe's seat takes Enemy Wing 1's parked lead
/// by the handoff a networked player uses, and the same tick loop flies it.
/// Probe-only choices: the wing flies alone (`--probe-wing-only`) and both
/// sides hold their weapons, so nothing but the script moves the wing.
fn redfor_probe_run(
    ticks: usize,
    quick: &mut quick_mission::QuickMission,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    terrain: terrain::Terrain,
    ai_mission: ai_wings::Preset,
    script: &ProbeScript,
) -> AppResult<terrain::Terrain> {
    use tore_sim::ai::{engagement::GroupObjective, launch::Side};
    use tore_world::mission::{EnemyStart, Start, WingSpec};
    if !script.wing_only {
        return Err("--probe-redfor flies its wing alone: add --probe-wing-only".into());
    }
    if script.attack.is_some()
        || script.fight.is_some()
        || !script.threats.is_empty()
        || !script.wing_route.is_empty()
        || script.blind_wing
        || !script.groups.is_empty()
        || !script.player_locks.is_empty()
        || script.enemy_aircraft.is_some()
        || script.friendly_aircraft.is_some()
    {
        return Err("--probe-redfor takes the takeoff, wing size, wing order, lose-player and trace options only".into());
    }
    if let Some(size) = script.wing_size {
        quick.draft.values[4] = size;
    }
    let mut spec = quick.mission_spec()?;
    let runway = spec
        .ground_runway()
        .ok_or("--probe-redfor needs --ground-start at a Redfor field")?;
    let own = spec.wings[0];
    spec.wings[3] = own;
    spec.wings[0] = WingSpec { count: 1, ..own };
    for wing in [1, 2, 4, 5] {
        spec.wings[wing].count = 0;
    }
    spec.start = Start::Airborne {
        altitude_ft: spec.start.altitude_ft(),
    };
    spec.enemy_start = EnemyStart::Ground { runway };
    spec.objectives = [GroupObjective::Inherit; tore_world::mission::WINGS];
    spec.objectives[0] = GroupObjective::Hold;
    spec.objectives[3] = GroupObjective::Hold;
    spec.preset = ai_mission;
    spec.loadout = None;
    spec.researched_flight = true;
    spec.ai_flight_model = ai_wings::AiFlightModel::AllHybrid;
    // The mission builds its own terrain from the spec.
    drop(terrain);
    let mut mission = world::World::new(&spec, resources, world::Seating::Open)?;
    if let Some(wings) = mission.ai_wings.as_mut() {
        wings.set_ai_test(script.ai_test);
    }
    let lead = mission
        .roster
        .planes()
        .iter()
        .find(|plane| {
            plane.slot.wing.side == Side::Enemy
                && plane.slot.wing.index == 0
                && plane.slot.member == 0
        })
        .map(|plane| plane.id)
        .ok_or("the mission has no Enemy Wing 1 lead")?;
    mission.take_plane(SEAT, lead)?;
    let parked = mission
        .combat
        .mission_layout
        .as_ref()
        .and_then(|layout| layout.enemy_ground.clone());
    let scripted = script.takeoff && parked.is_some();
    let cockpit = &mut mission.cockpits[OWN];
    if scripted {
        let flight = &mut cockpit.flight;
        flight.brake_out = false;
        flight.throttle = 1.;
        flight.burner = flight
            .model()
            .configuration()
            .propulsion
            .afterburner_thrust_lbf
            > 0.;
        cockpit.previous_flight = flight.clone();
    }
    let world = &mission.terrain;
    let bridge = mission.ai_wings.as_ref().expect(PROBE_BRIDGE);
    println!(
        "AI probe: aircraft={} actors={} ticks={ticks} enemy_skill=None mission={ai_mission} seat=redfor plane={}",
        own.aircraft.selection_key(),
        bridge.len(),
        lead.0
    );
    println!(
        "AI probe layout: airport={:?} ground={:?} runway_ft={:?} anchored={:?} spacing_ft={:?}",
        parked.as_ref().and_then(|g| world
            .airport_scene
            .airports
            .iter()
            .find(|a| a.id == g.airport)
            .map(|a| a.name.as_str())),
        parked.as_ref().map(|g| g.object),
        parked.as_ref().map(|g| g.runway.length_ft.round()),
        parked.as_ref().map(|g| g.anchored),
        parked.as_ref().and_then(|g| g.spacing_ft),
    );
    for slot in bridge.slots() {
        if let Some(home) = bridge
            .mission()
            .actor(slot.id)
            .and_then(|actor| actor.home_runway())
            .filter(|_| slot.side == Side::Enemy && slot.wing_number == 1)
        {
            println!(
                "AI probe home: {} runway={} airport={:?} length_ft={:.0} vertical_pad={}",
                slot.label(),
                home.object,
                world
                    .airport_scene
                    .airports
                    .iter()
                    .find(|a| a.id == home.airport)
                    .map(|a| a.name.as_str()),
                home.length_ft,
                world.airport_scene.vertical_pad(home.object)
            );
        }
    }
    let formation_trace = formation_trace::start(mission.ai_wings.as_mut().expect(PROBE_BRIDGE))?;
    let watch = ProbeWatch {
        trace: script.trace_ticks,
        redfor: Some(lead.0),
        ..Default::default()
    };
    probe_fly(
        ticks,
        script,
        ProbeFlight {
            mission,
            parked,
            scripted,
            pilot: ProbePilot::new(),
            watch,
            attacker: None,
            recording: None,
            verify: false,
            pictures: Vec::new(),
            devices: Vec::new(),
            encounter: Default::default(),
            formation_trace,
            opportunity_notes: Vec::new(),
            heard: Vec::new(),
        },
    )
}

/// What the AI probe's tick loop flies, set up by [`ai_probe_run`]: the
/// single-player probe's own mission, or the Redfor seat's open one
/// ([`redfor_probe_run`]).
/// One offscreen frame of the probe's world around the AI plane
/// `--probe-render-plane` (slice H2's renders): `chase` just behind and
/// beside it, `wide` well back and to the side, `ground` from the ground
/// beside it looking up.
fn probe_render(
    mission: &mut world::World,
    script: &ProbeScript,
    dir: &std::path::Path,
    tick: u64,
    f12: Option<camera::Camera>,
) -> AppResult<()> {
    let flight = mission
        .ai_wings
        .as_ref()
        .expect(PROBE_BRIDGE)
        .mission()
        .actor(script.render_plane)
        .ok_or("--probe-render-plane is not an AI aircraft")?
        .flight()
        .clone();
    let p = flight.position;
    let (sin, cos) = flight.yaw.sin_cos();
    let forward = [sin, 0., cos];
    let right = [cos, 0., -sin];
    let (back, side, up) = match script.render_view.as_str() {
        "wide" => (420., 330., 90.),
        "ground" => (500., 350., 0.),
        _ => (260., 140., 40.),
    };
    let mut eye: tore_sim::attitude::Vector =
        std::array::from_fn(|i| p[i] - forward[i] * back + right[i] * side);
    let ground = mission.terrain.surface(eye[0], eye[2]).height;
    eye[1] = if script.render_view == "ground" {
        ground + 15.
    } else {
        (p[1] + up).max(ground + 15.)
    };
    let view = if script.render_view.is_empty() {
        "chase"
    } else {
        script.render_view.as_str()
    };
    let path = dir.join(format!(
        "t{tick:06}-plane{}-{view}.png",
        script.render_plane
    ));
    let observer = mission.cockpits[OWN].plane;
    a2g_run::ai_render::render_world(mission, observer, eye, p, &path, f12)?;
    println!("t={tick} probe: RENDER {}", path.display());
    Ok(())
}

struct ProbeFlight {
    mission: world::World,
    /// The probe wing's parked start: the scripted takeoff's runway.
    parked: Option<mission_layout::GroundLayout>,
    scripted: bool,
    pilot: ProbePilot,
    watch: ProbeWatch,
    attacker: Option<ProbeAttacker>,
    recording: Option<replay::recorder::Recorder>,
    verify: bool,
    pictures: Vec<tore_world::snapshot::RenderSnapshot>,
    devices: Vec<(u64, u64)>,
    encounter: std::collections::BTreeMap<u32, ProbeEncounter>,
    formation_trace: Option<formation_trace::FormationTrace>,
    opportunity_notes: Vec<String>,
    heard: Vec<String>,
}

/// The AI probe's tick loop and summary lines.
fn probe_fly(ticks: usize, script: &ProbeScript, run: ProbeFlight) -> AppResult<terrain::Terrain> {
    let ProbeFlight {
        mut mission,
        parked,
        scripted,
        mut pilot,
        mut watch,
        mut attacker,
        mut recording,
        verify,
        mut pictures,
        mut devices,
        mut encounter,
        mut formation_trace,
        mut opportunity_notes,
        mut heard,
    } = run;
    let mut output = world::TickOutput::default();
    let mut invariants = probe_invariants::ProbeInvariants::default();
    invariants.report_pavement = script.pavement;
    if let Some(path) = &script.track {
        invariants.track = Some(std::io::BufWriter::new(std::fs::File::create(path)?));
    }
    // CAP-only diagnostics keep ordinary probe output stable.
    let mut patrol = std::collections::BTreeMap::<u32, (Option<u64>, u64, f64)>::new();
    let mut f12_rig = flight_views::Rig::default();
    let capture_f12 = script.render.is_some() && std::env::var_os("TORE_SCENE_F12").is_some();
    // Each AI aircraft's damage before any crash: a ground impact sets the
    // flight's damage to 1, so the final value cannot tell an undamaged
    // aircraft that flew into the ground from one that was hit first.
    let mut damage_before_crash = std::collections::BTreeMap::<u32, f64>::new();
    for tick in 0..ticks as u64 {
        if let Some(recording) = &mut recording {
            recording.start_tick(None, &mut mission.combat);
        }
        if script.lose_player == Some(tick) {
            mission.cockpits[OWN].flight.crashed = true;
            println!("t={tick} probe: the player is lost on purpose");
        }
        let mut keys = flight::PilotInput::default();
        if scripted {
            pilot.fly(
                tick,
                &mut mission.cockpits[OWN].flight,
                &mut keys,
                &mission.terrain,
                parked.as_ref(),
                script,
            );
        }
        let mut commands = Vec::new();
        if let Some(attacker) = &mut attacker {
            attacker.aim(
                tick,
                &mission.combat,
                &mission.cockpits[OWN].flight,
                mission.ai_wings.as_ref().expect(PROBE_BRIDGE),
                &mut commands,
            );
        }
        if ((matches!(script.missile_release, Some(2 | 5)) && tick == 1)
            || (script.missile_release == Some(4) && tick == 59))
            && let Some(target) = mission.combat.state.targets.first()
        {
            commands.push(seats::SeatCommand::Combat(
                tore_sim::combat::live::Command::DesignateTarget(target.id),
            ));
        }
        let release_tick = if script.missile_release == Some(5) {
            600
        } else {
            60
        };
        if script.missile_release.is_some() && (tick == release_tick || tick == release_tick + 1) {
            commands.push(trigger_key(tick == release_tick));
            if tick == release_tick {
                let own = mission.combat.state.own();
                let readout = mission.combat.state.own_view();
                let launcher = combat::launcher(&mission.cockpits[OWN].flight);
                println!("missile release tone: {:?}", readout.seeker_tone(launcher));
                println!(
                    "missile release probe: fire tick={release_tick} mode={:?} designated={:?} readiness={:?} rounds={}",
                    own.launch_mode,
                    readout.designated(),
                    readout.readiness(launcher),
                    own.rounds(own.selected)
                );
            }
        }
        // The script's wing orders, given after the leader's own controls.
        for (_, order, member) in script.orders.iter().filter(|(at, ..)| *at == tick) {
            // An order to one member addresses it as Alt+Shift+N does, then
            // goes back to the whole wing.
            if member.is_some() {
                commands.push(seats::SeatCommand::WingRecipient(*member));
            }
            commands.push(seats::SeatCommand::WingOrder(*order));
            if member.is_some() {
                commands.push(seats::SeatCommand::WingRecipient(None));
            }
        }
        // The player's designations: a lock follows once the sensors hold it.
        commands.extend(
            script
                .player_locks
                .iter()
                .filter(|(at, _)| *at == tick)
                .map(|(_, id)| {
                    seats::SeatCommand::Combat(tore_sim::combat::live::Command::DesignateTarget(
                        *id,
                    ))
                }),
        );
        for (ordinal, (_, threat)) in script
            .threats
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| *at == tick)
        {
            inject_probe_threat(
                *threat,
                ordinal,
                mission.ai_wings.as_mut().expect(PROBE_BRIDGE),
                &mut mission.combat,
                &mission.terrain,
            )?;
            if let Some(recording) = &mut recording {
                recording.note(
                    tore_replay::Event::new(tore_replay::vocab::kind::SYSTEM_NOTE)
                        .with_text(format!("controlled probe threat: {threat:?}")),
                );
            }
        }
        if verify {
            devices.push((
                mission.combat.state.tick(),
                replay::devices::digest(&mission.combat.state.devices),
            ));
        }
        let input = seats::SeatInput {
            seat: SEAT,
            tick: mission.tick(),
            pilot: keys,
            commands,
            ..Default::default()
        };
        let stepped =
            mission.step_observed(std::slice::from_ref(&input), &mut output, |world, out| {
                if let Some(attacker) = &mut attacker {
                    attacker.commands_applied(tick, &world.combat);
                }
                // What became of the script's wing orders.
                for reply in out.orders.iter().filter(|reply| reply.seat == SEAT) {
                    let order = reply.order;
                    match &reply.outcome {
                        world::OrderOutcome::Given { message } => {
                            println!("t={tick} order={order:?} reply={message:?}");
                        }
                        world::OrderOutcome::Refused { message } => {
                            println!("t={tick} order={order:?} refused: {message}");
                        }
                        world::OrderOutcome::Failed { message } => {
                            return Err(message.clone().into());
                        }
                    }
                }
                Ok(())
            });
        if let Some(wings) = &mut mission.ai_wings {
            formation_trace::drain(&mut formation_trace, wings);
        }
        stepped?;
        if let Some(wings) = &mission.ai_wings {
            print_opportunity_notes(tick, wings, &mut opportunity_notes);
            for actor in
                wings.mission().actors().iter().filter(|a| {
                    a.assignment().role == tore_sim::ai::engagement::Role::CombatAirPatrol
                })
            {
                let stats = patrol.entry(actor.id()).or_default();
                if actor.controller().target().is_some() {
                    stats.0.get_or_insert(tick);
                }
                stats.1 += wings
                    .last_output()
                    .launches
                    .iter()
                    .filter(|launch| launch.actor == actor.id())
                    .count() as u64;
                if let Some(region) = actor.assignment().patrol {
                    let position = actor.flight().position;
                    stats.2 = stats.2.max(
                        (position[0] - region.center_ft[0])
                            .hypot(position[2] - region.center_ft[2]),
                    );
                }
            }
        }
        // The data link's journal is drained once: for the probe's lines,
        // the recording, or both.
        let link_entries = if script.data_link || recording.is_some() {
            mission.datalink.take_journal()
        } else {
            Vec::new()
        };
        if script.data_link {
            for entry in &link_entries {
                println!("{}", data_link_line(entry));
            }
        }
        if let Some(error) = &output.fault {
            return Err(error.clone().into());
        }
        // Present the tick in the order the live game does.
        let now = mission.combat.state.tick() as f64 / 120.;
        let mut ejected = Vec::new();
        for cue in &output.cues {
            match cue {
                world::Cue::WingEjection {
                    id,
                    message,
                    friendly,
                } => {
                    if let Some(recording) = &mut recording {
                        recording.wing_ejection(*id, message, *friendly);
                    }
                    ejected.push(message.as_str());
                }
                world::Cue::Picture => {
                    let bridge = mission.ai_wings.as_ref().expect(PROBE_BRIDGE);
                    if let Some(attacker) = &mut attacker {
                        attacker.events(tick, &output.events, &mission.combat, bridge);
                    }
                    for actor in bridge
                        .mission()
                        .actors()
                        .iter()
                        .filter(|a| a.identity().side == ai_wings::ENEMY_SIDE)
                    {
                        let stats = encounter.entry(actor.id()).or_default();
                        if actor
                            .awareness()
                            .current_observations()
                            .any(|s| s.target.id == 0 && s.source_ticks.visual == Some(tick))
                        {
                            stats.visual.get_or_insert(tick);
                        }
                        if !actor.is_neutral() {
                            stats.engage.get_or_insert(tick);
                        }
                        if actor.trace().fire.selected {
                            stats.defense.get_or_insert(tick);
                        }
                        stats.bank = stats.bank.max(actor.flight().bank.to_degrees().abs());
                    }
                    if script.trace_ticks > 0 && tick % script.trace_ticks == 0 {
                        for actor in bridge
                            .mission()
                            .actors()
                            .iter()
                            .filter(|a| a.identity().side != ai_wings::FRIENDLY_SIDE)
                        {
                            let seen = actor
                                .awareness()
                                .current_observations()
                                .find(|s| s.target.id == 0);
                            println!(
                                "AI perception: tick={tick} actor={} player_sources={:?} neutral={} target={:?} activity={:?} fire={:?}",
                                actor.id(),
                                seen.map(|s| s.source_ticks),
                                actor.is_neutral(),
                                actor.controller().target(),
                                actor.activity(),
                                actor.incoming_fire_cue()
                            );
                        }
                    }
                    if script.fuel_trace_ticks > 0 && tick % script.fuel_trace_ticks == 0 {
                        // Every actor's fuel judgement (B48), for reading the
                        // rotorcraft schedule (slice H3).
                        for actor in bridge.mission().actors().iter() {
                            if let Some(fuel) = actor.controller().trace().fuel {
                                println!(
                                    "AI fuel: tick={tick} actor={} aircraft={:?} fuel_lb={:.0} endurance_s={:.0} time_home_s={} state={:?} activity={:?}",
                                    actor.id(),
                                    actor.identity().aircraft,
                                    fuel.internal_fuel_lbs,
                                    fuel.endurance_s,
                                    fuel.time_home_s
                                        .map_or("none".to_owned(), |t| format!("{t:.0}")),
                                    fuel.state,
                                    actor.activity()
                                );
                            }
                        }
                    }
                    if script.rotor_trace_ticks > 0 && tick % script.rotor_trace_ticks == 0 {
                        // Every AI rotorcraft's flight and the rotor adapter's
                        // command (slice HF1).
                        for actor in bridge.mission().actors().iter() {
                            let f = actor.flight();
                            if !tore_sim::ai::rotor_adapter::flies(f) || f.crashed {
                                continue;
                            }
                            let fly = actor.trace().fly.as_ref();
                            let rotor = fly.and_then(|t| t.rotor);
                            let intent = fly.and_then(|t| t.intent);
                            let ground =
                                mission.terrain.surface(f.position[0], f.position[2]).height;
                            println!(
                                "AI rotor: tick={tick} actor={} activity={:?} agl={:.0} kt={:.0} vs={:.1} pitch={:.1} bank={:.1} hdg={:.0} nacelle={:.0} collective={:.2} nr={:.2} regime={:?} cmd_hdg={} cmd_path={} cmd_kt={} cmd_climb={} cmd_nacelle={} intent_path={} floor={:?} max_kt={:.0} course={:.0} along_kt={:.0}",
                                actor.id(),
                                actor.activity(),
                                f.position[1] - ground,
                                f.speed / tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT,
                                f.vertical_speed,
                                f.pitch.to_degrees(),
                                f.bank.to_degrees(),
                                f.yaw.to_degrees().rem_euclid(360.),
                                f.nacelle_degrees(),
                                f.lift_controls.collective,
                                f.lift_controls.drive.rotor_speed,
                                rotor.map(|r| r.regime),
                                rotor.map_or("-".to_owned(), |r| format!("{:.0}", r.heading_deg)),
                                rotor.map_or("-".to_owned(), |r| format!(
                                    "{:.1}",
                                    r.flight_path_pitch_deg
                                )),
                                rotor.map_or("-".to_owned(), |r| format!(
                                    "{:.0}",
                                    r.speed_fps / tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT
                                )),
                                rotor.map_or("-".to_owned(), |r| format!("{:.1}", r.climb_fps)),
                                rotor
                                    .and_then(|r| r.nacelle_deg)
                                    .map_or("-".to_owned(), |n| format!("{n:.0}")),
                                intent.map_or("-".to_owned(), |i| format!(
                                    "{:.1}",
                                    i.flight_path_pitch_deg
                                )),
                                fly.and_then(|t| t.terrain_floor_deg)
                                    .map(|d| (d * 10.).round() / 10.),
                                actor.speed_limits().maximum.0
                                    / tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT,
                                f.velocity[0]
                                    .atan2(f.velocity[2])
                                    .to_degrees()
                                    .rem_euclid(360.),
                                tore_sim::autopilot::rotor_law::speed_along(f)
                                    / tore_sim::runway_wind::FEET_PER_SECOND_PER_KNOT,
                            );
                        }
                    }
                    if let Some(attacker) = &mut attacker {
                        attacker.observe(tick, bridge, &ejected);
                    }
                    if let Some(recording) = &mut recording {
                        if verify {
                            verify_probe_attitudes(mission.combat.render_snapshot(), bridge)?;
                        }
                        let idle = flight::PilotInput::default();
                        let others =
                            other_crews(&mission.cockpits, mission.cockpits[OWN].plane, &idle);
                        recording.begin(replay::recorder::Tick {
                            snapshot: mission.combat.render_snapshot(),
                            combat: &mission.combat,
                            flight: &mission.cockpits[OWN].flight,
                            previous: &mission.cockpits[OWN].previous_flight,
                            pilot: &input.pilot,
                            others: &others,
                            wings: Some(bridge),
                            world: &mission.terrain,
                            events: &output.events,
                            outcomes: &output.outcomes,
                            journal: output.journal.as_ref(),
                        });
                    }
                }
                world::Cue::Radio { seat, call } if *seat == SEAT => {
                    heard.push(format!("{now:.1}s {} {:?}", call.line(), call.stems));
                }
                _ => {}
            }
        }
        if let Some(recording) = &mut recording {
            // Write-only: every communication decision of the tick.
            recording.drain_comms(&mut mission.comms);
            recording.datalink_drained(mission.datalink.journal_lost(), &link_entries);
            let releases = seat_releases(&mission, SEAT, &output.releases);
            recording.sounds(&output.emissions, &releases);
            recording.end(None, &mut mission.combat);
            if verify {
                pictures.push(mission.combat.render_snapshot().clone());
            }
        }
        watch.observe(
            tick,
            mission.ai_wings.as_ref().expect(PROBE_BRIDGE),
            &mission.cockpits[OWN].flight,
            &mission.terrain,
        );
        let f12 = capture_f12.then(|| a2g_run::ai_render::f12_camera(&mission, &mut f12_rig));
        if let Some(dir) = &script.render
            && script.render_at.contains(&tick)
        {
            probe_render(&mut mission, script, dir, tick, f12)?;
        }
        invariants.observe(
            tick,
            mission.ai_wings.as_ref().expect(PROBE_BRIDGE),
            &mission.combat,
            &mission.terrain,
        );
        for actor in mission
            .ai_wings
            .as_ref()
            .expect(PROBE_BRIDGE)
            .mission()
            .actors()
        {
            if !actor.flight().crashed {
                damage_before_crash.insert(actor.id(), actor.flight().damage_fraction);
            }
        }
    }
    let world = &mission.terrain;
    let flight = &mission.cockpits[OWN].flight;
    let combat = &mission.combat;
    let bridge = mission.ai_wings.as_ref().expect(PROBE_BRIDGE);
    watch.summary();
    println!(
        "player crashed={} gear={} x={:.1} y={:.1} z={:.1} hdg={:.1}",
        flight.crashed,
        flight.gear_down,
        flight.position[0],
        flight.position[1],
        flight.position[2],
        flight.yaw.to_degrees()
    );
    for line in bridge.probe_lines() {
        println!("{line}");
    }
    for actor in bridge.mission().actors() {
        let flight = actor.flight();
        let faults: Vec<_> = flight
            .systems
            .counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| (index, *count))
            .collect();
        let damage = damage_before_crash
            .get(&actor.id())
            .copied()
            .unwrap_or(flight.damage_fraction);
        if !faults.is_empty() || damage > 0.0 {
            println!(
                "AI damage: actor={} faults={faults:?} return={:?} landing={:?} throttle={:.3} power={:.3} oil={:.3} temp={:.3} wounded={} escaped={} damage_fraction={}",
                actor.id(),
                actor.damage_return(),
                actor.airfield_phase(),
                flight.throttle,
                flight.systems.power_available(),
                flight.systems.oil_pressure(),
                flight.systems.engine.temperature,
                flight.systems.pilot.wounded(),
                flight.escape.is_some(),
                damage
            );
        }
        // What each aircraft still carries: AI jettison evidence
        // (docs/spec/jettison.md). Output only; no decision reads it.
        if let Some(config) = bridge.configuration(actor.id()) {
            let stores: Vec<String> = actor
                .stations()
                .iter()
                .filter_map(|s| {
                    let station = config.stations.get(usize::from(s.station.0))?;
                    let rounds = match s.rounds() {
                        tore_sim::ai::weapon_service::Rounds::Finite(n) => n.to_string(),
                        tore_sim::ai::weapon_service::Rounds::Unlimited => "inf".into(),
                    };
                    Some(format!("{} x{rounds}", station.weapon.hud_name))
                })
                .collect();
            let tanks: u32 = config.tanks.iter().map(|t| u32::from(t.quantity)).sum();
            println!(
                "AI stores: actor={} tanks={tanks} external_fuel={:.0} payload={:.0} stations=[{}]",
                actor.id(),
                flight.systems.external_lbs(),
                flight.payload_lbs,
                stores.join(", ")
            );
        }
    }
    let report = debrief::capture(&mission, SEAT).expect("the probe's seat flies a plane");
    println!("AI probe debrief: {}", report.summary());
    println!(
        "AI probe radio: calls={} heard={}",
        mission.radio.made, mission.radio.heard
    );
    for line in heard.iter().take(40) {
        println!("  {line}");
    }
    if let Some(attacker) = &attacker {
        attacker.summary(combat, flight, bridge);
    }
    // A single number that changes if any actor's path changes, so two runs can
    // be compared without diffing every coordinate.
    let checksum = bridge
        .positions()
        .iter()
        .flatten()
        .fold(0u64, |acc, v| acc.rotate_left(7) ^ v.to_bits());
    for (
        id,
        ProbeEncounter {
            visual,
            engage,
            defense,
            bank,
        },
    ) in encounter
    {
        println!(
            "AI probe encounter: actor={id} first_visual={visual:?} first_engage={engage:?} first_gun_defense={defense:?} peak_bank_deg={bank:.2}"
        );
    }
    for (id, (engage, shots, radius)) in patrol {
        println!(
            "AI probe CAP: actor={id} first_target_tick={engage:?} shots={shots} max_station_nm={:.3}",
            radius / 6076.
        );
    }
    println!(
        "AI probe totals: wings={} ticks={} shots={} dropped={} warnings={} live_projectiles={} player_hp={} target_hp={:?} checksum={checksum:016x}",
        bridge.slots().len(),
        bridge.mission().tick(),
        bridge.realised_launches,
        bridge.dropped_launches,
        bridge.threat_reports().len(),
        combat.state.projectiles.len(),
        combat.state.own().hp,
        combat
            .state
            .targets
            .iter()
            .map(|t| t.hp)
            .collect::<Vec<_>>()
    );
    invariants.summary(bridge);
    if let Some(mut recording) = recording {
        recording.note(
            tore_replay::Event::new(tore_replay::vocab::kind::SYSTEM_END)
                .with(tore_replay::vocab::field::REASON, "probe finished"),
        );
        let footer = replay_footer(combat, recording.player(), Some(&report), "probe finished");
        let path = recording
            .finish(&footer)
            .ok_or("the mission recording could not be finished; see the session log")?;
        if verify {
            devices.push((
                combat.state.tick(),
                replay::devices::digest(&combat.state.devices),
            ));
            let verification = replay::cli::verify(&path, &pictures, &devices, world)?;
            println!("AI probe {}", verification.line());
        }
    }
    Ok(mission.terrain)
}

/// Starts `--record-mission` for an AI probe, with the probe's settings and
/// what it does not simulate in the header, and records the start.
#[allow(clippy::too_many_arguments)]
fn start_probe_recording(
    record: &ProbeRecord,
    combat: &combat::Combat,
    flight: &flight::State,
    bridge: &ai_wings::AiWings,
    hornet: &aircraft::Airframe,
    world: &terrain::Terrain,
    ticks: usize,
    ai_mission: ai_wings::Preset,
    script: &ProbeScript,
) -> AppResult<replay::recorder::Recorder> {
    use replay::{convert, recorder};
    let snapshot = combat.render_snapshot();
    let mut extra = vec![
        (
            "probe".to_owned(),
            "headless AI probe on the full mission tick: no audio, music, HUD or rumble".to_owned(),
        ),
        ("probe.ticks".into(), ticks.to_string()),
        (
            "flight_model".into(),
            if flight.research.is_some() {
                "researched"
            } else {
                "legacy"
            }
            .into(),
        ),
        (
            "player.aircraft".into(),
            convert::identity_key(hornet.profile.id).into(),
        ),
        ("ai.mission".into(), ai_mission.to_string()),
        ("ai.aircraft".into(), bridge.len().to_string()),
    ];
    if script.takeoff {
        extra.push(("probe.script".into(), "takeoff".into()));
    }
    if let Some(attack) = script.attack {
        extra.push((
            "probe.attack".into(),
            format!("from tick {} repeat {} ticks", attack.from, attack.repeat),
        ));
    }
    for (tick, order, member) in &script.orders {
        extra.push((
            "probe.order".into(),
            match member {
                Some(member) => format!("tick {tick}: {order:?} to member {member}"),
                None => format!("tick {tick}: {order:?}"),
            },
        ));
    }
    for (tick, id) in &script.player_locks {
        extra.push((
            "probe.player_lock".into(),
            format!("tick {tick}: designate {id}"),
        ));
    }
    extra.push(("probe.geometry".into(), format!("{:?}", script.geometry)));
    extra.push(("probe.guns".into(), script.guns.to_string()));
    extra.push(("probe.ai_guns_only".into(), script.ai_guns_only.to_string()));
    if let Some(enemy) = script.enemy_aircraft {
        extra.push(("probe.enemy".into(), enemy.selection_key().into()));
    }
    for (tick, threat) in &script.threats {
        extra.push(("probe.threat".into(), format!("tick {tick}: {threat:?}")));
    }
    let header = recorder::header(
        tore_replay::MissionKind::Probe,
        world,
        &convert::Presentation::of(snapshot),
        extra,
        std::time::SystemTime::now(),
    );
    // The probe has one human, seat 0 in plane 0.
    let player = recorder::Human::single_player(&hornet.profile.name, true);
    let roster = recorder::roster(snapshot, &player, &[], Some(bridge), combat.dummy_types());
    let mut recording = recorder::Recorder::start(record.path.clone(), &header, &roster)
        .map_err(|error| format!("--record-mission {}: {error}", record.path.display()))?
        .for_seat(SEAT, player.id);
    // A probe has no frame rate to protect: wait for the writer rather than
    // drop ticks when the machine is busy.
    recording.wait_for_writer();
    recording.begin(recorder::Tick {
        snapshot,
        combat,
        flight,
        previous: flight,
        pilot: &flight::PilotInput::default(),
        others: &[],
        wings: Some(bridge),
        world,
        events: &[],
        outcomes: &[],
        journal: None,
    });
    Ok(recording)
}

/// What the app should do when there is no usable pack in application data.
/// Kept free of the file system so the decision itself can be tested.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// The pack loaded; start the game.
    Play,
    /// A source the app already knows: import it without asking.
    ImportNow(PathBuf),
    /// Show the locate screen, with this path in the field if there is one.
    Ask(Option<PathBuf>),
    /// No window and nothing to import from: report it in the terminal.
    Fail,
}

/// `known` is the remembered source or the developer checkout, whichever
/// detects first; `prefill` is the first automatically detected source.
fn next_step(
    loaded: bool,
    interactive: bool,
    known: Option<PathBuf>,
    prefill: Option<PathBuf>,
) -> Step {
    match (loaded, known, interactive) {
        (true, ..) => Step::Play,
        (false, Some(path), _) => Step::ImportNow(path),
        (false, None, true) => Step::Ask(prefill),
        (false, None, false) => Step::Fail,
    }
}

/// The path the shell imports without waiting for the player: a source the app
/// already knows, or, under `--smoke-test`, whatever the field was prefilled
/// with, so a first run can be checked end to end without clicks.
fn auto_import_path(step: &Step, prefill: Option<&Path>, smoke_test: bool) -> Option<PathBuf> {
    match step {
        Step::ImportNow(path) => Some(path.clone()),
        _ if smoke_test => prefill.map(Path::to_path_buf),
        _ => None,
    }
}

/// How a session of the game application ended.
enum Outcome {
    Done,
    /// Pref asked for another import. The frame is the menu the player was
    /// looking at, which the locate screen draws over.
    Reimport(Vec<u8>),
}

/// What a session starts from.
enum Session {
    First,
    Reimport(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellOutcome {
    Quit,
    Continue,
}

/// Progress and results from the importing worker thread. The error is already
/// a plain string, because `Box<dyn Error>` cannot cross a thread boundary.
enum ImportMessage {
    Progress(assets::Progress),
    Done(Vec<String>),
    Failed(String),
}

/// The pre-game shell: the locate screen, its own blit-only presenter, and the
/// import running on a worker thread. It is a second winit application handler
/// because every game object is built from imported assets, so the game `App`
/// cannot exist before an import does.
struct LocateShell {
    locate: locate::Locate,
    data: PathBuf,
    presenter: Option<canvas_present::CanvasPresenter>,
    font: menu::Sprite,
    small: menu::Sprite,
    background: Option<Vec<u8>>,
    pixels: Vec<u8>,
    worker: Option<std::sync::mpsc::Receiver<ImportMessage>>,
    /// When the starting bar last moved, so pointer motion cannot speed it up.
    last_tick: Instant,
    /// Last pointer position in canvas coordinates: winit reports a button
    /// press without one, so the latest motion is what a click uses.
    last_pointer: Option<(f64, f64)>,
    /// Imported as soon as the window is up, without waiting for the player.
    auto_import: Option<PathBuf>,
    /// `--smoke-test` continues as soon as the import finishes.
    auto_continue: bool,
    outcome: ShellOutcome,
    error: Option<Box<dyn Error>>,
    /// Live window mode, carried back to the game window so one Alt-Enter in
    /// the shell holds for the rest of the session.
    fullscreen: bool,
    /// Alt state, which winit reports separately from the key event.
    modifiers: ModifiersState,
    /// When to draw again after a present the surface gave no image for
    /// (`CanvasPresenter::surface_retry`).
    retry_at: Option<Instant>,
}

/// winit key names the locate screen understands. Printable characters go
/// through `text_input` instead, so their case survives.
fn locate_key_name(key: &Key) -> Option<&'static str> {
    use winit::keyboard::NamedKey;
    let Key::Named(named) = key else {
        return None;
    };
    Some(match named {
        NamedKey::Enter => "Enter",
        NamedKey::Escape => "Escape",
        NamedKey::Tab => "Tab",
        NamedKey::Backspace => "Backspace",
        NamedKey::Delete => "Delete",
        NamedKey::Home => "Home",
        NamedKey::End => "End",
        NamedKey::ArrowUp => "ArrowUp",
        NamedKey::ArrowDown => "ArrowDown",
        NamedKey::ArrowLeft => "ArrowLeft",
        NamedKey::ArrowRight => "ArrowRight",
        _ => return None,
    })
}

impl LocateShell {
    fn redraw(&mut self) {
        if let Some(presenter) = &self.presenter {
            presenter.window.request_redraw();
        }
    }
    fn settle(&mut self, event_loop: &ActiveEventLoop, event: locate::Event) {
        match event {
            locate::Event::None => {}
            locate::Event::Import(path) => self.start_import(path),
            locate::Event::Quit => {
                self.outcome = ShellOutcome::Quit;
                event_loop.exit();
            }
            locate::Event::Continue => {
                self.outcome = ShellOutcome::Continue;
                event_loop.exit();
            }
            // Every route from this screen into the game builds a new renderer
            // and instance after it, so the choice applies without a restart.
            locate::Event::ChooseApi(api) => {
                graphics_api::choose(api);
                self.locate.set_graphics_api(Some(api));
                self.locate.set_hint(format!(
                    "The game will use {} when it starts. No restart needed.",
                    api.label()
                ));
            }
        }
        self.redraw();
    }
    /// Classify the chosen path, then read it on a worker thread so the screen
    /// keeps drawing. Nothing partial is kept: a failure leaves the old pack.
    fn start_import(&mut self, path: PathBuf) {
        if self.worker.is_some() {
            return;
        }
        log::info!("Import source: {}", path.display());
        let source = match media_source::MediaSource::detect(&path) {
            Ok(source) => source,
            Err(error) => {
                log::warn!("Import source detection failed: {error}");
                self.locate.set_phase(locate::Phase::Failed {
                    reason: error.to_string(),
                });
                return;
            }
        };
        self.locate.clear_hint();
        // Say so at once: the worker reads nothing countable until it has
        // identified the build and found what to read.
        self.locate.set_phase(locate::Phase::Starting {
            step: String::from("Opening the game files"),
        });
        let (sender, receiver) = std::sync::mpsc::channel();
        let data = self.data.clone();
        std::thread::spawn(move || {
            log::info!("Import worker started");
            let reports = sender.clone();
            let result = Assets::import_with_progress(&source, &data, &mut |progress| {
                // Archive changes are bounded progress checkpoints, not per-resource logging.
                match &progress {
                    assets::Progress::Preparing(step) => log::info!("Import step: {step}"),
                    assets::Progress::Reading {
                        archive, done: 0, ..
                    } => {
                        log::info!("Import archive: {archive}")
                    }
                    assets::Progress::Reading { .. } => {}
                }
                let _ = reports.send(ImportMessage::Progress(progress));
            });
            let _ = sender.send(match result {
                // The decoded assets are dropped here; the caller reloads the
                // pack that was just written, which proves it reads back.
                Ok(outcome) => {
                    log::info!("Import worker completed successfully");
                    ImportMessage::Done(outcome.summary)
                }
                Err(error) => {
                    log::error!("Import failed: {error}");
                    ImportMessage::Failed(error.to_string())
                }
            });
        });
        self.worker = Some(receiver);
    }
    /// Drain the worker channel. Returns true when the screen changed.
    fn poll_import(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(receiver) = self.worker.take() else {
            return false;
        };
        let mut changed = false;
        let mut running = true;
        loop {
            match receiver.try_recv() {
                Ok(ImportMessage::Progress(assets::Progress::Preparing(step))) => {
                    self.locate.set_phase(locate::Phase::Starting {
                        step: step.to_owned(),
                    });
                    changed = true;
                }
                Ok(ImportMessage::Progress(assets::Progress::Reading {
                    archive,
                    done,
                    total,
                })) => {
                    self.locate.set_phase(locate::Phase::Importing {
                        archive,
                        resources_done: done,
                        resources_total: total,
                    });
                    changed = true;
                }
                Ok(ImportMessage::Done(summary)) => {
                    self.locate.set_phase(locate::Phase::Done { summary });
                    changed = true;
                    running = false;
                }
                Ok(ImportMessage::Failed(reason)) => {
                    self.locate.set_phase(locate::Phase::Failed { reason });
                    changed = true;
                    running = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if running {
            self.worker = Some(receiver);
        } else if self.auto_continue && matches!(self.locate.phase(), locate::Phase::Done { .. }) {
            self.outcome = ShellOutcome::Continue;
            event_loop.exit();
        }
        changed
    }
}

impl ApplicationHandler for LocateShell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.presenter.is_some() {
            return;
        }
        let result = (|| {
            diagnostics::stage("first-run window creation");
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("T.O.R.E-Fighters - Locate Fighters Anthology")
                        .with_inner_size(LogicalSize::new(960.0, 720.0))
                        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
                        .with_fullscreen(fullscreen_attribute(self.fullscreen)),
                )?,
            );
            diagnostics::stage_done();
            pollster::block_on(canvas_present::CanvasPresenter::new(window))
        })();
        match result {
            Ok(presenter) => {
                presenter.window.request_redraw();
                self.presenter = Some(presenter);
                if let Some(path) = self.auto_import.take() {
                    self.start_import(path);
                }
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(presenter) = self.presenter.as_mut() else {
            return;
        };
        if presenter.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.outcome = ShellOutcome::Quit;
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.last_pointer = None;
                presenter.resize();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::DroppedFile(path) => {
                // A dropped file or folder is classified before it reaches the
                // field, so the field always holds a folder the importer can read.
                match media_source::MediaSource::detect(&path) {
                    Ok(source) => self.locate.dropped_path(source.path),
                    Err(error) => {
                        self.locate.dropped_path(path);
                        self.locate.set_hint(error.to_string());
                    }
                }
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // The game's quit shortcuts work here too, so the screen can
                // be left from the keyboard where the desktop has no binding.
                let quit_key = match &event.logical_key {
                    Key::Named(winit::keyboard::NamedKey::F4) => self.modifiers.alt_key(),
                    Key::Character(c) => self.modifiers.super_key() && c.eq_ignore_ascii_case("q"),
                    _ => false,
                };
                if quit_key {
                    self.outcome = ShellOutcome::Quit;
                    event_loop.exit();
                    return;
                }
                // Same window-mode toggle the game uses, before the field sees
                // the key, so Alt-Enter never submits the locate form.
                if self.modifiers.alt_key()
                    && event.logical_key == Key::Named(winit::keyboard::NamedKey::Enter)
                {
                    if !event.repeat {
                        self.fullscreen = !self.fullscreen;
                        presenter
                            .window
                            .set_fullscreen(fullscreen_attribute(self.fullscreen));
                        self.last_pointer = None;
                        self.redraw();
                    }
                    return;
                }
                if let Some(name) = locate_key_name(&event.logical_key) {
                    let result = self.locate.key(name);
                    self.settle(event_loop, result);
                    return;
                }
                // Space continues a finished import and otherwise types a space.
                if event.logical_key == Key::Named(winit::keyboard::NamedKey::Space) {
                    let result = self.locate.key(" ");
                    self.locate.text_input(' ');
                    self.settle(event_loop, result);
                    return;
                }
                if let Some(text) = &event.text {
                    for ch in text.chars().filter(|ch| !ch.is_control()) {
                        self.locate.text_input(ch);
                    }
                    self.redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_pointer = presenter.viewport().point(position.x, position.y);
                if let Some((x, y)) = self.last_pointer {
                    self.locate.hover(x, y);
                    self.redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some((x, y)) = self.last_pointer {
                    let result = self.locate.click(x, y);
                    self.settle(event_loop, result);
                }
            }
            WindowEvent::RedrawRequested => {
                self.poll_import(event_loop);
                self.locate.draw(
                    &mut self.pixels,
                    &self.font,
                    &self.small,
                    self.background.as_deref(),
                );
                let pixels = std::mem::take(&mut self.pixels);
                if let Some(presenter) = &mut self.presenter {
                    if let Err(error) = presenter.present(&pixels) {
                        self.error = Some(error);
                        event_loop.exit();
                    }
                    // A surface that gave no image is tried again after a
                    // wait, never at once.
                    self.retry_at = presenter.surface_retry().map(|wait| Instant::now() + wait);
                }
                self.pixels = pixels;
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let changed = self.poll_import(event_loop);
        // The starting bar steps at the poll rate below, whatever else wakes
        // the loop.
        let animating = self.worker.is_some()
            && self.last_tick.elapsed() >= Duration::from_millis(33)
            && self.locate.tick();
        if animating {
            self.last_tick = Instant::now();
        }
        let retry = self.retry_at.is_some_and(|at| Instant::now() >= at);
        if retry {
            self.retry_at = None;
        }
        if changed || animating || retry {
            self.redraw();
        }
        // Poll the import often enough for a smooth progress bar.
        let poll = self
            .worker
            .is_some()
            .then(|| Instant::now() + Duration::from_millis(33));
        event_loop.set_control_flow(match poll.into_iter().chain(self.retry_at).min() {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Release the GPU backend while the display connection is still alive,
        // for the same reason the game renderer does; see docs/DEVELOPMENT.md.
        self.presenter = None;
    }
}

/// What the shell does without waiting for the player: the source to import as
/// soon as it opens, and whether a finished import continues into the game.
struct AutoImport {
    path: Option<PathBuf>,
    continue_when_done: bool,
}

/// Show the locate screen until the player quits or continues.
fn locate_shell(
    event_loop: &mut EventLoop<()>,
    data: &Path,
    prefill: Option<PathBuf>,
    candidates: Vec<media_source::MediaSource>,
    background: Option<Vec<u8>>,
    auto: AutoImport,
    window: &mut WindowState,
) -> AppResult<ShellOutcome> {
    let candidates: Vec<locate::Candidate> = candidates
        .into_iter()
        .map(|source| locate::Candidate {
            path: source.path,
            kind: match source.kind {
                media_source::Kind::Installed => locate::SourceKind::Installed,
                media_source::Kind::Disc => locate::SourceKind::Disc,
            },
        })
        .collect();
    match &auto.path {
        Some(path) => println!(
            "Locate Fighters Anthology: {} detected source(s), importing {}",
            candidates.len(),
            path.display()
        ),
        None => println!(
            "Locate Fighters Anthology: {} detected source(s), waiting for a choice",
            candidates.len()
        ),
    }
    let mut shell = LocateShell {
        locate: {
            let mut screen =
                locate::Locate::new(prefill.map(|path| path.display().to_string()), candidates);
            screen.set_graphics_api(graphics_api::screen_choice());
            screen
        },
        data: data.to_path_buf(),
        presenter: None,
        font: menu::flat_font([235, 239, 243]),
        small: menu::flat_font([210, 219, 230]),
        background: background.filter(|art| art.len() == menu::WIDTH * menu::HEIGHT * 4),
        pixels: vec![0; menu::WIDTH * menu::HEIGHT * 4],
        worker: None,
        last_tick: Instant::now(),
        auto_import: auto.path,
        auto_continue: auto.continue_when_done,
        outcome: ShellOutcome::Quit,
        error: None,
        last_pointer: None,
        fullscreen: window.fullscreen,
        modifiers: ModifiersState::empty(),
        retry_at: None,
    };
    event_loop.run_app_on_demand(&mut shell)?;
    if shell.fullscreen != window.fullscreen {
        window.toggle();
    }
    match shell.error {
        Some(error) => Err(error),
        None => Ok(shell.outcome),
    }
}

/// The modes of `--window-layout-preview`.
const WINDOW_LAYOUT_MODES: [&str; 4] = ["before", "launch", "free", "landed"];

/// `--panel-snapshot PATH --window-layout-preview before|launch|free|landed`:
/// the whole row or corners of instrument windows on a plain backdrop, with
/// no GPU, as a PPM at `--window-size` (docs/spec/target-window.md, "Seeker
/// window"). `before` fills every slot of the layout without the target
/// camera, `launch` is the same after a seeker launch has opened the camera
/// (the pop-up), `free` leaves slots free so the launch takes one, and
/// `landed` is the weapon's landing after `launch`. The camera shows the
/// seeker page of the synthetic preview scene.
fn write_window_layout_preview(
    path: &str,
    mode: &str,
    mut panels: instruments::Instruments,
    hornet: &aircraft::Airframe,
    state: &flight::State,
    size: [u32; 2],
) -> AppResult<Outcome> {
    use std::io::Write;
    let layout = panels.layout;
    panels.preview_window_layout(mode);
    let sizef = size.map(f64::from);
    let camera = panels
        .pages
        .iter()
        .position(|page| *page == instruments::TARGET_PAGE)
        .map(|slot| panels.screen_rect(slot, sizef));
    println!(
        "window layout {layout:?} {mode}: pages {:?}, popup {}, opened by launch {}, target camera {}",
        panels.pages,
        panels.popup_shown(),
        panels.seeker_opened,
        camera.map_or("closed".to_string(), |(x, y, w, h)| format!(
            "x={x:.0} y={y:.0} w={w:.0} h={h:.0}"
        ))
    );
    let mut canvas = flight_canvas::FlightCanvas::default();
    canvas.begin(size, hornet, state, &panels);
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", size[0], size[1])?;
    let mut out = Vec::with_capacity(size[0] as usize * size[1] as usize * 3);
    for (i, p) in canvas.pixels.chunks_exact(4).enumerate() {
        // A plain sky over a plain ground, so the windows stand out.
        let y = i / size[0] as usize;
        let back: [f32; 3] = if y * 100 < size[1] as usize * 58 {
            [92., 128., 176.]
        } else {
            [78., 96., 66.]
        };
        let a = f32::from(p[3]) / 255.;
        for c in 0..3 {
            out.push((f32::from(p[c]) * a + back[c] * (1. - a)).round() as u8);
        }
    }
    f.write_all(&out)?;
    Ok(Outcome::Done)
}

/// `--hud-snapshot PATH [--hud-snapshot-state forward|hover|converting|low|attitude:PITCH,BANK]`:
/// the HUD of the selected aircraft over a flat background, headless, as a
/// PPM. Powered-lift aircraft start in trimmed forward flight (`forward`) or
/// in a hover (`hover`: a jet with its nozzles vertical, a helicopter trimmed
/// at rest), 3,000 feet over flat ground, and fly a second so the rotor and
/// engines are settled; any other aircraft is shown as it starts. `low` is the
/// hover 45 feet over the ground at stability Off, which shows the radar
/// height and the stability label (VTOL overhaul design section 6).
fn write_hud_snapshot(hornet: &aircraft::Airframe, state: &str, path: &str) -> AppResult<()> {
    use std::io::Write;
    let mut flight = flight::State::new(&hornet.profile, [0., 3000., 0.])?;
    flight.enable_research(1)?;
    flight.cheats.unlimited_fuel = true;
    flight.yaw = 0.;
    let mut ground = 0.;
    match state {
        "forward" | "laser-lsr" | "laser-ready" | "laser-mask" | "laser-bud" | "laser-both"
        | "laser-edge" | "tti" | "tti-lsr" | "tti-many" | "tti-impact" => {
            flight.start_airborne([0.; 3]);
        }
        // A review preview of the pitch ladder: `attitude:PITCH,BANK` in degrees.
        attitude if attitude.starts_with("attitude:") => {
            flight.start_airborne([0.; 3]);
        }
        "hover" => {
            if !flight.trim_hover() {
                return Err("this aircraft has no hover to show".into());
            }
        }
        // A tiltrotor in a hover that is going too fast for its nacelles: the
        // conversion protection drives them forward (the CONV cue).
        "converting" => {
            if !flight.trim_hover() {
                return Err("this aircraft has no hover to show".into());
            }
            flight.velocity = [0., 0., 140. * 1.687_81];
            flight.speed = flight.velocity[2];
        }
        // The hover low over the ground at stability Off.
        "low" => {
            if !flight.trim_hover() {
                return Err("this aircraft has no hover to show".into());
            }
            flight.command(tore_input::PilotCommand::Lift(
                tore_input::LiftCommand::SetStability(tore_input::StabilityLevel::Off),
            ));
            ground = flight.position[1] - 45.;
        }
        other => {
            return Err(format!(
                "unknown HUD snapshot state {other}; use forward, hover, converting, low, attitude:PITCH,BANK, laser-lsr, laser-ready, laser-mask, laser-bud, laser-both, laser-edge, tti, tti-lsr, tti-many or tti-impact"
            )
            .into());
        }
    }
    // A second of flight over a flat plain 3,000 feet below (45 for `low`).
    for _ in 0..120 {
        flight.step_surface(&tore_input::PilotInput::default(), |_, _| {
            tore_sim::research::Surface::runway(ground)
        });
    }
    if let Some(attitude) = state.strip_prefix("attitude:") {
        let mut angles = attitude
            .split(',')
            .map(|v| v.trim().parse::<f64>().map(f64::to_radians));
        match (angles.next(), angles.next(), angles.next()) {
            (Some(Ok(pitch)), Some(Ok(bank)), None) => {
                flight.pitch = pitch;
                flight.bank = bank;
            }
            _ => return Err("attitude needs PITCH,BANK in degrees, e.g. attitude:20,30".into()),
        }
    }
    let color = hornet.daylight_palette()[usize::from(hornet.hud.primary_color)];
    let mut pixels = vec![0u8; menu::WIDTH * menu::HEIGHT * 4];
    hud::draw(
        &mut pixels,
        &flight,
        &hornet.hud_font,
        ground,
        None,
        true,
        false,
        color,
        1.,
        None,
        None,
        (flight.bank, 1.),
    );
    // The laser's box and text and the time to impact row over the HUD
    // (docs/spec/air-to-ground.md).
    let tti = weapon_hud::laser::tti_preview(state);
    let laser = weapon_hud::laser::preview(
        if state == "tti-lsr" {
            "laser-lsr"
        } else {
            state
        },
        &flight,
    );
    if laser.is_some() || tti.is_some() {
        weapon_hud::laser::draw_rows(
            &mut pixels,
            &flight,
            laser.as_ref(),
            0,
            tti.as_deref(),
            &hornet.hud_font,
            color,
            1.,
        );
    }
    // Over a plain sky and ground split at the horizon of the camera.
    let horizon = hud::project(flight.pitch, flight.bank, 0., 0., 1.)
        .map_or(240., |(_, y)| y)
        .clamp(0., menu::HEIGHT as f64);
    let mut file = std::fs::File::create(path)?;
    write!(file, "P6\n{} {}\n255\n", menu::WIDTH, menu::HEIGHT)?;
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        let y = (index / menu::WIDTH) as f64;
        let back: [u8; 3] = if y < horizon {
            [52, 88, 140]
        } else {
            [64, 84, 52]
        };
        let rgb = if pixel[3] == 0 {
            back
        } else {
            [pixel[0], pixel[1], pixel[2]]
        };
        file.write_all(&rgb)?;
    }
    Ok(())
}

/// Headless locate-screen preview (`--snapshot PATH --snapshot-state locate`,
/// `locate-starting`, `locate-importing` or `locate-done`). The candidate list and the import
/// figures are fixed so the layout is reviewable on any machine, with or
/// without media. The figures are those of a disc 1 import.
fn locate_snapshot(path: &Path, state: &str) -> AppResult<()> {
    use std::io::Write;
    // The progress and completion previews show the disc being imported.
    let chosen = (state != "locate").then(|| String::from("/run/media/pilot/FA_DISC1"));
    let mut screen = locate::Locate::new(
        chosen,
        vec![
            locate::Candidate {
                path: PathBuf::from("gameassets/fighters-anthology"),
                kind: locate::SourceKind::Installed,
            },
            locate::Candidate {
                path: PathBuf::from("/run/media/pilot/FA_DISC1"),
                kind: locate::SourceKind::Disc,
            },
        ],
    );
    match state {
        "locate" => screen.set_hint(
            "Drop the mounted disc 1 folder on this window, or type the folder above.".into(),
        ),
        "locate-starting" => screen.set_phase(locate::Phase::Starting {
            step: "Finding aircraft and theaters".into(),
        }),
        "locate-graphics-api" => {
            screen.set_graphics_api(Some(graphics::GraphicsApi::Vulkan));
            screen
                .set_hint("The game will use Vulkan when it starts. No restart needed.".to_owned());
        }
        "locate-importing" => screen.set_phase(locate::Phase::Importing {
            archive: "FA_2.LIB".into(),
            resources_done: 1344,
            resources_total: Some(2247),
        }),
        "locate-done" => screen.set_phase(locate::Phase::Done {
            summary: vec![
                "Build read: FA.EXE 1.0 (disc)".into(),
                "FA_1.LIB: 1996 entries read".into(),
                "FA_2.LIB: 5405 entries read".into(),
            ],
        }),
        other => return Err(format!("unknown locate snapshot state {other}").into()),
    }
    let mut pixels = vec![0u8; menu::WIDTH * menu::HEIGHT * 4];
    screen.draw(
        &mut pixels,
        &menu::flat_font([235, 239, 243]),
        &menu::flat_font([210, 219, 230]),
        None,
    );
    let mut file = std::fs::File::create(path)?;
    write!(file, "P6\n{} {}\n255\n", menu::WIDTH, menu::HEIGHT)?;
    for pixel in pixels.chunks_exact(4) {
        file.write_all(&pixel[..3])?;
    }
    println!("Locate preview: {}", path.display());
    Ok(())
}

/// The saved window-mode preference. Borderless fullscreen is the default, so
/// a missing, unreadable or older preferences file starts fullscreen.
fn saved_fullscreen() -> bool {
    let Ok(directory) = assets::data_directory() else {
        return true;
    };
    preferences::read(&directory.join("preferences-v1.conf"))
        .ok()
        .and_then(|text| preferences::Preferences::parse(&text).ok())
        .is_none_or(|saved| saved.fullscreen)
}

/// The event loop is created at most once per process and only when a window
/// is actually wanted, so headless runs still work without a display.
fn shared_event_loop(slot: &mut Option<EventLoop<()>>) -> AppResult<&mut EventLoop<()>> {
    if slot.is_none() {
        diagnostics::stage("event loop creation");
        slot.replace(EventLoop::new()?);
        diagnostics::stage_done();
    }
    Ok(slot.as_mut().expect("the event loop was just created"))
}

fn sessions(event_loop: &mut Option<EventLoop<()>>) -> AppResult<()> {
    let mut session = Session::First;
    loop {
        match run(event_loop, session)? {
            Outcome::Done => return Ok(()),
            // Pref asked for another import: the shell runs again, and on
            // Continue the game is rebuilt from the new pack.
            Outcome::Reimport(frame) => session = Session::Reimport(frame),
        }
    }
}

fn main() -> std::process::ExitCode {
    diagnostics::init();
    tore_import::set_log(|level, text| match level {
        tore_import::Level::Info => log::info!("{text}"),
        tore_import::Level::Warn => log::warn!("{text}"),
    });
    let interactive = startup::interactive();
    diagnostics::install_native_crash_reporter(interactive);
    let result = std::panic::catch_unwind(|| {
        if let Some(result) = startup::self_test() {
            return result;
        }
        let mut event_loop = None;
        sessions(&mut event_loop)
    });
    match result {
        Ok(Ok(())) => {
            diagnostics::finish_success();
            std::process::ExitCode::SUCCESS
        }
        Ok(Err(error)) => {
            diagnostics::report_error(error.as_ref(), interactive);
            std::process::ExitCode::FAILURE
        }
        Err(_) => {
            diagnostics::report_panic(interactive);
            std::process::ExitCode::from(101)
        }
    }
}

fn run(event_loop: &mut Option<EventLoop<()>>, session: Session) -> AppResult<Outcome> {
    if std::env::args().nth(1).as_deref() == Some("--reel-render") {
        reel::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--replay-stills") {
        reel::stills()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--reel-music") {
        reel::music()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--terrain-colors") {
        terrain_colors::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--terrain-audit") {
        terrain_audit::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--blast-preview") {
        blast_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--impact-preview") {
        impact_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--cockpit-sheet") {
        cockpit_sheet::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--dome-sheet") {
        cockpit_sheet::domes()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--stores-size-sheet") {
        stores_preview::size_sheet()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--stores-sizes") {
        stores_preview::sizes()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--replay-banner-check") {
        replay::viewer::banner_check::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--replay-stores-check") {
        replay::stores_check::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--stores-preview") {
        stores_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--gun-flash-preview") {
        gun_flash_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--vegetation-survey") {
        vegetation::survey()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--vegetation-art") {
        vegetation_preview::art()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--vegetation-preview") {
        vegetation_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--city-layout-digest") {
        city_fill_dump::digests()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--city-fill-survey") {
        city_fill_dump::survey()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--city-fires") {
        city_fill_dump::fires()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--city-fill-dump") {
        city_fill_dump::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--bridge-audit") {
        bridge_audit::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--qmc-distance-table") {
        qmc_distance_table::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-dump") {
        surface_dump::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-battle") {
        surface_battle::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-drive") {
        surface_drive::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--airfield-audit") {
        airfield_audit::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--airfield-sheets") {
        airfield_sheets::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--missile-study") {
        missile_study::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--a2g-run") {
        a2g_run::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--hud-a2g-preview") {
        a2g_run::preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--hud-guided-preview") {
        a2g_run::guided_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-objective") {
        surface_objective::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--a2g-radar") {
        a2g_radar::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-parked") {
        surface_parked::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-scene") {
        surface_scene::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-sheets") {
        surface_dump::sheets()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-trace") {
        surface_trace::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--flak-pass") {
        flak_pass::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-preview") {
        surface_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--flight-map-preview") {
        flight_map_preview::run()?;
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--night-light-slots") {
        night_glow::stability_probe()?;
        println!("Night light slots: 240 frames, 8 fires stable through both budgets and upload");
        return Ok(Outcome::Done);
    }
    if std::env::args().nth(1).as_deref() == Some("--surface-fx-preview") {
        surface_fx_preview::run()?;
        return Ok(Outcome::Done);
    }
    diagnostics::stage("argument parsing and startup options");
    if matches!(session, Session::First)
        && !std::env::args().any(|arg| arg == "--version" || arg == "-V")
    {
        // `--find-games` and `--browse` keep stdout for the games they list.
        if std::env::args().any(|a| a == "--find-games" || a == "--browse" || a == "--map-port") {
            eprintln!("{}", version::label());
        } else {
            println!("{}", version::label());
        }
    }
    if let Ok(value) = std::env::var("TORE_FRAME_LOG") {
        frame_log::configure(frame_log::threshold(&value)?);
    }
    let mut args = std::env::args().skip(1).peekable();
    let mut live_fire = false;
    let mut dummy_aircraft = Vec::new();
    let mut jammer_on = false;
    let mut combat_smoke = false;
    let mut gunsight_probe: Option<String> = None;
    let mut gunsight_dump: Option<PathBuf> = None;
    let mut missile_acceptance = false;
    let mut missile_sag = false;
    let mut record_combat = None;
    let mut replay_combat = None;
    let mut combat_probe = None;
    let mut ai_wings_enabled = false;
    let mut fixture_wings = false;
    let mut ai_probe = None;
    let mut ai_roster_probe = false;
    let mut rotor_probe = false;
    // Mission recordings: what happened, not re-simulation tapes.
    let mut record_mission: Option<PathBuf> = None;
    let mut verify_render = false;
    let mut recording_info: Option<PathBuf> = None;
    let mut recording_log: Option<PathBuf> = None;
    let mut recording_acmi: Option<PathBuf> = None;
    let mut recording_diff: Option<(PathBuf, PathBuf)> = None;
    let mut convert_capture: Option<PathBuf> = None;
    let mut recording_out: Option<PathBuf> = None;
    let mut recording_from: Option<f64> = None;
    let mut recording_to: Option<f64> = None;
    let mut recording_ids: Option<Vec<u32>> = None;
    let mut recording_rate: Option<f64> = None;
    let mut recording_guns = false;
    // Session only. `docs/spec/ai-experience.md` records the flight-menu
    // enemy-skill preference's persistence as untraced, so this setting is not
    // written to the preferences file and does not survive a restart.
    let mut enemy_skill = None;
    let mut ai_mission = ai_wings::Preset::Free;
    let mut combat_commands = Vec::new();
    let mut weapon_slot: Option<usize> = None;
    let mut stripped_loadout: Option<String> = None;
    let mut input_profile = None;
    let mut native_input = true;
    let mut record_input = None;
    let mut replay_input = None;
    let mut input_seconds = None;
    let mut write_input_profile = None;
    let mut test_rumble = None;
    let (mut import, mut snapshot) = (None, None);
    let mut snapshot_state = String::from("normal");
    let mut background = None;
    let mut theater_code = String::from("UKR");
    let mut aircraft_id = tore_formats::aircraft::AircraftId::F18;
    // Whether --aircraft was given, for probes that cover one aircraft only.
    let mut aircraft_requested = false;
    let mut initial_screen = Screen::Main;
    let mut flight_view = 0;
    let mut flight_reference = flight_views::Reference::Player;
    let mut flight_look = [0f32; 2];
    let mut flight_zoom = 1f32;
    let mut flight_menu = false;
    let mut flight_map = false;
    let mut mission_brief = false;
    let mut weapon_diagnostics = false;
    let mut debug_panels = false;
    let mut flight_panels = Vec::new();
    let mut controls_menu = false;
    let mut flight_mode_arg = None;
    let mut native_tables_path: Option<PathBuf> = None;
    let mut window_size = [960, 720];
    let mut instrument_page = None;
    let mut sensor_channel = None;
    let mut scope_range = None;
    let mut scope_history = false;
    let mut instrument_layout = instruments::Layout::Large;
    let mut capture_terrain = None;
    let mut native_flight_report = false;
    let mut native_flight_trig = None;
    let mut headless_ticks = None;
    let mut flight_probe_ticks = None;
    let mut flight_trace_ticks = 0u64;
    let mut flight_faults: Vec<(u64, usize)> = Vec::new();
    let mut flight_cheats: Vec<String> = Vec::new();
    let mut flight_fuel: Option<f64> = None;
    let mut flight_start: Option<[f64; 4]> = None;
    let mut flight_devices = None;
    let mut flight_controls = None;
    let mut flight_throttle = None;
    let mut flight_bay = None;
    let mut damage_preview = None;
    let mut ejection_preview = None;
    let mut hud_target_preview: Option<[f64; 3]> = None;
    let mut sight_preview: Option<[f64; 3]> = None;
    let mut damage_preview_section = tore_sim::combat::live::DamageSection::Nose;
    let mut damage_preview_ticks = 240usize;
    let mut countermeasure_preview = None;
    let mut maneuver = String::from("level");
    let mut panel_snapshot = None;
    let mut target_cam_preview: Option<String> = None;
    let mut window_layout_preview: Option<String> = None;
    let mut hud_snapshot: Option<String> = None;
    let mut hud_snapshot_state = String::from("forward");
    let mut systems_preview: Vec<usize> = Vec::new();
    let mut validate_creator = false;
    let mut validate_tanks = false;
    let mut validate_ordnance = false;
    let mut wing_loadout_probe = None;
    let mut animation_probe: Option<std::path::PathBuf> = None;
    let mut sensor_summary = false;
    let mut validate_weather = false;
    let mut validate_maps = false;
    let mut validate_ils = false;
    let mut airport_allegiance = false;
    let mut validate_text = false;
    let mut weather_condition: Option<usize> = None;
    let mut airport_probe: Option<(u32, tore_sim::airport::Aircraft, Option<[f64; 2]>)> = None;
    let mut ground_start_airport: Option<u32> = None;
    let mut enemy_ground_start_airport: Option<u32> = None;
    let mut probe_script = ProbeScript::default();
    let mut separation_nm: Option<f64> = None;
    // `--ground-target STEM`, `--defenses AAA SAM`, `--surface-seed N`: the
    // creator's ground target for a launched flight or a capture.
    let mut ground_target: Option<String> = None;
    let mut target_defenses = (3_usize, 3_usize);
    let mut target_defenses_given = false;
    let mut target_seed = 1_u32;
    let mut launch_creator = false;
    let (mut smoke_test, mut no_audio, mut import_only) = (false, false, false);
    // `--windowed`, and any flag that fixes the window size, opt out of the
    // borderless fullscreen default.
    let mut windowed_flag = false;
    let mut graphics_flags: Vec<(String, String)> = Vec::new();
    let mut window_size_flag = false;
    // Mission replay viewer; see docs/REPLAYS.md.
    let mut watch_replay: Option<PathBuf> = None;
    let mut replay_capture: Option<PathBuf> = None;
    // Joining a dedicated server, or hosting; see docs/DEDICATED-SERVER.md.
    let mut session_args = net::options::SessionArgs::default();
    // `--find-games SECONDS`: list the games on this network and exit.
    let mut find_games: Option<f64> = None;
    // `--browse SECONDS`: list the games on the Internet Lobby and exit.
    let mut browse_games: Option<f64> = None;
    // `--map-port SECONDS`: forward the game port on the router for that
    // long, and exit.
    let mut map_port: Option<f64> = None;
    let mut input_script_steps: Option<Vec<input_script::Step>> = None;
    let mut replay_options = replay::viewer::Options::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-controllers" => native_input = false,
            "--launch-quick-mission" => { launch_creator=true; initial_screen=Screen::Flight; },
            "--enemy-ground-start" => {
                enemy_ground_start_airport = Some(option_number(
                    "--enemy-ground-start",
                    &args.next().ok_or("--enemy-ground-start needs an airport number")?,
                )?);
            }
            "--ground-start" => {
                ground_start_airport=Some(option_number("--ground-start", &args.next().ok_or("--ground-start needs an airport number")?)?);
            }
            "--ground-target" => {
                ground_target = Some(args.next().ok_or("--ground-target needs a template stem, for example QTAAA")?);
            }
            "--defenses" => {
                let usage = "--defenses needs an AAA level and a SAM level, each 0 to 3";
                let level = |value: Option<String>| -> Result<usize, String> {
                    value.ok_or(usage)?.parse::<usize>().ok().filter(|n| *n <= 3).ok_or_else(|| usage.to_owned())
                };
                target_defenses = (level(args.next())?, level(args.next())?);
                target_defenses_given = true;
            }
            "--surface-seed" => {
                target_seed = args.next().ok_or("--surface-seed needs a number")?.parse().map_err(|e| bad_number(&arg, &e))?;
            }
            "--separation" => {
                let nm: f64 = args.next().ok_or("--separation needs a distance in nautical miles")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !mission_layout::SEPARATION_NM.contains(&nm) {
                    return Err(format!("--separation needs one of {:?} nautical miles", mission_layout::SEPARATION_NM).into());
                }
                separation_nm = Some(nm);
            }
            "--probe-wing-only" => probe_script.wing_only = true,
            "--probe-render" => {
                probe_script.render = Some(args.next().ok_or("--probe-render needs DIR")?.into())
            }
            "--probe-render-at" => {
                probe_script.render_at = args
                    .next()
                    .ok_or("--probe-render-at needs TICK,...")?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--probe-render-at needs TICK,...")?
            }
            "--probe-render-plane" => {
                probe_script.render_plane = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--probe-render-plane needs an aircraft id")?
            }
            "--probe-render-view" => {
                probe_script.render_view = args.next().ok_or("--probe-render-view needs a view")?
            }
            "--probe-pavement" => probe_script.pavement = true,
            "--probe-track" => probe_script.track = Some(PathBuf::from(args.next().ok_or("--probe-track needs an output file")?)),
            "--probe-redfor" => probe_script.redfor = true,
            "--probe-group" => {
                let usage = "--probe-group needs GROUP:CHOICE[:survive], group 1..6";
                let value = args.next().ok_or(usage)?;
                let mut parts = value.split(':');
                let group: usize = parts.next().ok_or(usage)?.parse().map_err(|_| usage)?;
                let choice: usize = parts.next().ok_or(usage)?.parse().map_err(|_| usage)?;
                let survive = match parts.next() {
                    None => false,
                    Some("survive") => true,
                    Some(_) => return Err(usage.into()),
                };
                if !(1..=6).contains(&group) {
                    return Err(usage.into());
                }
                probe_script.groups.push((group - 1, choice, survive));
            }
            "--ai-observe-hz" => {
                let usage = "--ai-observe-hz needs 120, 60, 40, 30 or 20";
                let hz: u32 = args.next().ok_or(usage)?.parse().map_err(|_| usage)?;
                probe_script.ai_test = probe_script.ai_test.with_observation_hz(hz).ok_or(usage)?;
            }
            "--ai-lean-sensors" => {
                // Bare means on; `on` or `off` follows when given.
                let lean = match args.peek().map(String::as_str) {
                    Some(word @ ("on" | "off")) => {
                        let lean = word == "on";
                        args.next();
                        lean
                    }
                    _ => true,
                };
                probe_script.ai_test = probe_script.ai_test.with_lean_sensors(lean);
            }
            "--probe-fight" => {
                let usage = "--probe-fight needs FRIENDLY:ENEMY, each 1..15";
                let value = args.next().ok_or(usage)?;
                let (f, e) = value.split_once(':').ok_or(usage)?;
                let (f, e): (usize, usize) = (f.parse().map_err(|_| usage)?, e.parse().map_err(|_| usage)?);
                if !(1..=15).contains(&f) || !(1..=15).contains(&e) {
                    return Err(usage.into());
                }
                probe_script.fight = Some((f, e));
            }
            "--probe-friendly-aircraft" => probe_script.friendly_aircraft = Some(tore_formats::aircraft::AircraftId::parse(&args.next().ok_or("--probe-friendly-aircraft needs an aircraft")?)?),
            "--probe-matrix" => probe_script.matrix = Some(PathBuf::from(args.next().ok_or("--probe-matrix needs a new output directory")?)),
            "--probe-enemy-aircraft" => probe_script.enemy_aircraft = Some(tore_formats::aircraft::AircraftId::parse(&args.next().ok_or("--probe-enemy-aircraft needs an aircraft")?)?),
            "--probe-enemy-skill" => probe_script.enemy_skill = Some(match args.next().ok_or("--probe-enemy-skill needs novice|average|experienced|ace")?.as_str() {
                "novice" => 0, "average" => 1, "experienced" => 2, "ace" => 3, _ => return Err("unknown probe skill".into()),
            }),
            "--probe-geometry" => probe_script.geometry = match args.next().ok_or("--probe-geometry needs head|rear|side")?.as_str() {
                "head" => ProbeGeometry::Head, "rear" => ProbeGeometry::Rear, "side" => ProbeGeometry::Side, _ => return Err("unknown probe geometry".into()),
            },
            "--probe-guns" => probe_script.guns = true,
            "--probe-ai-guns-only" => probe_script.ai_guns_only = true,
            "--probe-flight-model" => probe_script.researched = match args.next().ok_or("--probe-flight-model needs legacy|researched")?.as_str() {
                "legacy" => false, "researched" => true, _ => return Err("unknown probe flight model".into()),
            },
            "--probe-ai-flight-model" => {
                probe_script.ai_flight_model = args
                    .next()
                    .ok_or("--probe-ai-flight-model needs standard|all-hybrid")?
                    .parse()?;
            }
            "--probe-fault" => {
                if probe_script.threats.len() >= 64 { return Err("at most 64 controlled threats/faults per probe".into()); }
                let value = args.next().ok_or("--probe-fault needs TICK:INDEX (0..44)")?;
                let (tick, index) = value.split_once(':').ok_or("--probe-fault needs TICK:INDEX")?;
                let index: usize = index.parse().map_err(|e| bad_number(&arg, &e))?;
                if index >= 45 { return Err("fault index must be 0..44".into()); }
                probe_script.threats.push((tick.parse().map_err(|e| bad_number(&arg, &e))?, ProbeThreat::Fault(index)));
            }
            "--probe-threat" => {
                if probe_script.threats.len() >= 64 { return Err("at most 64 controlled threats per probe".into()); }
                let value = args.next().ok_or("--probe-threat needs TICK:hit|gun|aaa")?;
                let (tick, kind) = value.split_once(':').ok_or("--probe-threat needs TICK:hit|gun|aaa")?;
                let kind = match kind { "hit" => ProbeThreat::Hit, "gun" => ProbeThreat::Gun, "aaa" => ProbeThreat::Aaa, _ => return Err("unknown probe threat".into()) };
                probe_script.threats.push((tick.parse().map_err(|e| bad_number(&arg, &e))?, kind));
            }
            "--probe-wing-size" => {
                let size: usize = args.next().ok_or("--probe-wing-size needs 1..5")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(1..=5).contains(&size) {
                    return Err("--probe-wing-size needs 1..5".into());
                }
                probe_script.wing_size = Some(size);
            }
            "--probe-wing-order" => probe_script.orders.push(ProbeScript::parse_order(
                &args.next().ok_or("--probe-wing-order needs TICK:ORDER")?,
            )?),
            "--probe-player-lock" => {
                probe_script.player_locks.push(ProbeScript::parse_player_lock(
                    &args.next().ok_or("--probe-player-lock needs TICK:ID")?,
                )?);
                probe_script.data_link = true;
            }
            "--probe-data-link" => probe_script.data_link = true,
            "--probe-blind-wing" => probe_script.blind_wing = true,
            "--probe-missile-release" => probe_script.missile_release = Some(match args.next().as_deref() {
                Some("empty") => 0,
                Some("close") => 1,
                Some("head-ir") => 2,
                Some("maddog") => 3,
                Some("unlocked-ir") => 4,
                Some("max-range") => 5,
                _ => return Err("--probe-missile-release needs empty, close, head-ir, maddog, unlocked-ir or max-range".into()),
            }),
            "--probe-attack" => probe_script.attack = Some(ProbeScript::parse_attack(
                &args.next().ok_or("--probe-attack needs TICK or TICK:REPEAT_SECONDS")?,
            )?),
            "--probe-trace" => {
                let seconds: f64 = args.next().ok_or("--probe-trace needs seconds")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(seconds > 0. && seconds <= 3600.) {
                    return Err("--probe-trace needs 0..3600 seconds".into());
                }
                probe_script.trace_ticks = (seconds * 120.).round().max(1.) as u64;
            }
            "--probe-wing-route" => {
                let usage = "--probe-wing-route needs EAST_NM:NORTH_NM:ALTITUDE_FT";
                let text = args.next().ok_or(usage)?;
                let fields: Vec<f64> = text
                    .split(':')
                    .map(|f| f.parse::<f64>().map_err(|e| bad_number(&arg, &e)))
                    .collect::<Result<_, _>>()?;
                let [east, north, altitude] = fields[..] else {
                    return Err(usage.into());
                };
                if !(east.is_finite() && north.is_finite() && (0. ..=60_000.).contains(&altitude)) {
                    return Err(usage.into());
                }
                probe_script.wing_route.push([east, north, altitude]);
            }
            "--probe-fuel-trace" => {
                let seconds: f64 = args.next().ok_or("--probe-fuel-trace needs seconds")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(seconds > 0. && seconds <= 3600.) {
                    return Err("--probe-fuel-trace needs 0..3600 seconds".into());
                }
                probe_script.fuel_trace_ticks = (seconds * 120.).round().max(1.) as u64;
            }
            "--probe-rotor-trace" => {
                let seconds: f64 = args.next().ok_or("--probe-rotor-trace needs seconds")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(seconds > 0. && seconds <= 3600.) {
                    return Err("--probe-rotor-trace needs 0..3600 seconds".into());
                }
                probe_script.rotor_trace_ticks = (seconds * 120.).round().max(1.) as u64;
            }
            "--probe-fuel" => {
                let pounds: f64 = args
                    .next()
                    .ok_or("--probe-fuel needs pounds of internal fuel")?
                    .parse()
                    .map_err(|e| bad_number(&arg, &e))?;
                if !(pounds.is_finite() && (0. ..=100_000.).contains(&pounds)) {
                    return Err("--probe-fuel needs 0..100000 pounds".into());
                }
                probe_script.fuel_lbs = Some(pounds);
            }
            "--probe-lose-player" => {
                let tick = args.next().ok_or("--probe-lose-player needs a tick")?;
                probe_script.lose_player = Some(option_number("--probe-lose-player", &tick)?);
            }
            "--probe-player-home" => {
                let usage = "--probe-player-home needs FROM:UNTIL ticks";
                let text = args.next().ok_or(usage)?;
                let (from, until) = text.split_once(':').ok_or(usage)?;
                probe_script.home = Some((from.parse().map_err(|e| bad_number(&arg, &e))?, until.parse().map_err(|e| bad_number(&arg, &e))?));
            }
            "--airport-probe" => {
                let value = args.next().ok_or("--airport-probe needs ID,X,Y,Z,NAV,GEAR[,HEADING,PITCH]")?;
                let fields: Vec<_> = value.split(',').collect();
                if !matches!(fields.len(), 6 | 8) {
                    return Err("--airport-probe needs ID,X,Y,Z,NAV,GEAR[,HEADING,PITCH]".into());
                }
                let flag = |text: &str| match text { "0" => Ok(false), "1" => Ok(true), _ => Err("airport probe flags need 0 or 1") };
                let position = [fields[1].parse().map_err(|e| bad_number(&arg, &e))?, fields[2].parse().map_err(|e| bad_number(&arg, &e))?, fields[3].parse().map_err(|e| bad_number(&arg, &e))?];
                if position.iter().any(|value: &f64| !value.is_finite()) {
                    return Err("airport probe position must be finite".into());
                }
                let angles = if fields.len() == 8 {
                    let angles = [fields[6].parse::<f64>().map_err(|e| bad_number(&arg, &e))?, fields[7].parse::<f64>().map_err(|e| bad_number(&arg, &e))?];
                    if !angles.iter().all(|value| value.is_finite())
                        || angles[0].abs() > 360_000.
                        || !(-90. ..=90.).contains(&angles[1])
                    {
                        return Err("airport probe heading/pitch outside bounded degree range".into());
                    }
                    Some(angles)
                } else {
                    None
                };
                airport_probe = Some((fields[0].parse().map_err(|e| bad_number(&arg, &e))?, tore_sim::airport::Aircraft {
                    position,
                    forward: [0., 0., 1.],
                    nav_mode: flag(fields[4])?,
                    gear_down: flag(fields[5])?,
                    supported: false,
                    alive: true,
                    speed_fps: 140.0,
                    ground_clearance_ft: 0.,
                    redfor: false,
                }, angles));
            }
            "--record-input" => {
                record_input = Some(PathBuf::from(
                    args.next().ok_or("--record-input needs a new path")?,
                ))
            }
            "--replay-input" => {
                replay_input = Some(PathBuf::from(
                    args.next().ok_or("--replay-input needs a tape path")?,
                ))
            }
            "--combat-command" => {
                let name = args.next().ok_or(
                    "--combat-command needs arm/jettison/clear/class/fail/next/target/designate/damage/incoming/target-jammer",
                )?;
                if combat_commands.len() >= 32 {
                    return Err("too many combat setup commands".into());
                }
                combat_commands
                    .push(combat_tape::command(&name).ok_or("unknown combat setup command")?);
                live_fire = true;
                initial_screen = Screen::Flight;
            }
            "--record-combat" => {
                record_combat = Some(PathBuf::from(
                    args.next().ok_or("--record-combat requires a new path")?,
                ));
                live_fire = true;
                initial_screen = Screen::Flight;
            }
            "--replay-combat" => {
                replay_combat = Some(PathBuf::from(
                    args.next().ok_or("--replay-combat requires a path")?,
                ));
            }
            "--jammer-on" => {
                jammer_on = true;
                live_fire = true;
                initial_screen = Screen::Flight;
            }
            "--hud-target-preview" => {
                let values: Vec<f64> = args.next().ok_or("--hud-target-preview needs bearing,elevation,feet")?
                    .split(',').map(str::parse).collect::<Result<_,_>>()?;
                if values.len()!=3 || values.iter().any(|v| !v.is_finite()) || values[0].abs()>180. || values[1].abs()>90. || !(100. ..=60000.).contains(&values[2]) {
                    return Err("--hud-target-preview needs bearing +/-180, elevation +/-90, range 100..60000 feet".into());
                }
                hud_target_preview = Some([values[0],values[1],values[2]]);
                live_fire = true;
            }
            "--sight-preview" => {
                let usage = "--sight-preview needs heading,elevation,ticks (degrees from the AC-130's nose, 0..7200 ticks)";
                let values: Vec<f64> = args.next().ok_or(usage)?
                    .split(',').map(str::parse).collect::<Result<_,_>>()?;
                if values.len() != 3 || values.iter().any(|v| !v.is_finite()) || values[0].abs() > 360. || values[1].abs() > 89. || !(0. ..=7200.).contains(&values[2]) {
                    return Err(usage.into());
                }
                sight_preview = Some([values[0], values[1], values[2].round()]);
                live_fire = true;
            }
            "--damage-preview-ticks" => {
                damage_preview_ticks = args.next().ok_or("--damage-preview-ticks needs 1..7200")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(1..=7200).contains(&damage_preview_ticks) { return Err("--damage-preview-ticks needs 1..7200".into()); }
            }
            "--countermeasure-preview" => {
                let ticks: usize = args.next().ok_or("--countermeasure-preview needs 1..7200")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !(1..=7200).contains(&ticks) { return Err("--countermeasure-preview needs 1..7200".into()); }
                countermeasure_preview = Some(ticks);
            }
            "--damage-preview" => {
                let fraction = args.next().ok_or("--damage-preview needs 0..1")?.parse::<f64>().map_err(|e| bad_number(&arg, &e))?;
                if !fraction.is_finite() || !(0. ..=1.).contains(&fraction) { return Err("--damage-preview needs 0..1".into()); }
                damage_preview = Some(fraction);
                initial_screen = Screen::Flight;
            }
            "--damage-preview-section" => {
                damage_preview_section = match args.next().as_deref() {
                    Some("nose") => tore_sim::combat::live::DamageSection::Nose,
                    Some("cockpit") => tore_sim::combat::live::DamageSection::Cockpit,
                    Some("core") => tore_sim::combat::live::DamageSection::Core,
                    Some("left-wing") => tore_sim::combat::live::DamageSection::LeftWing,
                    Some("right-wing") => tore_sim::combat::live::DamageSection::RightWing,
                    Some("tail") => tore_sim::combat::live::DamageSection::Tail,
                    _ => return Err("--damage-preview-section needs nose|cockpit|core|left-wing|right-wing|tail".into()),
                };
            }
            "--dummy-aircraft" => {
                let value = args.next().ok_or("--dummy-aircraft needs ID,COUNT")?;
                let (id, count) = value.split_once(',').ok_or("--dummy-aircraft needs ID,COUNT")?;
                dummy_aircraft.push((tore_formats::aircraft::AircraftId::parse(id)?, count.parse::<usize>().map_err(|e| bad_number(&arg, &e))?));
                initial_screen = Screen::Flight;
            }
            "--live-fire" => {
                live_fire = true;
                initial_screen = Screen::Flight;
            }
            "--ai-wings" => ai_wings_enabled = true,
            "--ai-mission" => {
                ai_mission = args.next().ok_or("--ai-mission needs a preset")?.parse()?;
            }
            "--fixture-wings" => fixture_wings = true,
            "--enemy-skill" => {
                enemy_skill = Some(
                    match args
                        .next()
                        .ok_or("--enemy-skill needs novice or average")?
                        .as_str()
                    {
                        "novice" => tore_sim::ai::experience::EnemySkillOverride::AllNovice,
                        "average" => tore_sim::ai::experience::EnemySkillOverride::AllAverage,
                        _ => return Err("--enemy-skill needs novice or average".into()),
                    },
                );
            }
            "--rotor-probe" => rotor_probe = true,
            "--ai-probe-ticks" | "--ai-roster-probe-ticks" => {
                ai_roster_probe = arg == "--ai-roster-probe-ticks";
                let ticks: usize = option_number(
                    &arg,
                    &args.next().ok_or("--ai-probe-ticks requires 1..288000")?,
                )?;
                if !(1..=288_000).contains(&ticks) {
                    return Err(format!("{arg} needs 1 to 288000 ticks, not {ticks}").into());
                }
                ai_probe = Some(ticks);
                ai_wings_enabled = true;
            }
            "--record-mission" => {
                let path = args.next().ok_or("--record-mission needs a new path")?;
                if path.is_empty() {
                    return Err("--record-mission needs a new path, not an empty one".into());
                }
                record_mission = Some(PathBuf::from(path));
            }
            "--verify-render" => verify_render = true,
            "--recording-info" => {
                recording_info = Some(PathBuf::from(
                    args.next().ok_or("--recording-info needs a recording")?,
                ));
            }
            "--recording-log" => {
                recording_log = Some(PathBuf::from(
                    args.next().ok_or("--recording-log needs a recording")?,
                ));
            }
            "--recording-acmi" => {
                recording_acmi = Some(PathBuf::from(
                    args.next().ok_or("--recording-acmi needs a recording")?,
                ));
            }
            "--convert-capture" => {
                convert_capture = Some(PathBuf::from(
                    args.next().ok_or("--convert-capture needs a capture")?,
                ));
            }
            "--recording-diff" => {
                let usage = "--recording-diff needs two recordings";
                let a = PathBuf::from(args.next().ok_or(usage)?);
                let b = PathBuf::from(args.next().ok_or(usage)?);
                recording_diff = Some((a, b));
            }
            "--out" => {
                recording_out = Some(PathBuf::from(
                    args.next().ok_or("--out needs a path")?,
                ));
            }
            "--from" | "--to" => {
                let seconds: f64 = option_number(
                    &arg,
                    &args
                        .next()
                        .ok_or(format!("{arg} needs seconds of mission time"))?,
                )?;
                if !(seconds.is_finite() && seconds >= 0.) {
                    return Err(format!("{arg} needs seconds of mission time, 0 or more").into());
                }
                if arg == "--from" {
                    recording_from = Some(seconds);
                } else {
                    recording_to = Some(seconds);
                }
            }
            "--ids" => {
                recording_ids = Some(
                    args.next()
                        .ok_or("--ids needs aircraft ids such as 0,7")?
                        .split(',')
                        .map(|id| option_number("--ids", id))
                        .collect::<Result<_, _>>()?,
                );
            }
            "--rate" => {
                let hz: f64 = option_number(
                    "--rate",
                    &args.next().ok_or("--rate needs samples per second")?,
                )?;
                if !(hz.is_finite() && hz > 0. && hz <= 120.) {
                    return Err("--rate needs samples per second above 0 and at most 120".into());
                }
                recording_rate = Some(hz);
            }
            "--guns" => recording_guns = true,
            "--missile-acceptance" => missile_acceptance = true,
            "--missile-sag" => { missile_acceptance = true; missile_sag = true; }
            "--compatibility-weapons" => combat_commands.push(tore_sim::combat::live::Command::CompatibilityWeapons),
            "--combat-smoke" => {
                combat_smoke = true;
            }
            "--gunsight-probe" => {
                let name = args.next().ok_or("--gunsight-probe needs a probe name")?;
                if !gunsight_probe::NAMES.contains(&name.as_str()) {
                    return Err(format!("--gunsight-probe needs one of {}", gunsight_probe::NAMES.join(", ")).into());
                }
                gunsight_probe = Some(name);
            }
            "--gunsight-dump" => {
                gunsight_dump = Some(PathBuf::from(
                    args.next().ok_or("--gunsight-dump needs a CSV path")?,
                ));
            }
            "--loadout" => {
                let choice = args.next().ok_or("--loadout needs none or guns")?;
                if !["none", "guns"].contains(&choice.as_str()) {
                    return Err("--loadout needs none (every store off) or guns (everything but the gun off)".into());
                }
                stripped_loadout = Some(choice);
            }
            "--weapon-slot" => {
                weapon_slot = Some(
                    args.next()
                        .ok_or("--weapon-slot requires a 1-based PT weapon slot")?
                        .parse().map_err(|e| bad_number(&arg, &e))?,
                );
            }
            "--combat-probe-ticks" => {
                let ticks: usize = args
                    .next()
                    .ok_or("--combat-probe-ticks requires 1..7200")?
                    .parse().map_err(|e| bad_number(&arg, &e))?;
                if !(1..=7200).contains(&ticks) {
                    return Err("combat probe tick limit exceeded".into());
                }
                combat_probe = Some(ticks);
                live_fire = true;
                initial_screen = Screen::Flight;
            }
            "--input-profile" => {
                input_profile = Some(PathBuf::from(
                    args.next().ok_or("--input-profile needs a path")?,
                ))
            }
            "--list-inputs" => input_seconds = Some(2),
            "--monitor-inputs" => {
                input_seconds = Some(option_number::<u64>(
                    "--monitor-inputs",
                    &args.next().ok_or("--monitor-inputs needs seconds")?,
                )?)
            }
            "--write-input-profile" => {
                write_input_profile = Some(PathBuf::from(
                    args.next()
                        .ok_or("--write-input-profile needs a new path")?,
                ));
                input_seconds.get_or_insert(2);
            }
            "--test-rumble" => {
                test_rumble = Some(
                    args.next()
                        .ok_or("--test-rumble needs a device id or only")?,
                );
                input_seconds.get_or_insert(2);
            }
            "--researched-flight" => flight_mode_arg = Some(true),
            "--legacy-flight" => flight_mode_arg = Some(false),
            "--native-flight-tables" => native_tables_path = Some(args.next().ok_or("--native-flight-tables needs a directory containing sine-q15.bin and atan-pa.bin")?.into()),
            "--capture-terrain" => {
                capture_terrain = Some(PathBuf::from(
                    args.next().ok_or("--capture-terrain needs a .ppm path")?,
                ));
                initial_screen = Screen::Viewer;
                smoke_test = true;
            }
            "--aircraft" => {
                aircraft_id = tore_formats::aircraft::AircraftId::parse(
                    &args.next().ok_or("--aircraft needs a supported aircraft ID (see --help)")?,
                )?;
                aircraft_requested = true;
            }
            "--theater" => {
                theater_code = args
                    .next()
                    .ok_or("--theater needs a code")?
                    .to_ascii_uppercase()
            }
            "--window-size" => {
                let size = args.next().ok_or("--window-size needs WIDTHxHEIGHT")?;
                let (w, h) = size
                    .split_once('x')
                    .ok_or("--window-size needs WIDTHxHEIGHT")?;
                window_size = [w.parse().map_err(|e| bad_number(&arg, &e))?, h.parse().map_err(|e| bad_number(&arg, &e))?];
                window_size_flag = true;
                if !(640..=3840).contains(&window_size[0])
                    || !(480..=2160).contains(&window_size[1])
                {
                    return Err("window size outside 640x480..3840x2160".into());
                }
            }
            "--instrument-layout" => {
                instrument_layout = match args.next().as_deref() {
                    Some("large") => instruments::Layout::Large,
                    Some("small") => instruments::Layout::Small,
                    _ => return Err("--instrument-layout needs large or small".into()),
                };
            }
            "--instrument-page" => {
                let page = args
                    .next()
                    .ok_or("--instrument-page needs 0..9")?
                    .parse::<u8>().map_err(|e| bad_number(&arg, &e))?;
                if !(0..=9).contains(&page) {
                    return Err("--instrument-page needs 0..9".into());
                }
                instrument_page = Some(page);
            }
            "--flight-zoom" => {
                flight_zoom = args.next().ok_or("--flight-zoom needs 0.5..4")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !flight_zoom.is_finite() || !(0.5..=4.).contains(&flight_zoom) {
                    return Err("--flight-zoom needs a finite value in 0.5..4".into());
                }
            }
            "--flight-look" => {
                let value = args
                    .next()
                    .ok_or("--flight-look needs YAW,PITCH in degrees")?;
                let (yaw, pitch) = value
                    .split_once(',')
                    .ok_or("--flight-look needs YAW,PITCH in degrees")?;
                flight_look = [yaw.parse().map_err(|e| bad_number(&arg, &e))?, pitch.parse().map_err(|e| bad_number(&arg, &e))?];
                if flight_look.iter().any(|v| !v.is_finite() || v.abs() > 360.) {
                    return Err(
                        "flight look angles must be finite and within -360..360 degrees".into(),
                    );
                }
            }
            "--flight-reference" => {
                flight_reference = match args.next().as_deref() {
                    Some("player") => flight_views::Reference::Player,
                    Some("target") => flight_views::Reference::Target,
                    Some("missile") => flight_views::Reference::Missile,
                    _ => return Err("--flight-reference needs player, target or missile".into()),
                };
            }
            "--flight-view" => {
                flight_view = args
                    .next()
                    .ok_or("--flight-view needs 0..11")?
                    .parse::<u8>().map_err(|e| bad_number(&arg, &e))?;
                // 13 and 14 are the replay's HUD camera and Sensor View.
                if flight_view > flight_views::MISSILE
                    && !matches!(flight_view, replay::tape_views::HUD_CAMERA | replay::tape_views::SENSOR)
                {
                    return Err("--flight-view needs 0..11".into());
                }
            }
            "--mission-brief" => { mission_brief = true; flight_map = true; initial_screen = Screen::Flight; }
            "--flight-map" => { flight_map = true; initial_screen = Screen::Flight; }
            "--weapon-diagnostics" => weapon_diagnostics = true,
            "--frame-log" => {
                let value = args.next_if(|arg| !arg.starts_with("--"));
                frame_log::configure(frame_log::threshold(value.as_deref().unwrap_or("on"))?);
            }
            "--debug-panels" => debug_panels = true,
            "--flight-panels" => {
                flight_panels = replay::viewer::Request::parse(
                    &args.next().ok_or("--flight-panels needs a list of panels")?,
                )?;
                debug_panels = true;
            }
            "--ejection-preview" => {
                let phase = args.next().ok_or("--ejection-preview needs seat, freefall or chute")?;
                if !matches!(phase.as_str(), "seat" | "freefall" | "chute") { return Err("--ejection-preview needs seat, freefall or chute".into()); }
                ejection_preview = Some(phase);
                initial_screen = Screen::Flight;
            }
            "--capture-flight" => {
                capture_terrain = Some(PathBuf::from(
                    args.next().ok_or("--capture-flight needs a PPM path")?,
                ));
                initial_screen = Screen::Flight;
                smoke_test = true;
            }
            "--controls-menu" => {
                controls_menu = true;
                flight_menu = true;
                initial_screen = Screen::Flight;
            }
            "--flight-menu" => {
                flight_menu = true;
                initial_screen = Screen::Flight;
            }
            "--free-flight" => initial_screen = Screen::Flight,
            "--flight-fault" => {
                let usage = "--flight-fault needs TICK:INDEX with a system fault index 0..44";
                let value = args.next().ok_or(usage)?;
                let (tick, index) = value.split_once(':').ok_or(usage)?;
                let (tick, index): (u64, usize) = (tick.parse().map_err(|_| usage)?, index.parse().map_err(|_| usage)?);
                if index > 44 || flight_faults.len() >= 64 {
                    return Err(usage.into());
                }
                flight_faults.push((tick, index));
            }
            "--flight-start" => {
                let usage = "--flight-start needs X,Z,HEADING_DEGREES,AGL_FEET (feet from the map's south-west corner)";
                let values: Vec<f64> = args
                    .next()
                    .ok_or(usage)?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| usage)?;
                if values.len() != 4
                    || values.iter().any(|v| !v.is_finite())
                    || !(10. ..=90_000.).contains(&values[3])
                {
                    return Err(usage.into());
                }
                flight_start = Some([values[0], values[1], values[2], values[3]]);
            }
            "--flight-fuel" => {
                let pounds: f64 = args.next().ok_or("--flight-fuel needs internal fuel in pounds")?.parse()?;
                if !pounds.is_finite() || !(0. ..=100_000.).contains(&pounds) {
                    return Err("--flight-fuel needs 0..100000 pounds".into());
                }
                flight_fuel = Some(pounds);
            }
            "--flight-cheat" => {
                let name = args.next().ok_or("--flight-cheat needs extra-g, no-g-effects, no-spins, no-crashes, unlimited-fuel, unlimited-ammo, easy-physics, invulnerable or realistic-damage")?;
                if !PROBE_CHEATS.contains(&name.as_str()) {
                    return Err("--flight-cheat needs extra-g, no-g-effects, no-spins, no-crashes, unlimited-fuel, unlimited-ammo, easy-physics, invulnerable or realistic-damage".into());
                }
                flight_cheats.push(name);
            }
            "--flight-trace" => {
                flight_trace_ticks = args.next().ok_or("--flight-trace needs a tick count")?.parse()?;
            }
            "--flight-probe-ticks" => {
                let ticks = args
                    .next()
                    .ok_or("--flight-probe-ticks needs a tick count")?
                    .parse::<usize>().map_err(|e| bad_number(&arg, &e))?;
                if ticks > 120 * 60 {
                    return Err("rendered flight probe limited to one minute".into());
                }
                flight_probe_ticks = Some(ticks);
            }
            "--flight-bay" => {
                let value: f64 = args.next().ok_or("missing bay fraction")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !value.is_finite() || !(0. ..=1.).contains(&value) {
                    return Err("--flight-bay requires 0..1".into());
                }
                flight_bay = Some(value);
            }
            "--flight-throttle" => {
                let value: f64 = args.next().ok_or("missing flight throttle")?.parse().map_err(|e| bad_number(&arg, &e))?;
                if !value.is_finite() || !(0. ..=1.).contains(&value) { return Err("--flight-throttle requires 0..1".into()); }
                flight_throttle = Some(value);
            }
            "--flight-devices" | "--flight-controls" => {
                let raw = args
                    .next()
                    .ok_or("animation preview needs comma-separated values")?;
                let values = raw
                    .split(',')
                    .map(|v| option_number::<f64>(&arg, v))
                    .collect::<Result<Vec<_>, _>>()?;
                let devices = arg == "--flight-devices";
                if values.len() != if devices { 5 } else { 3 }
                    || values
                        .iter()
                        .any(|v| !v.is_finite() || *v > 1. || *v < if devices { 0. } else { -1. })
                {
                    return Err("--flight-devices requires G,F,B,H,AB in 0..1; --flight-controls requires pitch,roll,rudder in -1..1".into());
                }
                if devices {
                    flight_devices = Some(values);
                } else {
                    flight_controls = Some(values);
                }
            }
            "--maneuver" => {
                maneuver = args.next().ok_or(
                    "--maneuver needs level, takeoff, pull, loop, roll, stall, spin, bank-left, bank-right or hover",
                )?;
                if ![
                    "level",
                    "takeoff",
                    "takeoff-gear-early",
                    "takeoff-gear-airborne",
                    "pull",
                    "loop",
                    "roll",
                    "stall",
                    "spin",
                    "spin-recover",
                    "stall-recover",
                    "climb",
                    "dive",
                    "overspeed",
                    "rudder",
                    "nosewheel",
                    "gcurve",
                    "sprint",
                    "devices",
                    "autopilot",
                    "waypoint",
                    "eject",
                    "eject-low",
                    "land",
                    "land-gear-up",
                    "land-hard",
                    "land-off-runway",
                    "bank-left",
                    "bank-right",
                    "hover",
                ]
                .contains(&maneuver.as_str())
                    && !maneuver.starts_with("settle:")
                {
                    return Err("unsupported maneuver".into());
                }
            }
            "--retail-stall-speeds" => flight::set_retail_stall_speeds(true),
            "--native-flight-report" => native_flight_report = true,
            "--native-flight-trig" => {
                native_flight_trig = Some(
                    args.next()
                        .ok_or("--native-flight-trig needs extracted sine-q15.bin path")?,
                );
                native_flight_report = true;
            }
            "--headless-flight" => {
                headless_ticks = Some(
                    args.next()
                        .ok_or("--headless-flight needs tick count")?
                        .parse::<usize>().map_err(|e| bad_number(&arg, &e))?,
                )
            }
            "--systems-preview" => {
                systems_preview = args.next().ok_or("--systems-preview needs comma-separated damage indices 1..35")?
                    .split(',').map(str::parse).collect::<Result<Vec<usize>, _>>()?;
                if systems_preview.iter().any(|i| !(1..=35).contains(i)) { return Err("--systems-preview indices must be 1..35".into()); }
            }
            "--target-cam-preview" => {
                let mode = args.next().ok_or("--target-cam-preview needs a mode")?;
                if instruments::gunsight::preview(&mode).is_none()
                    && instruments::targeting::preview(&mode).is_none()
                {
                    return Err(format!(
                        "--target-cam-preview modes: {}. Targeting page: {}",
                        instruments::gunsight::PREVIEW_MODES,
                        instruments::targeting::PREVIEW_MODES
                    )
                    .into());
                }
                target_cam_preview = Some(mode);
            }
            "--window-layout-preview" => {
                let mode = args.next().ok_or("--window-layout-preview needs a mode")?;
                if !WINDOW_LAYOUT_MODES.contains(&mode.as_str()) {
                    return Err(format!(
                        "--window-layout-preview modes: {}",
                        WINDOW_LAYOUT_MODES.join(", ")
                    )
                    .into());
                }
                window_layout_preview = Some(mode);
            }
            "--panel-snapshot" => {
                panel_snapshot = Some(args.next().ok_or("--panel-snapshot needs output path")?)
            }
            "--hud-snapshot" => {
                hud_snapshot = Some(args.next().ok_or("--hud-snapshot needs output path")?)
            }
            "--hud-snapshot-state" => {
                hud_snapshot_state = args.next().ok_or("--hud-snapshot-state needs forward, hover, converting or low")?
            }
            "--viewer" => initial_screen = Screen::Viewer,
            "--connect" => {
                session_args.connect =
                    Some(args.next().ok_or("--connect needs HOST or HOST:PORT")?);
            }
            "--find-games" => {
                find_games = Some(
                    args.next()
                        .and_then(|text| text.parse::<f64>().ok())
                        .filter(|seconds| (0.1..=3600.0).contains(seconds))
                        .ok_or("--find-games needs a number of seconds, 0.1 to 3600")?,
                );
            }
            "--browse" => {
                browse_games = Some(
                    args.next()
                        .and_then(|text| text.parse::<f64>().ok())
                        .filter(|seconds| (0.1..=3600.0).contains(seconds))
                        .ok_or("--browse needs a number of seconds, 0.1 to 3600")?,
                );
            }
            "--map-port" => {
                map_port = Some(
                    args.next()
                        .and_then(|text| text.parse::<f64>().ok())
                        .filter(|seconds| (0.0..=3600.0).contains(seconds))
                        .ok_or("--map-port needs a number of seconds, 0 to 3600")?,
                );
            }
            "--host" => {
                session_args.host = Some(PathBuf::from(
                    args.next().ok_or("--host needs a mission file")?,
                ));
            }
            "--callsign" => {
                session_args.callsign = Some(args.next().ok_or("--callsign needs a name")?);
            }
            "--slot" => {
                session_args.slot = Some(args.next().ok_or("--slot needs a plane number")?)
            }
            "--password" => {
                session_args.password = Some(args.next().ok_or("--password needs the password")?)
            }
            "--port" => session_args.port = Some(args.next().ok_or("--port needs a port number")?),
            "--name" => session_args.name = Some(args.next().ok_or("--name needs a name")?),
            "--list" => session_args.list = true,
            "--master" => {
                session_args.master = Some(args.next().ok_or("--master needs HOST or HOST:PORT")?)
            }
            "--open-planes" => {
                session_args.open_planes = Some(
                    args.next()
                        .ok_or("--open-planes needs friendly, all or plane numbers")?,
                )
            }
            "--watch-replay" => {
                watch_replay = Some(PathBuf::from(
                    args.next().ok_or("--watch-replay needs a recording")?,
                ));
                initial_screen = Screen::Replay;
            }
            "--capture-replay" => {
                replay_capture = Some(PathBuf::from(
                    args.next().ok_or("--capture-replay needs a .ppm or .png path")?,
                ));
                smoke_test = true;
            }
            "--input-script" => {
                let path = PathBuf::from(args.next().ok_or("--input-script needs a script file")?);
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                input_script_steps = Some(
                    input_script::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
                );
            }
            "--replay-tick" => {
                replay_options.tick = Some(option_number(
                    "--replay-tick",
                    &args.next().ok_or("--replay-tick needs a tick")?,
                )?);
            }
            "--replay-aircraft" => {
                replay_options.aircraft = Some(option_number(
                    "--replay-aircraft",
                    &args.next().ok_or("--replay-aircraft needs an aircraft id")?,
                )?);
            }
            "--replay-drone" => replay_options.drone = true,
            "--replay-speed" => {
                replay_options.speed = Some(option_number(
                    "--replay-speed",
                    &args
                        .next()
                        .ok_or("--replay-speed needs a speed such as 16 or -2")?,
                )?);
            }
            "--replay-ui" => {
                replay_options.ui = replay::viewer::Ui::parse(
                    &args.next().ok_or("--replay-ui needs a list of parts")?,
                )?;
            }
            "--replay-clean" => replay_options.ui.hidden = true,
            "--replay-menu" => {
                replay_options.menu = Some(replay::pause::Start::parse(
                    &args.next().ok_or("--replay-menu needs ?, pref, time, help, graphics, sound or controls")?,
                )?);
            }
            "--replay-look-at" => {
                replay_options.look_at = Some(replay::viewer::parse_object(
                    &args
                        .next()
                        .ok_or("--replay-look-at needs aircraft:ID, ground:ID or weapon:ID")?,
                )?);
            }
            "--replay-panels" => {
                replay_options.panels = replay::viewer::Request::parse(
                    &args.next().ok_or("--replay-panels needs a list of panels")?,
                )?;
            }
            "--quick-mission" => initial_screen = Screen::Quick,
            "--background" => {
                background = Some(args.next().ok_or("--background needs an asset name")?)
            }
            "--snapshot-state" => {
                snapshot_state = args.next().ok_or("--snapshot-state needs a state name")?
            }
            "--import" => {
                import = Some(PathBuf::from(
                    args.next()
                        .ok_or("--import needs an installed folder or a disc folder")?,
                ))
            }
            "--snapshot" => {
                snapshot = Some(PathBuf::from(
                    args.next().ok_or("--snapshot needs a .ppm output path")?,
                ))
            }
            "--smoke-test" => smoke_test = true,
            "--windowed" => windowed_flag = true,
            flag @ ("--anti-aliasing" | "--render-scale" | "--spotting-aid"
            | "--terrain-filtering" | "--building-distance" | "--building-detail"
            | "--vegetation-texture" | "--vegetation-texture-distance"
            | "--vegetation-distance" | "--vegetation-detail" | "--vegetation-density"
            | "--shadows" | "--shadow-distance" | "--graphics-api") => {
                let value = args.next().ok_or(format!("{flag} needs a value"))?;
                graphics_flags.push((flag.to_owned(), value));
            }
            "--original-graphics" => graphics_flags.push((arg.clone(), String::new())),
            "--no-audio" => no_audio = true,
            "--version" | "-V" => {
                println!("{}", version::compatibility_label());
                return Ok(Outcome::Done);
            }
            "--import-only" => import_only = true,
            "--validate-creator" => validate_creator = true,
            "--validate-tanks" => validate_tanks = true,
            "--validate-wing-loadout" => wing_loadout_probe = Some(PathBuf::from(args.next().ok_or("--validate-wing-loadout needs an output directory")?)),
            "--validate-ordnance" => validate_ordnance = true,
            "--animation-probe" => {
                animation_probe = Some(args.next().ok_or("--animation-probe needs an output directory")?.into());
            }
            "--sensor-summary" => sensor_summary = true,
            "--sensor-channel" => {
                sensor_channel = Some(
                    match args
                        .next()
                        .ok_or("--sensor-channel needs radar or ir")?
                        .as_str()
                    {
                        "radar" => 0,
                        "ir" => 1,
                        _ => return Err("--sensor-channel needs radar or ir".into()),
                    },
                );
            }
            "--scope-range" => {
                let nmi: f64 = args
                    .next()
                    .ok_or("--scope-range needs a recovered scope setting in nautical miles")?
                    .parse().map_err(|e| bad_number(&arg, &e))?;
                scope_range = Some(
                    tore_sim::sensors::RANGE_LADDER_NMI
                        .iter()
                        .position(|v| *v == nmi)
                        .ok_or("--scope-range needs 5, 10, 25, 50, 100 or 150")?,
                );
            }
            "--scope-history" => scope_history = true,
            "--validate-weather" => validate_weather = true,
            "--validate-maps" => validate_maps = true,
            "--validate-ils" => validate_ils = true,
            "--airport-allegiance" => airport_allegiance = true,
            "--validate-text" => validate_text = true,
            "--weather-condition" => {
                let value: usize = args
                    .next()
                    .ok_or("--weather-condition needs 0..5")?
                    .parse().map_err(|e| bad_number(&arg, &e))?;
                if value >= tore_sim::environment::CONDITIONS.len() {
                    return Err("--weather-condition needs one of the six source choices".into());
                }
                weather_condition = Some(value);
            }
            "--help" | "-h" => {
                println!(
                    "Visuals: --ejection-preview seat|freefall|chute inspects imported escape poses with --capture-flight. --hud-target-preview bearing,elevation,feet inspects selected-target cues with --capture-flight. --gun-flash-preview OUT_DIR (first argument) renders the AC-130 muzzle flashes, gun light, blast smoke and 105 mm tracer offscreen by day, dusk and night, outside and through the gunsight camera (TORE_PREVIEW_THEATER picks the theater). --blast-preview OUT_DIR (first argument) renders a large ground explosion's shockwave ring through its two seconds, and a water explosion's, offscreen. --impact-preview OUT_DIR (first argument) renders weapon impacts side by side (20 mm, 105 mm, 250, 500 and 2,000 lb bombs) with their light, sparks and smoke, by day and at night, offscreen; add shockwave after OUT_DIR for a 500 lb bomb, a 2,000 lb bomb and an AGM-65G from 0.1 to 60 seconds (shockwave ring, vapor and lingering dust; TORE_PREVIEW_THEATER picks the theater, so its climate), or night-range for bombs and fires at night from 0.3 to 30 nm (the night glow; TORE_NIGHT_GLOW=0 for without). --replay-stills REPLAY FRAME_PLAN OUT_DIR (first argument) draws a recording's director frames offscreen as PNG files. --cockpit-sheet OUT_DIR [ID,ID,...] (first argument) renders every flyable aircraft's cockpit with live mirrors, its cockpit art with the mirror masks tinted, and two contact sheets, offscreen. --dome-sheet OUT_DIR (first argument) renders the E-3, IL-76 and E-2 radar domes at six ticks to show their turn, offscreen. --surface-preview OUT_DIR (first argument) writes contact sheets of the surface-unit shapes (ships, launchers, soldiers) and their damaged shapes from four sides, read from the retail archives without opening a window (add destroyed for each class beside its destroyed look). --surface-scene OUT_DIR THEATER STEM renders a ground target in game offscreen after --seconds, with --kill, --rails and --focus (docs/DEVELOPMENT.md). --hud-a2g-preview OUT_DIR (first argument) writes review frames and sheets of the HUD's bomb and rocket sights (CCIP, the level T, the rocket pipper, LOW and FRAG) without a window. --hud-guided-preview OUT_DIR (first argument) writes review frames of the guided weapons' release range scale (JDAM, laser bomb and laser missile) without a window. --sight-preview heading,elevation,ticks holds the AC-130 sight at a body-relative look and runs the sim that many ticks first, so --capture-flight shows the aim-point box and diamond (add --combat-command sight-pin for a pinned point). --damage-preview 0..1 with --capture-flight inspects original damage bodies and two seconds of smoke. --countermeasure-preview TICKS advances flight and combat after the setup commands, so --combat-command chaff/flare captures show the devices developing.\nCreator: --dummy-aircraft ID,COUNT adds straight-flight fixtures one mile ahead (repeat for mixed aircraft). --quick-mission opens setup; --snapshot-state ordnance opens the loadout preview; --snapshot-state map (or map-lobby) draws the creator's Map of both sides' starts and the objective area, and enemy-ground-start, enemy-ground-start-auto and enemy-airports show the Enemy start rows; --validate-creator checks all imported loadouts and restart without a display; --validate-ordnance checks source station coverage, weapon/tank availability and saved loads for every reviewed aircraft; --validate-wing-loadout OUT_DIR records a four-ship F-16 custom load before and after sharing and checks ground starts and restart.\nCombat: --live-fire starts an explicit PT-default range. Space fires; [ and ] cycle NAV/weapons; T designates; backslash resets target. --weapon-slot N selects a 1-based weapon slot. --loadout none|guns starts with every store off, or everything but the gun off (the Guns only restriction), as the Load Ordnance page leaves them. --combat-command NAME applies a manual setup command before the probe. Shift-K jettisons tanks; Ctrl-Shift-K jettisons the selected external group; ; or L clears designation; Insert/Delete release chaff/flare; Use --combat-command class/fail for damage-class and station-fault fixtures. D reports ownship damage and systems in the sim log; Ctrl-Shift-I launches one incoming selected weapon; Shift-Y toggles target ECM; J toggles own ECM (--jammer-on starts powered). Select is the gamepad combat modifier; see INPUT.md. --record-combat NEW_PATH writes version-7 combat-service inputs, including sensor controls and wreck body presence; --replay-combat PATH replays them headlessly with matching --aircraft/--theater and assets. --gunsight-probe pin-orbit|fire-no-target|track-out-of-arc (with --aircraft ac130) runs a scripted AC-130 gunsight check on flat ground and prints PASS or FAIL lines, and --gunsight-dump PATH writes the orbit's gun train as CSV; --combat-smoke runs all default slots and five damage classes; TORE_COMBAT_EVIDENCE=DIR also roundtrips per-slot tapes. --combat-probe-ticks 1..7200 advances a scripted firing pass before --capture-flight.\nAI wings: Quick Mission uses AI by default, with separate friendly and enemy delta formations. --ai-wings opens the creator; --fixture-wings retains the old straight-flight setup. --ai-mission free|cap|intercept|escort|self-defense|hold selects the next Quick Mission policy; free is the default. --enemy-skill novice|average forces every enemy aircraft to that level for this session only (the original's persistence of this preference is untraced). --probe-matrix NEW_DIR records the 1,008-case F-22/opponent/skill/geometry/adapter suite using --ai-probe-ticks. --probe-enemy-aircraft ID, --probe-enemy-skill novice|average|experienced|ace, --probe-geometry head|rear|side, --probe-guns (player), --probe-ai-guns-only (AI stores), --probe-flight-model legacy|researched, --probe-ai-flight-model standard|all-hybrid and --probe-threat TICK:hit|gun|aaa configure encounter probes. --probe-fault TICK:INDEX injects a reviewed system fault (0..44) into the first enemy through the normal damage bridge. --ai-probe-ticks 1..288000 runs a headless AI mission and prints a deterministic per-actor summary; with --ground-start it also prints phase transitions and ground hazards. --maneuver takeoff flies the player off the ground start and cruises on the autopilot; --probe-wing-size 1..5 sizes the player's wing; --probe-fight FRIENDLY:ENEMY sizes a whole battle (1..15 a side, five to a wing) and --probe-friendly-aircraft ID picks the friendly AI aircraft; --probe-wing-only removes all other wings for isolated probes or creator captures; --probe-redfor (with --probe-wing-only and --ground-start at a Redfor field) seats the probe's wing on Redfor, as Enemy Wing 1 of an open mission whose parked lead the probe takes by handoff; --probe-wing-order TICK:bug-out|land-selected|attack-on-contact|engage-my-target|sort orders all wingmen, or one with a trailing @MEMBER (1..4, the first wingman is 1); --probe-player-lock TICK:ID has the player designate aircraft ID at that tick, so the sensors lock it as they would for a human; --probe-data-link and --probe-player-lock print a `data link:` line for each member (with its radar flag) and each lock taken or dropped in the flight data link's picture; --probe-blind-wing takes the sensors from the player's wingmen, so only the data link can show them an enemy; --probe-player-home FROM:UNTIL flies the player gear down over the departure field; --probe-lose-player TICK crashes the player's aircraft at that tick; --probe-wing-route EAST_NM:NORTH_NM:ALT_FT (repeatable) gives the player's wing waypoints, flown by an AI that takes the lead from the lost player once its search finds nothing; --probe-attack TICK[:SECONDS] has the scripted leader designate the nearest hostile aircraft, select a weapon and fire from that tick, attacking again SECONDS after each shot. --ai-observe-hz 120|60|40|30|20 and --ai-lean-sensors [on|off] are AI test settings (also in the flight menu's Debug tab, which they start): the first makes AI aircraft observe that many times a second, staggered across ticks, with the controller and flight model still at 120 Hz; the second has them skip the visual channel and build the received-radar list only when a warning needs it. Defaults are 60 and on (60 behaves as 120 within a fight's chaos; lean sensors decide exactly as full ones do); the probe prints an `AI probe test settings:` line only when they differ from the defaults. --separation 1|2|5|10|20|50|75|100|150|200|300 sets the Quick Mission enemy distance in nautical miles.\nMissiles: click CUED/BORESIGHT or bind weapon-seeker-mode. --missile-acceptance runs controlled reach probes. --compatibility-weapons retains prior weapon rules independently of the flight model.\nSensors: one shared radar/infrared component serves every imported aircraft. M cycles the available channels, I selects infrared, R returns to radar, Y toggles contact history, comma/period change the scope setting and a click designates a contact. --sensor-summary prints each aircraft's imported capability; --sensor-channel radar|ir, --scope-range 5|10|25|50|100|150 and --scope-history set the scope for a headless capture. Guidance/contact/damage coupling is a development approximation, not native parity."
                );
                println!(
                    "Multiplayer: --connect HOST[:PORT] joins a dedicated server (docs/DEDICATED-SERVER.md); --callsign NAME (1 to 15 printable ASCII characters), --slot N (the plane to take) and --password TEXT go with it. --host MISSION_FILE hosts a game of that mission file (the dedicated server's format) and flies in it: other players join with --connect; --port N (default 26900), --name TEXT, --open-planes friendly|all|N,N and --password TEXT (the password joining players must give) set the game, and --callsign and --slot are the hosting player's own. --list also lists the hosted game on the Internet Lobby, on the master --master HOST[:PORT] names (default the public one). --find-games SECONDS [--port N] looks for games on the local network for that long, prints each one found (address, build, name, mission, players, phase, King, password, full) and exits. --browse SECONDS [--master HOST[:PORT]] lists the games on the Internet Lobby for that long, with each one's mission and players, and exits. A hosting game asks the router to forward its port (UPnP, NAT-PMP or PCP) while Options > Forward the game port on my router is on, and removes it when hosting stops; --map-port SECONDS [--port N] does the same by itself for that long and exits, to check that the router answers."
                );
                println!(
                    "Replays: --watch-replay FILE plays a mission recording (docs/REPLAYS.md). With it, --capture-replay OUT.ppm writes one GPU frame and exits (OUT.png saves the clean view as P does); --replay-tick N pauses at a tick; --flight-view 0..11 and --replay-aircraft ID choose the view; --replay-drone starts in the follow drone; --replay-speed 0.125..16 starts playing at that speed, negative for reverse; --replay-ui labels,timer,trails,marks,banner,comms,subtitles chooses the interface parts; --replay-panels thought,telemetry,guidance,comms,menu opens debug panels, or the right-click menu, on the selected aircraft; --replay-clean starts with the interface hidden, as H hides it; --replay-menu ?|pref|time|help|graphics|sound|controls opens the Escape menu at that page, or that screen over it; --replay-look-at aircraft:ID, ground:ID or weapon:ID starts in the object view looking at it."
                );
                println!(
                    "Controllers: --no-controllers, --record-input NEW_PATH, --replay-input PATH, --list-inputs, --monitor-inputs SECONDS, --write-input-profile NEW_PATH, --input-profile PATH, --test-rumble DEVICE_ID|only, --controls-menu. See docs/INPUT.md.\nInstrument focus: Ctrl-Tab / Ctrl-Shift-Tab, Ctrl-1..6; Ctrl-Shift-1..4 operates selected instrument buttons.\nScripted input: --input-script FILE feeds key presses and mouse clicks to a windowed run through the window's own handlers, for tests (steps: wait, waittick, key, down, up, move, movemenu, click, press, release, wheel, snapshot, exit; see docs/DEVELOPMENT.md)."
                );
                println!(
                    "Mission recordings: every flight records what happened into replays/ in the data folder; Ctrl+B marks a moment (TORE_RECORD_MISSIONS=0 turns recording off for a run). These are not the --record-input/--replay-input or --record-combat/--replay-combat tapes, which store inputs and simulate them again. --recording-info FILE describes a recording. --recording-log FILE [--out DIR] [--from SECONDS] [--to SECONDS] [--ids 0,7] [--rate HZ] writes log.jsonl, summary.txt and feedback.jsonl. --recording-acmi FILE [--out FILE] [--rate HZ] [--guns] writes a Tacview .txt.acmi file. --recording-diff A B compares two recordings. --convert-capture CAPTURE [--out REPLAY] turns a networked flight's capture (replays/*.tore-capture) into a replay, smoothed through every update received; it needs the import and takes the replay's name from the capture unless --out names it. --ai-probe-ticks N --record-mission NEW_PATH records a headless probe without changing its output; --verify-render then checks every recorded tick redraws the picture the probe drew. See docs/REPLAYS.md."
                );
                println!(
                    "Usage: tore-app [--free-flight | --viewer | --quick-mission] [--theater CODE] [--capture-terrain OUTPUT.ppm] [--import MEDIA_DIR] [--import-only] [--no-audio] [--smoke-test] [--snapshot OUTPUT.ppm] [--snapshot-state STATE] [--background NAME]\n\nImports original menus, all theaters and the 36 reviewed retail aircraft into platform application data. See docs/spec/aircraft-variety.md for the expanded roster.\n--import MEDIA_DIR takes an installed Fighters Anthology folder, or the folder of a mounted disc 1 holding SETUP.ESA (the container path itself is also accepted). A raw .iso is not read: mount it and choose the mounted folder.\nOn first run without --import the remembered source is used, otherwise a local gameassets/fighters-anthology directory.\n--aircraft ID selects a reviewed aircraft (default f18), including c130, ac130, e3, il76, e2, av8, yak141, v22, ah64, mi24, ch47, mig17, f4b, f4j, f4e, f4g, a7, f15, f16c, f104, a10, b747 and a310. Existing identities and faxx remain available.\n--free-flight launches the selected aircraft; --headless-flight TICKS runs without a display.\n--launch-quick-mission launches the creator setup directly.\n--ground-start AIRPORT_NUMBER selects a runway start, or presets Ground in --quick-mission (which refuses an enemy airfield; a STOL strip is allowed). The researched flight model is required.\nUse --ground-start N --headless-flight TICKS --maneuver takeoff for a deterministic rollout probe. --enemy-ground-start N parks Enemy Wing 1 at Redfor airport N (the creator's Enemy start).\nFlight: Shift-arrows look/orbit, keypad 5 or Shift-/ recenter. Arrows pitch/bank, End/PageDown or Z/X rudder, 1-5 throttle idle to 100%, 6 afterburner, 7/8 throttle -/+5%, Insert/Delete chaff/flare, Shift-E twice to eject. F1 front, F2 back, F3 up, F4 track, F5 threat, F6 wing, F7 player-target, F8 target-player, F9 fly-by, F10 external, F12 missile-target. Alt/Ctrl+view references target/last missile (Alt-F4 exits). V saves Other View. Shift-0..9 instruments. Esc > Pref > Large windows? switches four-corner/six-bottom layouts. Esc flight menu, Ctrl-P pause, Backspace cockpit, F11 keyboard help. See docs/FLIGHT-CONTROLS.md.\n--quick-mission opens the creator; --viewer opens the selected theater.\n--theater CODE selects a base theater or imported layout variant, such as ~UKR1 (default UKR). --validate-maps constructs every imported map without a display. --validate-ils checks the ILS alignment at every airport. --airport-allegiance lists every base theater's ground-start airports for Blue and for Redfor and checks that none is the other side's.
Weather: --weather-condition 0..5 selects one of the six source choices (clear, cloudy, foggy, dawn, sunset, night); --validate-weather checks every imported module, one full simulated day and every choice without a display. TORE_WEATHER_TIME=HH:MM overrides the launch time for matched captures; TORE_VAPOR_PROBE=1 prints the resolved wing vapor trail headlessly.\n--capture-flight PATH captures flight with instruments; --flight-view 0..11 chooses front/external/oblique/back/up/track/threat/wing/player-target/target-player/fly-by/missile-target. --flight-reference player/target/missile selects the reference. --flight-menu captures the paused menu. --flight-map opens the Shift-M map. --weapon-diagnostics shows the upper-right weapon diagnostic panel (Escape > Pref > Weapon diagnostics? in flight). --debug-panels turns on the mission timer, right-click menu and debug panels (Escape > Pref > Debug panels?); --flight-panels thought,telemetry,guidance,comms,menu also opens them. --flight-look YAW,PITCH sets look angles in degrees for inspection. --flight-zoom 0.5..4 sets initial zoom.\n--flight-throttle 0..1 sets initial throttle for material inspection. --flight-bay 0..1 sets an F-22 main-bay pose. O toggles bays in flight.\n--flight-devices G,F,B,H,AB sets initial fractions (0..1); --flight-controls pitch,roll,rudder sets initial deflections (-1..1). Animation captures pause at the specified pose. --animation-probe OUT sweeps the selected aircraft through control/device poses using the actual transformed drawing geometry, without a window, and writes local geometry metrics and contact sheets.\n--instrument-layout large/small selects four corners or six bottom windows. --window-layout-preview before|launch|free|landed (with --panel-snapshot, --window-size, --instrument-layout) draws the whole window layout on a plain backdrop without a GPU, and with --capture-flight or in flight sets the windows the same way: before = every slot taken and the target camera closed, launch = a seeker launch has opened the target camera (pop-up), free = the same with free slots, landed = the weapon has landed.\n--panel-snapshot PATH writes one instrument; --systems-preview 12,13,14 injects panel-only faults and advances --flight-probe-ticks (default 1200); --instrument-page 0..9 selects it. --target-cam-preview MODE (with --panel-snapshot, default page 4) draws the AC-130 gunsight page from a synthetic readout on a synthetic scene: free, pinned, tracked, outside, range, close, mask, nolos, empty, returning, gimbal, gimbal-text, gimbal-bitmap, zoom1..zoom6; the targeting page of a laser or FLIR aircraft (lase, lase-off, lase-mask, lase-mask-range, lase-mask-terrain, lase-pin, ready, slew, limit, tti, impact, buddy, both, buddy-nosight, flir, flir-tracked, seeker). --hud-snapshot PATH with --hud-snapshot-state laser-lsr, laser-bud, laser-both, laser-ready, laser-mask, laser-edge, tti, tti-lsr, tti-many or tti-impact draws the HUD laser box and text and the time to impact row.\n--native-flight-tables DIR enables airborne native research using extracted sine/atan tables; environmental turbulence and native contact/lifecycle producers are unavailable.\n--researched-flight explicitly selects the default hybrid flight/contact model (not native parity). --retail-stall-speeds turns the weight-scaled stall speed off, so the imported envelope's slow edges apply at every weight (developer switch). --legacy-flight selects the previous compatibility model.\n--native-flight-report prints static-translated helper probes (not a native simulation). --native-flight-trig PATH additionally probes an extracted sine-q15.bin table.\n--headless-flight TICKS supports --maneuver level/pull/loop/roll/stall/spin/bank-left/bank-right; --maneuver hover starts the AV-8, Yak-141, V-22 or a helicopter at rest in its own hover trim (hands off it holds; --replay-input flies it). --flight-probe-ticks TICKS advances that maneuver before a rendered flight (maximum 7200 ticks).\n--capture-terrain writes a GPU-rendered 960x720 terrain PPM and exits (display required).\nGraphics for one run: --anti-aliasing off/2x/4x/8x, --render-scale 75/100/125/150/200, --spotting-aid off/subtle/strong, --terrain-filtering on/off; --original-graphics turns every addition off. --graphics-api vulkan/dx12 chooses the graphics API for one run on Windows; auto asks for the default.\nViewer: arrows move; Shift speeds up; Q/E or PageDown/PageUp change altitude; A/D turn; W/S pitch; Escape returns.\n--snapshot writes a headless 640x480 menu preview and exits (supports --quick-mission).\n--snapshot-state: normal, hover, pressed, help, pref, multi, notice, internet, internet-games, internet-joining, internet-options, internet-unreachable, controls, controls-keyboard, controls-mouse, controls-head, controls-search, controls-search-keys, controls-inv, graphics, sound, replays, replays-settings, replays-delete, locate, locate-graphics-api, locate-importing, locate-done. Quick mission (with --quick-mission): normal, aircraft, theaters, help, objectives, ground-start, ground-start-auto, airports, ground-target, ground-target-last, enemy-ground-start, enemy-ground-start-auto, enemy-airports, map, map-building, map-wide, map-close, map-lobby (the mission map), objective-1 through objective-6 (the group order popups), field-3 through field-36 (the setting popups), ordnance, ordnance-empty, ordnance-drag, ordnance-message, ordnance-message-long, and debrief, debrief-2 to debrief-5, debrief-success.\n--background: CHOOSEAC, CHOOSE3, CHOOSEU, CHOOSEM, CHOOSEV (default: random; snapshots use CHOOSEV).\n--smoke-test presents one frame without audio and exits.\nThe game starts in borderless fullscreen; --windowed starts in a window, as --window-size, --smoke-test and the captures already do. Alt-Enter switches at any time and the choice is remembered.\n--frame-log [MS] enables slow-frame diagnostics (default 50 ms); TORE_FRAME_LOG=MS does the same. Escape > Debug > Slow-frame log toggles it for this session.\nTORE_DATA_DIR overrides the application data directory. TORE_LOG_DIR overrides diagnostic logs; TORE_NO_ERROR_DIALOG=1 suppresses failure dialogs.\n--diagnostics-self-test[=error|panic|worker-panic|graphics|dialog|native-crash|native-crash-window|stack-overflow] checks reporting without retail media; the native crash tests are deliberate, Windows only and may be combined with --graphics-api.\nTab/arrows + Enter navigate; Escape dismisses; ? contains Exit."
                );
                return Ok(Outcome::Done);
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    if let Some(seconds) = find_games {
        let port = net::options::find_games_port(&mut session_args)?;
        net::search::find_games(seconds, port)
            .map_err(|error| format!("--find-games could not search: {error}"))?;
        return Ok(Outcome::Done);
    }
    if let Some(seconds) = map_port {
        let port = net::options::map_port_port(&mut session_args)?;
        let forward = net::hosting::forward::choose_explicit();
        let done = net::hosting::forward::map_port(
            forward,
            port,
            std::time::Duration::from_secs_f64(seconds),
            std::time::Duration::from_secs(8),
            &mut std::io::stdout(),
        )
        .map_err(|error| format!("--map-port could not print: {error}"))?;
        if !done {
            return Err("--map-port: the router did not forward the port".into());
        }
        return Ok(Outcome::Done);
    }
    if let Some(seconds) = browse_games {
        let data = assets::data_directory().ok();
        let remembered = data
            .as_deref()
            .and_then(|data| net::settings::Remembered::load(data).master);
        let master = net::browse::browse_master(&mut session_args, remembered.as_deref())?;
        net::browse::browse_games(seconds, &master, false)
            .map_err(|error| format!("--browse failed: {error}"))?;
        return Ok(Outcome::Done);
    }
    let connect = session_args.session(
        snapshot.is_some()
            || import_only
            || capture_terrain.is_some()
            || ai_probe.is_some()
            || headless_ticks.is_some()
            || launch_creator
            || watch_replay.is_some()
            || record_input.is_some()
            || replay_input.is_some()
            || smoke_test,
    )?;
    if watch_replay.is_none()
        && (replay_capture.is_some() || replay_options != replay::viewer::Options::default())
    {
        return Err("--capture-replay and the --replay-* options need --watch-replay FILE".into());
    }
    if watch_replay.is_some()
        && (snapshot.is_some()
            || import_only
            || capture_terrain.is_some()
            || ai_probe.is_some()
            || headless_ticks.is_some()
            || launch_creator
            || record_input.is_some()
            || replay_input.is_some())
    {
        return Err("--watch-replay opens the replay viewer and cannot combine with other captures, probes or recordings".into());
    }
    if watch_replay.is_some() && std::env::args().any(|a| a == "--flight-view") {
        replay_options.view = Some(flight_view);
    } else if flight_view > flight_views::MISSILE {
        return Err("--flight-view 13 and 14 are replay views: use --watch-replay".into());
    }
    if fixture_wings
        && (ai_wings_enabled || enemy_skill.is_some() || ai_mission != ai_wings::Preset::Free)
    {
        return Err(
            "--fixture-wings cannot combine with AI wings, an AI probe or enemy skill".into(),
        );
    }
    if ai_wings_enabled
        && (live_fire
            || !dummy_aircraft.is_empty()
            || combat_probe.is_some()
            || combat_smoke
            || gunsight_probe.is_some()
            || missile_acceptance
            || native_tables_path.is_some()
            || record_combat.is_some()
            || replay_combat.is_some()
            || record_input.is_some()
            || replay_input.is_some()
            || headless_ticks.is_some())
    {
        return Err(
            "--ai-wings flies a Quick Mission; the range, dummy fixtures, combat/input tapes, native research flight and headless flight have their own paths"
                .into(),
        );
    }
    if ai_probe.is_some() && (capture_terrain.is_some() || snapshot.is_some() || import_only) {
        return Err("--ai-probe-ticks is a headless probe and cannot capture or snapshot".into());
    }
    if (probe_script.enemy_aircraft.is_some()
        || (probe_script.fight.is_some() && !launch_creator)
        || probe_script.friendly_aircraft.is_some()
        || probe_script.enemy_skill.is_some()
        || probe_script.geometry != ProbeGeometry::Head
        || probe_script.guns
        || probe_script.ai_guns_only
        || probe_script.blind_wing
        || probe_script.researched
        || probe_script.ai_flight_model != ai_wings::AiFlightModel::Standard
        || !probe_script.threats.is_empty()
        || probe_script.matrix.is_some())
        && (ai_probe.is_none() || ai_roster_probe)
    {
        return Err("encounter probe options require --ai-probe-ticks".into());
    }
    if probe_script.attack.is_some() && (ai_probe.is_none() || ai_roster_probe) {
        return Err("--probe-attack scripts the leader of an --ai-probe-ticks run".into());
    }
    if ai_wings_enabled && ai_probe.is_none() {
        // The bridge reads the Quick Mission setup screen, so the flag opens it.
        initial_screen = Screen::Quick;
    }
    if !dummy_aircraft.is_empty()
        && (live_fire
            || native_tables_path.is_some()
            || record_input.is_some()
            || replay_input.is_some()
            || replay_combat.is_some()
            || headless_ticks.is_some())
    {
        return Err("dummy aircraft require normal desktop flight; range/research/replay/headless-flight modes have separate fixtures".into());
    }
    if countermeasure_preview.is_some() && capture_terrain.is_none() {
        return Err("--countermeasure-preview requires --capture-flight".into());
    }
    if target_cam_preview.is_some() && panel_snapshot.is_none() {
        return Err("--target-cam-preview requires --panel-snapshot".into());
    }

    if !systems_preview.is_empty() && panel_snapshot.is_none() {
        return Err("--systems-preview requires --panel-snapshot".into());
    }
    if damage_preview.is_some()
        && (capture_terrain.is_none()
            || native_tables_path.is_some()
            || record_input.is_some()
            || record_combat.is_some()
            || replay_combat.is_some()
            || replay_input.is_some())
    {
        return Err("--damage-preview requires --capture-flight and cannot record/replay or use native research flight".into());
    }
    if sight_preview.is_some() && capture_terrain.is_none() {
        return Err(
            "--sight-preview requires --capture-flight and an AC-130 (--aircraft ac130)".into(),
        );
    }
    if hud_target_preview.is_some()
        && (capture_terrain.is_none()
            || native_tables_path.is_some()
            || record_input.is_some()
            || record_combat.is_some()
            || replay_combat.is_some()
            || replay_input.is_some())
    {
        return Err("--hud-target-preview requires --capture-flight without recording/replay or native research flight".into());
    }
    let researched_flight = flight_mode_arg.unwrap_or(native_tables_path.is_none());
    let native_tables = if let Some(path) = native_tables_path {
        if flight_mode_arg.is_some() || live_fire || combat_probe.is_some() || combat_smoke {
            return Err(
                "native research flight cannot combine with explicit hybrid/legacy or combat modes"
                    .into(),
            );
        }
        use std::io::Read;
        let read = |name: &str, limit: u64| -> AppResult<Vec<u8>> {
            let mut bytes = Vec::new();
            std::fs::File::open(path.join(name))?
                .take(limit + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() != limit as usize {
                return Err(format!("{name}: incorrect table length").into());
            }
            Ok(bytes)
        };
        Some(std::sync::Arc::new(tore_sim::native::Tables::parse(
            &read("sine-q15.bin", 642)?,
            &read("atan-pa.bin", 1028)?,
        )?))
    } else {
        None
    };
    if capture_terrain.is_some()
        && (snapshot.is_some()
            || import_only
            || !matches!(initial_screen, Screen::Viewer | Screen::Flight))
    {
        return Err("scene capture requires flight/viewer and cannot combine with --snapshot or --import-only".into());
    }
    if snapshot.is_none() && !smoke_test && snapshot_state != "normal" {
        return Err("--snapshot-state requires --snapshot or --smoke-test".into());
    }
    if let Some(path) = &snapshot
        && snapshot_state.starts_with("loading")
    {
        use std::io::Write;
        let (photo, window) = loading::snapshot_window(&snapshot_state)
            .ok_or_else(|| format!("unknown loading snapshot: {snapshot_state}"))?;
        // Headless, before any window: decode here, then sample.
        loading::prepare_photos();
        let mut pixels = Vec::new();
        let size = loading::draw_window(
            &mut pixels,
            window,
            photo,
            loading::Stage::Scenery,
            2.5,
            ["", ""],
        );
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(out, "P6\n{} {}\n255\n", size[0], size[1])?;
        for p in pixels.chunks_exact(4) {
            out.write_all(&p[..3])?;
        }
        println!("Menu preview: {}", path.display());
        return Ok(Outcome::Done);
    }
    // The locate screen owns no imported media, so its preview is written
    // before any pack is looked for.
    if let Some(path) = &snapshot
        && snapshot_state.starts_with("locate")
    {
        locate_snapshot(path, &snapshot_state)?;
        return Ok(Outcome::Done);
    }
    // Mission recording tools read a recording file and need no media.
    let recording_commands = [
        recording_info.is_some(),
        recording_log.is_some(),
        recording_acmi.is_some(),
        recording_diff.is_some(),
        convert_capture.is_some(),
    ]
    .into_iter()
    .filter(|on| *on)
    .count();
    if recording_commands > 1 {
        return Err("use one --recording-* or --convert-capture command at a time".into());
    }
    if recording_out.is_some()
        && recording_log.is_none()
        && recording_acmi.is_none()
        && convert_capture.is_none()
    {
        return Err(
            "--out goes with --recording-log, --recording-acmi or --convert-capture".into(),
        );
    }
    if (recording_from.is_some() || recording_to.is_some() || recording_ids.is_some())
        && recording_log.is_none()
    {
        return Err("--from, --to and --ids go with --recording-log".into());
    }
    if recording_rate.is_some() && recording_log.is_none() && recording_acmi.is_none() {
        return Err("--rate goes with --recording-log or --recording-acmi".into());
    }
    if recording_guns && recording_acmi.is_none() {
        return Err("--guns goes with --recording-acmi".into());
    }
    if record_mission.is_some() && (ai_probe.is_none() || ai_roster_probe) {
        return Err("--record-mission records an --ai-probe-ticks run".into());
    }
    if verify_render && record_mission.is_none() && probe_script.matrix.is_none() {
        return Err("--verify-render checks a --record-mission run".into());
    }
    if let Some(path) = recording_info {
        replay::cli::info(&path, &mut std::io::stdout().lock())?;
        return Ok(Outcome::Done);
    }
    if let Some(path) = recording_log {
        let folder = replay::cli::log(
            &path,
            &replay::cli::LogOptions {
                out: recording_out,
                from_s: recording_from,
                to_s: recording_to,
                ids: recording_ids,
                rate: recording_rate,
            },
        )?;
        if std::env::var("TORE_WEAPON_SOUND_LOG").is_ok_and(|v| v == "1") {
            replay::sound::audit(&path, &folder)?;
        }
        println!("Recording log: {}", folder.join("log.jsonl").display());
        println!(
            "Recording summary: {}",
            folder.join("summary.txt").display()
        );
        return Ok(Outcome::Done);
    }
    if let Some(path) = recording_acmi {
        let written = replay::cli::acmi(&path, recording_out, recording_rate, recording_guns)?;
        println!("Tacview file: {}", written.display());
        return Ok(Outcome::Done);
    }
    if let Some((a, b)) = recording_diff {
        replay::cli::diff(&a, &b, &mut std::io::stdout().lock())?;
        return Ok(Outcome::Done);
    }
    if let Some(path) = convert_capture {
        replay::net_convert::command(&path, recording_out.as_deref())?;
        return Ok(Outcome::Done);
    }
    let probe_record = record_mission.map(|path| ProbeRecord {
        path,
        verify: verify_render,
    });
    if let Some(seconds) = input_seconds {
        input::diagnostics(
            seconds,
            write_input_profile.as_deref(),
            test_rumble.as_deref(),
            native_input,
        )?;
        return Ok(Outcome::Done);
    }
    if record_input.is_some()
        && (headless_ticks.is_some()
            || import_only
            || snapshot.is_some()
            || initial_screen != Screen::Flight
            || capture_terrain.is_some()
            || flight_probe_ticks.is_some()
            || flight_devices.is_some()
            || flight_controls.is_some()
            || (flight_throttle.is_some() || flight_bay.is_some()))
    {
        return Err("--record-input requires direct --free-flight without headless/capture/probe/pose overrides".into());
    }
    let replay_frames = if let Some(path) = replay_input {
        let frames =
            tore_input::recording::read(std::io::BufReader::new(std::fs::File::open(path)?))?;
        if frames.is_empty()
            || headless_ticks.is_some()
            || record_input.is_some()
            || !matches!(maneuver.as_str(), "level" | "hover")
        {
            return Err("--replay-input requires a nonempty tape, the level or hover maneuver, and no --headless-flight/--record-input".into());
        }
        headless_ticks = Some(frames.len());
        Some(frames)
    } else {
        None
    };
    diagnostics::stage_done();
    diagnostics::stage("data directory and preferences");
    let data = assets::data_directory()?;
    log::info!("Data directory: {}", data.display());
    if ground_start_airport.is_some() {
        let ground_capture_probe = flight_probe_ticks.is_some()
            && capture_terrain.is_some()
            && initial_screen == Screen::Flight
            && matches!(maneuver.as_str(), "level" | "takeoff" | "nosewheel");
        if !researched_flight || native_tables.is_some() {
            return Err("Ground start requires the researched flight model; choose Airborne for this adapter.".into());
        }
        if airport_probe.is_some()
            || flight_devices.is_some()
            || flight_controls.is_some()
            || damage_preview.is_some()
            || (flight_probe_ticks.is_some() && !ground_capture_probe)
            || flight_throttle.is_some()
            || flight_bay.is_some()
        {
            return Err("ground start cannot combine with other pose overrides".into());
        }
        if initial_screen == Screen::Viewer {
            return Err("ground start is for flight or the Quick Mission creator".into());
        }
        if initial_screen == Screen::Main {
            initial_screen = if snapshot.is_some() {
                Screen::Quick
            } else {
                Screen::Flight
            };
        }
    }
    // A window is opened only when no headless mode was selected. The locate
    // screen belongs to those runs alone; everything else keeps the terminal
    // behaviour, which package C settled.
    let windowed = !import_only
        && snapshot.is_none()
        && replay_combat.is_none()
        && panel_snapshot.is_none()
        && hud_snapshot.is_none()
        && headless_ticks.is_none()
        && ai_probe.is_none()
        && !sensor_summary
        && !missile_acceptance
        && !combat_smoke
        && gunsight_probe.is_none()
        && !native_flight_report
        && !validate_creator
        && !validate_tanks
        && wing_loadout_probe.is_none()
        && !validate_ordnance
        && animation_probe.is_none()
        && !validate_weather
        && !validate_maps
        && !validate_ils
        && !airport_allegiance
        && !validate_text
        && !(airport_probe.is_some() && !(smoke_test && initial_screen == Screen::Flight))
        && std::env::var_os("TORE_ENVIRONMENT_PROBE").is_none();
    // Borderless fullscreen on the monitor the window would have opened on is
    // the default for an interactive start, and the locate shell and the game
    // window share the choice for the rest of the session.
    let preference = saved_fullscreen();
    // Before anything can open a window, because the first-run screen and the
    // game must use the same API. Runs that ignore saved choices elsewhere
    // (`preferences_enabled`, below) ignore this one too.
    let saved_choices = !smoke_test
        && capture_terrain.is_none()
        && std::env::var_os("TORE_PERF_FRAMES").is_none()
        && std::env::var_os("TORE_PERF_TICKS").is_none();
    graphics_api::select(
        &data,
        graphics::GraphicsApi::flag(&graphics_flags)?,
        if saved_choices {
            graphics::Options::load(&data.join("graphics-v1.conf")).graphics_api
        } else {
            graphics::GraphicsApi::Auto
        },
        saved_choices,
    );
    let mut window = WindowState {
        fullscreen: initial_window_mode(
            windowed_flag,
            window_size_flag,
            smoke_test
                || capture_terrain.is_some()
                || snapshot.is_some()
                || panel_snapshot.is_some()
                || hud_snapshot.is_some(),
            preference,
        )
        .fullscreen(),
        preference,
    };
    let reimport_background = match session {
        Session::Reimport(frame) => Some(frame),
        Session::First => None,
    };
    let reimporting = reimport_background.is_some();
    diagnostics::stage_done();
    diagnostics::stage("asset loading or import");
    let mut assets = if let Some(chosen) = import.filter(|_| !reimporting) {
        // --import accepts an installed folder, a mounted disc folder or the
        // installer container inside one; the kind is decided by content.
        Assets::import_path(&chosen, &data)?
    } else {
        // Pref asked for this screen, so the pack on disk is deliberately ignored.
        let loaded = match reimporting {
            true => Err("Re-import media".into()),
            false => Assets::load(&data),
        };
        match loaded {
            Ok(assets) => assets,
            Err(error) => {
                log::warn!("Imported assets unavailable: {error}");
                diagnostics::stage("media source discovery");
                // A remembered source is tried first, then a developer checkout.
                let known = media_source::remembered(&data)
                    .map(|(path, _)| path)
                    .into_iter()
                    .chain(std::iter::once(PathBuf::from(
                        "gameassets/fighters-anthology",
                    )))
                    .find_map(|path| media_source::MediaSource::detect(&path).ok())
                    .map(|source| source.path);
                let candidates = if windowed {
                    media_source::candidates(Duration::from_secs(2))
                } else {
                    Vec::new()
                };
                diagnostics::stage_done();
                let first = candidates.first().map(|source| source.path.clone());
                let prefill = known.clone().or_else(|| first.clone());
                let step = next_step(false, windowed, known, first);
                diagnostics::stage("media availability and import");
                match step {
                    Step::Fail => {
                        return Err(format!(
                            "{error}\nImport your own Fighters Anthology media with --import <directory>: an installed Fighters Anthology folder, or the folder of a mounted disc 1 holding SETUP.ESA."
                        )
                        .into());
                    }
                    // No window: the terminal path package C left in place.
                    Step::ImportNow(path) if !windowed => Assets::import_path(&path, &data)?,
                    Step::Play => Assets::load(&data)?,
                    step => {
                        // Re-import is a deliberate choice, so that screen waits
                        // for the player even when a source is already known.
                        let auto = if reimporting && !smoke_test {
                            None
                        } else {
                            auto_import_path(&step, prefill.as_deref(), smoke_test)
                        };
                        let outcome = locate_shell(
                            shared_event_loop(event_loop)?,
                            &data,
                            prefill,
                            candidates,
                            reimport_background,
                            AutoImport {
                                path: auto,
                                continue_when_done: smoke_test,
                            },
                            &mut window,
                        )?;
                        if outcome == ShellOutcome::Quit {
                            return Ok(Outcome::Done);
                        }
                        diagnostics::stage("loading newly imported assets");
                        Assets::load(&data)?
                    }
                }
            }
        }
    };
    diagnostics::stage_done();
    if import_only {
        return Ok(Outcome::Done);
    }
    if let Some(out) = animation_probe {
        aircraft_animation_probe::run(&assets.theater_resources, aircraft_id, &out)?;
        return Ok(Outcome::Done);
    }
    if let Some(out) = wing_loadout_probe {
        wing_loadout_probe::run(&assets, &out)?;
        return Ok(Outcome::Done);
    }
    if validate_ordnance {
        ordnance_audit::validate(&assets.theater_resources)?;
        return Ok(Outcome::Done);
    }
    if validate_tanks {
        ordnance::validate_tanks(
            &assets.theater_resources,
            aircraft_requested.then_some(aircraft_id),
        )?;
        return Ok(Outcome::Done);
    }
    if validate_text {
        // Every imported string must decode without loss and be drawable in the
        // original fonts. Retail text is CP437 (tore_formats::text); the only
        // byte above 0x7F in any text is the Kurile airport's e with a diaeresis.
        let mut problems: Vec<String> = Vec::new();
        let mut accented = Vec::new();
        for (name, bytes) in &assets.theater_resources {
            if !(bytes.starts_with(b"textFormat")
                || name.ends_with(".MT")
                || name.ends_with(".BRF"))
            {
                continue;
            }
            for line in bytes.split(|b| *b == b'\n' || *b == 0) {
                if line.iter().any(|b| *b >= 0x80) {
                    let text = tore_formats::text::decode_cp437(line);
                    accented.push(format!("{name}: {}", text.trim()));
                    if let Some(c) = text.chars().find(|c| {
                        *c != '\r' && !tore_formats::text::is_drawn(*c) && !c.is_control()
                    }) {
                        problems.push(format!(
                            "{name}: {c:?} cannot be drawn in the original fonts: {text}"
                        ));
                    }
                }
            }
        }
        let quick = quick_mission::QuickMission::new(
            aircraft_id,
            assets.creator_options.clone(),
            &assets.theater_resources,
        );
        let mut strings = quick.imported_strings();
        for (name, bytes) in &assets.theater_resources {
            if name.ends_with(".JT")
                && let Ok(weapon) = tore_formats::weapons::Weapon::parse(name, bytes)
            {
                strings.push((format!("weapon {name}"), weapon.name));
                strings.push((format!("weapon {name} HUD name"), weapon.hud_name));
            }
            if name.ends_with(".PT")
                && let Ok(aircraft) = tore_formats::aircraft::Aircraft::parse(bytes)
            {
                strings.push((format!("aircraft {name}"), aircraft.name));
            }
        }
        for (what, text) in &strings {
            if text.contains('\u{fffd}') {
                problems.push(format!("{what}: replacement character in {text:?}"));
            }
            if let Some(c) = text.chars().find(|c| !tore_formats::text::is_drawn(*c)) {
                problems.push(format!("{what}: {c:?} cannot be drawn in {text:?}"));
            }
        }
        for line in &accented {
            println!("non-ASCII text: {line}");
        }
        println!(
            "Scanned {} imported strings, {} lines with non-ASCII bytes, {} problems",
            strings.len(),
            accented.len(),
            problems.len()
        );
        for problem in &problems {
            println!("  PROBLEM {problem}");
        }
        if !problems.is_empty() {
            return Err("imported text problems".into());
        }
        return Ok(Outcome::Done);
    }
    if airport_allegiance {
        // Every base theater's airports by side, for Blue and for Redfor
        // (slice AL1; docs/testing/lane-menus.md).
        quick_mission::allegiance::run(
            &assets.theater_resources,
            assets.creator_options.clone(),
            aircraft_id,
        )?;
        return Ok(Outcome::Done);
    }
    if validate_ils {
        // Every base theater, and the developer variant named by --theater.
        let mut codes: Vec<String> = tore_formats::theater::map_catalog(&assets.theater_resources)?
            .into_iter()
            .map(|(code, _)| code)
            .filter(|code| !code.starts_with('~'))
            .collect();
        if theater_code.starts_with('~') {
            codes.push(theater_code.clone());
        }
        let clearance = flight::State::new(
            &aircraft::Airframe::load(&assets.theater_resources, aircraft_id)?.profile,
            [0.; 3],
        )
        .map(|state| state.model().configuration().equipment.ground_clearance_ft)
        .unwrap_or(0.);
        ils_survey::run(&assets.theater_resources, &codes, clearance, 5.)?;
        return Ok(Outcome::Done);
    }
    if validate_maps {
        let catalog = tore_formats::theater::map_catalog(&assets.theater_resources)?;
        for (code, _) in &catalog {
            let world = scenery::launch_terrain(&assets.theater_resources, code, None)?;
            // The layout's own art, without the flight vegetation: baking
            // vegetation for 75 layouts took 5 to 15 s each on a cold cache
            // and pushed this past the battery's 10 minutes. The vegetation
            // pages are checked against the device limits, through the real
            // mission loader, by `--vegetation-preview DIR limits-check`.
            let scenery = scenery::Scenery::build_for(&assets.theater_resources, &world, false)?;
            let bytes = scenery.texture_indices.len() + scenery.sky_indices.len();
            if bytes / 65536 > 4096 {
                return Err("map artwork exceeds GPU page budget".into());
            }
            println!(
                "map {code}: source={} grid={}x{} textures={} placements={} bodies={} terrain_vertices={} scenery_vertices={} artwork_bytes={}",
                world.environment.map,
                world.theater.cols,
                world.theater.rows,
                world.environment.textures.len(),
                world.static_manifest.len(),
                world.airport_scene.objects.len(),
                scenery.vertices.len() / 10,
                scenery.static_vertex_count(),
                bytes
            );
        }
        println!("Validated {} retail map layouts", catalog.len());
        return Ok(Outcome::Done);
    }
    diagnostics::stage("aircraft loading");
    let hornet = aircraft::Airframe::load(&assets.theater_resources, aircraft_id)?;
    diagnostics::stage_done();
    if let Some(path) = replay_combat {
        if record_combat.is_some() {
            return Err("combat record and replay are mutually exclusive".into());
        }
        let c = combat::Combat::new(&hornet, &assets.theater_resources, true)?;
        let w = scenery::launch_terrain(&assets.theater_resources, &theater_code, None)?;
        tape_file::replay(
            &path,
            &assets.theater_resources,
            c.state.own().configuration().clone(),
            &theater_code,
            &w,
        )?;
        return Ok(Outcome::Done);
    }
    if sensor_summary {
        // The reviewable per-aircraft capability report. Porting an aircraft
        // means reviewing this output, not writing another radar controller.
        for id in tore_formats::aircraft::AircraftId::ALL {
            match aircraft::Airframe::load(&assets.theater_resources, id) {
                Ok(airframe) => println!("{}", airframe.sensors.summary()),
                Err(error) => println!("{id:?}: unavailable, {error}"),
            }
        }
        return Ok(Outcome::Done);
    }
    if missile_sag {
        missile_sag::run(&assets.theater_resources)?;
        return Ok(Outcome::Done);
    }
    if missile_acceptance {
        let config = tore_sim::combat::live::Configuration::from_source(&hornet.profile, |name| {
            assets
                .theater_resources
                .get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other("missing probe resource"))
        })?;
        missile_acceptance::run(config)?;
        return Ok(Outcome::Done);
    }
    if combat_smoke {
        combat_smoke::smoke(&hornet, &assets.theater_resources)?;
        return Ok(Outcome::Done);
    }
    if let Some(name) = &gunsight_probe {
        let config = tore_sim::combat::live::Configuration::from_source(&hornet.profile, |name| {
            assets
                .theater_resources
                .get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other("missing probe resource"))
        })?;
        gunsight_probe::run(
            &config,
            name,
            gunsight_dump.as_deref(),
            &assets.theater_resources,
        )?;
        return Ok(Outcome::Done);
    }
    if live_fire && record_input.is_some() {
        return Err("combat recording is not in the flight-only tape format".into());
    }
    if native_flight_report {
        flight::native_report(&hornet.profile)?;
        if let Some(path) = native_flight_trig {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(643)
                .read_to_end(&mut bytes)?;
            let table = tore_formats::flight_model::rotation::TrigTable::parse(&bytes)?;
            flight::native_rotation_report(&hornet.profile, &table)?;
        }
        return Ok(Outcome::Done);
    }
    let ground_object = |world: &terrain::Terrain| -> AppResult<Option<u32>> {
        ground_start_airport
            .map(|id| {
                world
                    .airport_scene
                    .airport(id)
                    .and_then(|a| a.runway_objects.first())
                    .copied()
                    .ok_or_else(|| "Selected ground-start airport is unavailable".into())
            })
            .transpose()
    };
    let setup_maneuver = |state: &mut flight::State| {
        let mut keys = flight::PilotInput::default();
        match maneuver.as_str() {
            "pull" | "loop" => {
                keys.pitch = 1.;
                if maneuver == "loop" {
                    state.throttle = 1.;
                    state.burner = true;
                }
            }
            "bank-left" | "bank-right" => {
                state.bank = if maneuver == "bank-left" {
                    -45f64
                } else {
                    45f64
                }
                .to_radians();
                state.throttle = 1.;
                state.burner = true;
                keys.pitch = 1.;
            }
            "takeoff" | "takeoff-gear-early" | "takeoff-gear-airborne" => {
                state.brake_out = false;
                state.throttle = 1.;
                state.burner = state
                    .model()
                    .configuration()
                    .propulsion
                    .afterburner_thrust_lbf
                    > 0.;
                keys.pitch = 0.35;
            }
            "roll" => {
                keys.roll = 1.;
            }
            "rudder" => {
                state.yaw = 0.;
                state.velocity = [0., 0., state.speed];
                state.engine = false;
                state.throttle = 0.;
                keys.yaw = 1.;
            }
            "nosewheel" => {
                state.brake_out = false;
                state.throttle = 0.;
                state.speed = 10. * 5280. / 3600.;
                state.velocity = attitude::Basis::new(state.yaw, 0., 0.)
                    .forward
                    .map(|v| v * state.speed);
                keys.yaw = 1.;
            }
            "spin" => {
                state.speed = 180.;
                state.engine = false;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
                keys.pitch = 1.;
                keys.yaw = 1.;
            }
            "eject-low" => state.position[1] = 250.,
            "hover" => {
                // A powered-lift aircraft at rest in the air, in the hover its
                // own rotors, nozzles or nacelles trim to (VTOL overhaul).
                // Hands off it holds; a tape flies the rest.
                state.trim_hover();
            }
            "waypoint" => {
                // Waypoint 1 lies 60,000 feet away, 60 degrees right of north.
                state.autopilot.set_navigation_target(Some(
                    tore_sim::autopilot::NavigationTarget {
                        number: 1,
                        position: [51_961.5, 30_000.],
                    },
                ));
                keys.commands = vec![flight::PilotCommand::Set(
                    flight::Switch::WaypointAutopilot,
                    true,
                )];
            }
            "autopilot" => {
                // A 25 degree bank with the heading and altitude hold engaged: it
                // has to level the wings and hold the 5,000 feet it captured.
                state.bank = 25f64.to_radians();
                keys.commands = vec![flight::PilotCommand::Set(flight::Switch::Autopilot, true)];
            }
            "climb" => {
                state.throttle = 1.;
                state.burner = true;
            }
            "dive" => {
                // A full afterburner dive from 40,000 ft: it should end in the
                // ground impact or time-based overspeed loss.
                state.position[1] = 40_000.;
                state.pitch = -60f64.to_radians();
                state.throttle = 1.;
                state.burner = true;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
            "overspeed" => {
                // Timer fixture at 20,000 ft, continuously held at 1.1 times
                // the current limit below. Not a natural acceleration test.
                state.position[1] = 20_000.;
                if let Some(ratio) = state.overspeed_ratio().filter(|r| *r > 0.) {
                    state.speed *= 1.1 / ratio;
                }
                state.throttle = 1.;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
            "sprint" => {
                // Full afterburner in level flight, altitude held by the autopilot.
                state.throttle = 1.;
                state.burner = true;
                keys.commands = vec![
                    flight::PilotCommand::Set(flight::Switch::Autopilot, true),
                    flight::PilotCommand::Set(flight::Switch::Burner, true),
                    flight::PilotCommand::Throttle(1.),
                ];
            }
            "stall-recover" => {
                state.position[1] = flight_probe::STALL_START_FT;
                state.update_stall_scale();
                // 200 knots, or 15 percent over the weight-scaled clean stall
                // speed at that height if that is higher: a loaded aircraft's
                // stall speed can pass 200 knots up there.
                state.speed = flight_probe::STALL_START_FPS
                    .max(state.clean_stall_speed().unwrap_or(0.) * 1.15);
                state.throttle = 0.3;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
            "spin-recover" => {
                state.speed = 180.;
                state.position[1] = flight_probe::SPIN_START_FT;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
            "stall" => {
                state.engine = false;
                state.pitch = 0.2;
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
            // `settle:SINK_FPS[:GROUND_KNOTS]`: the touchdown rule's probe
            // (docs/spec/touchdown.md). A powered-lift aircraft trims into a
            // hover 60 feet over flat open ground, with that ground speed at
            // the start, and `settle_keys` holds SINK_FPS to the ground;
            // anything else comes in gear down at 140 knots, hands off.
            // Ground is at 0 feet.
            other if other.starts_with("settle:") => {
                let mut parts = other["settle:".len()..].split(':');
                let sink: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(5.);
                let knots: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
                state.gear_down = true;
                state.gear = 1.;
                let clearance = state.model().configuration().equipment.ground_clearance_ft;
                if state.trim_hover() {
                    state.position[1] = clearance + 60.;
                    state.velocity = [0., -sink, knots * 1.68781];
                } else {
                    state.position[1] = clearance + 60.;
                    state.speed = 140. * 1.68781;
                    state.velocity = [0., -sink, state.speed];
                }
                state.speed = state.velocity[2].hypot(state.velocity[1]);
            }
            _ => {}
        }
        keys
    };
    if let Some(ticks) = headless_ticks {
        if ticks > 120 * 3600 {
            return Err("headless flight limited to one hour".into());
        }
        let replay_world = if replay_frames.is_some()
            || ground_start_airport.is_some()
            || flight_start.is_some()
        {
            Some(scenery::launch_terrain(
                &assets.theater_resources,
                &theater_code,
                None,
            )?)
        } else {
            None
        };
        let mut landing_probe: Option<flight_probe::Landing> = None;
        // What the player's ILS reads along the scripted landing.
        let mut ils_probe = ils_survey::PathRecord::default();
        let mut ils_service: Option<tore_sim::airport::Service> = None;
        let mut state = if let Some(world) = &replay_world {
            hornet.start(world)
        } else {
            flight::State::new(&hornet.profile, [0., 5000., 0.])?
        };
        if researched_flight {
            state.enable_research(1)?;
        }
        if let Some(world) = &replay_world {
            let cell = f64::from(tore_formats::theater::CELL_FEET);
            println!(
                "map_extent_ft: x={:.0} z={:.0}",
                (world.theater.cols - 1) as f64 * cell,
                (world.theater.rows - 1) as f64 * cell
            );
            if let Some([x, z, heading, agl]) = flight_start {
                state.position = [x, f64::from(world.height(x as f32, z as f32)) + agl, z];
                state.yaw = heading.to_radians();
                state.velocity = attitude::Basis::new(state.yaw, state.pitch, state.bank)
                    .forward
                    .map(|v| v * state.speed);
            }
        }
        // A powered-lift aircraft starts in trimmed forward flight, as in a
        // mission (VTOL overhaul decision 8); a ground start below replaces it.
        if state.research.is_some() {
            state.start_airborne(replay_world.as_ref().map_or([0.; 3], |w| w.wind()));
        }
        println!(
            "flight_model={}",
            if native_tables.is_some() {
                "native-research-airborne"
            } else if state.research.is_some() {
                "hybrid"
            } else {
                "legacy"
            }
        );
        if let Some(world) = &replay_world
            && let Some(object) = ground_object(world)?
        {
            if replay_frames.is_none() {
                let mut load = build_combat(
                    &hornet,
                    &assets.theater_resources,
                    false,
                    stripped_loadout.as_deref(),
                )?;
                if stripped_loadout.is_some() {
                    // A stripped load only takes effect on a reset, which also
                    // sets the payload and the fuel systems from it.
                    load.reset(&mut state)?;
                } else {
                    state.set_payload(load.state.own().payload_lbs())?;
                    state.systems = tore_sim::aircraft_systems::Systems::new(
                        load.state.own().configuration().engines,
                        load.state.own().external_fuel_lbs(),
                    );
                }
            }
            mission_layout::apply_ground_start(world, &mut state, object)?;
            println!(
                "ground_start={object} position={:?} heading={:.3} gear={} brakes={}",
                state.position,
                state.yaw.to_degrees(),
                state.gear,
                state.brake_out
            );
            {
                use tore_sim::models::FlightModel;
                let mass = state.model().configuration().mass;
                println!(
                    "loadout: external_fuel_lb={:.0} empty_lb={:.0} internal_fuel_lb={:.0} fuel_lb={:.0} carried_lb={:.0} gross_lb={:.0} max_takeoff_lb={:.0}",
                    state.systems.external_lbs(),
                    mass.empty_lbs,
                    mass.internal_fuel_lbs,
                    state.fuel,
                    state.carried_lbs(),
                    mass.empty_lbs + state.fuel + state.carried_lbs(),
                    mass.max_takeoff_lbs
                );
                // The imported 1 G envelope row at the airport and the figures the
                // hybrid model derives its stall speed and lift from.
                state.update_stall_scale();
                let configuration = state.model().configuration();
                let altitude = state.position[1];
                let row = |g: i32| {
                    state
                        .retail_envelopes()
                        .iter()
                        .find(|e| e.g == g)
                        .and_then(|e| e.speeds(altitude))
                        .map_or_else(
                            || "none".to_string(),
                            |(low, high)| format!("{:.1}..{:.1}", low / 1.68781, high / 1.68781),
                        )
                };
                let scale = state.stall_scale();
                println!(
                    "envelope: altitude_ft={altitude:.1} g1_kt={} g2_kt={} g3_kt={} flaps_lift_f8={} loaded_elevator_percent={} loading={:.3} landing_limit_kt={:.1} stall_scale={scale:.3} stall_kt={:.1} min_level_flaps_kt={:.1}",
                    row(1),
                    row(2),
                    row(3),
                    configuration.aerodynamics.flaps_lift_f8,
                    configuration.aerodynamics.loaded_elevator_percent,
                    (state.fuel + state.carried_lbs()) / mass.empty_lbs,
                    f64::from(configuration.native.landing.forward_fps) / 1.68781,
                    state.clean_stall_speed().unwrap_or(0.) * 0.75 / 1.68781,
                    state.minimum_level_speed(altitude, 1.) / 1.68781
                );
            }
            if let Some(variant) = flight_probe::LandingVariant::from_maneuver(&maneuver) {
                let layout = mission_layout::ground_layout(world, object, 1)?;
                let length = world
                    .airport_scene
                    .runway(object)
                    .map_or(layout.runway.length_ft, |runway| runway.length_ft);
                state.update_stall_scale();
                let probe = flight_probe::Landing::new(
                    &state,
                    layout.slots[0],
                    layout.heading,
                    length,
                    |x, z| world.surface(x, z).height,
                    variant,
                );
                probe.place(&mut state, layout.heading);
                landing_probe = Some(probe);
                println!(
                    "landing_start: position={:?} heading={:.1} runway_length_ft={length:.0} anchored={} slot={:?} runway_center={:?} runway_heading={:.1} footprint_half_ft={:?}",
                    state.position,
                    layout.heading.to_degrees(),
                    layout.anchored,
                    layout.slots[0],
                    layout.runway.center,
                    layout.runway.heading.to_degrees(),
                    world
                        .airport_scene
                        .runway(object)
                        .map(|runway| runway.surface.half.map(f64::round))
                );
            }
        }
        for name in &flight_cheats {
            apply_probe_cheat(&mut state.cheats, name);
        }
        if let Some(pounds) = flight_fuel {
            state.fuel = pounds;
        }
        if flight_probe::LandingVariant::from_maneuver(&maneuver).is_some()
            && landing_probe.is_none()
        {
            return Err(
                "--maneuver land needs --ground-start AIRPORT and the researched flight model"
                    .into(),
            );
        }
        let keys = setup_maneuver(&mut state);
        if maneuver == "hover" && state.model().powered_lift().is_none() {
            return Err(
                "--maneuver hover needs a powered-lift aircraft (av8, yak141, v22, ah64, mi24 or ch47)"
                    .into(),
            );
        }
        if maneuver == "gcurve" {
            // The loaded positive G limit at each speed at 5,000 feet and the
            // aircraft's own weight: what a full back stick can pull.
            state.position[1] = 5_000.;
            state.update_stall_scale();
            let raw_top = state
                .retail_envelopes()
                .iter()
                .map(|e| e.g)
                .max()
                .unwrap_or(1);
            let corner = state
                .retail_envelopes()
                .iter()
                .find(|e| e.g == raw_top)
                .and_then(|e| e.speeds(5_000.))
                .map_or(0., |(low, _)| low / 1.68781);
            println!(
                "gcurve_header: stall_scale={:.3} top_g={raw_top} corner_kt={corner:.1}",
                state.stall_scale()
            );
            for kt in (100..=750).step_by(25) {
                let mut probe = state.clone();
                probe.speed = f64::from(kt) * 1.68781;
                probe.velocity = [0., 0., probe.speed];
                probe.yaw = 0.;
                probe.pitch = 0.;
                probe.bank = 0.;
                probe.throttle = 1.;
                let pull = flight::PilotInput {
                    pitch: 1.,
                    ..Default::default()
                };
                probe.step(&pull, |_, _| 0.);
                let limit = probe
                    .trace()
                    .adapter
                    .as_ref()
                    .map_or(0., |a| a.envelope.limits_g[1]);
                println!("gcurve: kt={kt} limit_g={limit:.2}");
            }
            let mut probe = state.clone();
            probe.speed = corner * 1.68781;
            probe.velocity = [0., 0., probe.speed];
            probe.step(
                &flight::PilotInput {
                    pitch: 1.,
                    ..Default::default()
                },
                |_, _| 0.,
            );
            println!(
                "gcurve_corner: limit_g={:.2}",
                probe
                    .trace()
                    .adapter
                    .as_ref()
                    .map_or(0., |a| a.envelope.limits_g[1])
            );
            return Ok(Outcome::Done);
        }
        if let Some(tables) = &native_tables {
            state.enable_native(tables.clone(), 1)?;
        }
        let initial_forward = attitude::Basis::new(state.yaw, state.pitch, state.bank).forward;
        let (mut vertical, mut inverted, mut completed) = (false, false, false);
        let mut watch = flight_watch::FlightWatch::new(flight_trace_ticks, &state);
        let mut spin_recovery =
            (maneuver == "spin-recover").then(|| flight_probe::SpinRecovery::new(&state));
        let mut climb = (maneuver == "climb").then(|| flight_probe::Climb::new(&state));
        let mut devices = (maneuver == "devices").then(|| flight_watch::DeviceWatch::new(&state));
        let mut stall_recovery =
            (maneuver == "stall-recover").then(|| flight_probe::StallRecovery::new(&state));
        let mut gear_pulled = false;
        // `--maneuver settle:SINK[:KNOTS]` holds that sink rate to the ground.
        let settle_sink: Option<f64> = maneuver.strip_prefix("settle:").map(|rest| {
            rest.split(':')
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5.)
        });
        let mut takeoff_start: Option<attitude::Vector> = None;
        let mut rotation: Option<f64> = None;
        let mut liftoff: Option<(u64, f64, f64)> = None;
        for tick in 0..ticks {
            if maneuver == "overspeed"
                && !state.crashed
                && let Some(ratio) = state.overspeed_ratio().filter(|r| *r > 0.)
            {
                let scale = 1.1 / ratio;
                state.speed *= scale;
                state.velocity = state.velocity.map(|v| v * scale);
            }
            let scripted;
            let keys = if let Some(probe) = spin_recovery.as_mut() {
                scripted = probe.keys(&state);
                &scripted
            } else if matches!(maneuver.as_str(), "eject" | "eject-low")
                && (tick == 120 || tick == 140)
            {
                // Two presses inside the confirmation interval, as Shift-E twice.
                scripted = flight::PilotInput {
                    commands: vec![flight::PilotCommand::Eject],
                    ..Default::default()
                };
                &scripted
            } else if maneuver == "devices"
                && let Some(commands) = device_schedule(tick as u64)
            {
                scripted = flight::PilotInput {
                    commands,
                    ..Default::default()
                };
                &scripted
            } else if let Some(probe) = climb.as_mut() {
                scripted = probe.keys(&state);
                &scripted
            } else if let Some(probe) = stall_recovery.as_mut() {
                scripted = probe.keys(&state);
                &scripted
            } else if let Some(probe) = landing_probe.as_mut() {
                let world = replay_world.as_ref().unwrap();
                let surface = world.surface(state.position[0], state.position[2]);
                scripted = probe.keys(&state, surface.height, surface.landable);
                &scripted
            } else if let Some(sink) = settle_sink
                && state.model().powered_lift().is_some()
            {
                scripted = settle_keys(&state, sink);
                &scripted
            } else if let Some(pull) =
                gear_pull(&maneuver, &state, &mut gear_pulled, &keys, &replay_world)
            {
                scripted = pull;
                &scripted
            } else {
                replay_frames.as_ref().map_or(&keys, |frames| &frames[tick])
            };
            for (_, index) in flight_faults.iter().filter(|(at, _)| *at == tick as u64) {
                state.systems.hit(*index, state.throttle);
            }
            if ground_start_airport.is_some() {
                let world = replay_world.as_ref().unwrap();
                state.step_surface(keys, |x, z| world.surface(x, z));
            } else {
                state.step(keys, |x, z| {
                    replay_world
                        .as_ref()
                        .map_or(0., |world| f64::from(world.height(x as f32, z as f32)))
                });
            }
            if let Some(error) = state.native_fault() {
                return Err(error.into());
            }
            if takeoff_start.is_none() {
                takeoff_start = Some(state.position);
            }
            let on_runway = state.research.as_ref().is_some_and(|r| r.on_ground);
            if rotation.is_none() && on_runway && state.pitch > 1.5f64.to_radians() {
                rotation = Some(state.speed / 1.68781);
            }
            if liftoff.is_none() && !on_runway && state.ticks > 1 {
                let start = takeoff_start.unwrap_or(state.position);
                liftoff = Some((
                    state.ticks,
                    state.speed / 1.68781,
                    (state.position[0] - start[0]).hypot(state.position[2] - start[2]),
                ));
            }
            if let Some(world) = &replay_world {
                apply_edge_loss(&mut state, world);
            }
            if landing_probe.is_some()
                && let Some(world) = &replay_world
            {
                let service = match &mut ils_service {
                    Some(service) => service,
                    None => {
                        let mut service = tore_sim::airport::Service::new(&world.airport_scene)
                            .map_err(std::io::Error::other)?;
                        // The pilot selects the airport he is landing at.
                        if let Some(object) = ground_object(world)?
                            && let Some(runway) = world.airport_scene.runway(object)
                        {
                            service.command(
                                &world.airport_scene,
                                airport_aircraft(world, &state, true, false),
                                tore_sim::airport::Command::SelectAirport(runway.airport),
                            );
                        }
                        ils_service.insert(service)
                    }
                };
                let ground = world.surface(state.position[0], state.position[2]).height;
                let aircraft = airport_aircraft(world, &state, true, false);
                ils_probe.observe(
                    service.guidance(&world.airport_scene, aircraft),
                    state.position[1] - aircraft.ground_clearance_ft - ground,
                );
            }
            watch.observe(&state);
            if let Some(world) = &replay_world {
                let ground = world.surface(state.position[0], state.position[2]).height;
                watch.observe_ground(&state, ground);
            }
            if let Some(devices) = devices.as_mut() {
                devices.observe(&state);
            }
            let basis = attitude::Basis::new(state.yaw, state.pitch, state.bank);
            vertical |= basis.forward[1] > 0.999;
            inverted |= basis.up[1] < -0.9;
            completed |= inverted
                && basis.up[1] > 0.9
                && attitude::dot(basis.forward, initial_forward) > 0.98;
            if maneuver.starts_with("takeoff") && ground_start_airport.is_some() {
                let world = replay_world.as_ref().unwrap();
                let object = ground_object(world)?.unwrap();
                let height = world.airport_scene.runway(object).unwrap().elevation_ft;
                if !state.crashed && state.position[1] > height + 100. {
                    println!("takeoff_complete=true airport_ground_ft={height}");
                    break;
                }
            }
            if maneuver == "loop" && completed {
                break;
            }
            if spin_recovery.as_ref().is_some_and(|probe| probe.finished())
                || stall_recovery
                    .as_ref()
                    .is_some_and(|probe| probe.finished())
                || climb.as_ref().is_some_and(|probe| probe.finished(&state))
                || landing_probe.as_ref().is_some_and(|probe| probe.finished())
            {
                break;
            }
        }
        let body = attitude::Basis::new(state.yaw, state.pitch, state.bank);
        let forward = attitude::dot(state.velocity, body.forward);
        let side = attitude::dot(state.velocity, body.right);
        let up = attitude::dot(state.velocity, body.up);
        println!(
            "aoa_deg={:.4} sideslip_deg={:.4} bank_deg={:.4}",
            (-up).atan2(forward).to_degrees(),
            side.atan2(forward.hypot(up)).to_degrees(),
            state.bank.to_degrees()
        );
        println!("vertical={vertical} inverted={inverted} loop_completed={completed}");
        if maneuver.starts_with("takeoff") {
            match liftoff {
                Some((tick, speed, distance)) => println!(
                    "liftoff: tick={tick} speed_kt={speed:.1} distance_ft={distance:.0} rotation_kt={}",
                    rotation.map_or("none".into(), |r| format!("{r:.1}"))
                ),
                None => println!("liftoff: none"),
            }
        }
        if maneuver.starts_with("takeoff-gear") {
            let refusals = state
                .systems
                .messages
                .iter()
                .filter(|m| m.as_str() == flight::GROUND_SENSOR_MESSAGE)
                .count();
            println!(
                "gear_pulled={gear_pulled} ground_sensor_refusals={refusals} gear={:.2}",
                state.gear
            );
        }
        println!(
            "departure_alert={:?} spin_direction={}",
            state.stall_alert(0.),
            state.research.as_ref().map_or(0, |r| r.spinning)
        );

        println!(
            "ticks={} speed_kt={:.3} altitude_ft={:.3} fuel_lb={:.3} crashed={}",
            state.ticks,
            state.speed / 1.68781,
            state.position[1],
            state.fuel,
            state.crashed
        );
        println!(
            "final_position: x={:.0} z={:.0} heading_deg={:.1}",
            state.position[0],
            state.position[2],
            state.yaw.to_degrees().rem_euclid(360.)
        );
        println!("overspeed_ticks={}", state.overspeed_ticks);
        if maneuver.starts_with("settle:") {
            println!(
                "touchdown: on_ground={} gear_damaged={} crashed={}",
                state.research.as_ref().is_some_and(|r| r.on_ground),
                state.systems.has(16),
                state.crashed
            );
        }
        if matches!(maneuver.as_str(), "rudder" | "nosewheel") {
            println!(
                "lateral: yaw_deg={:.3} bank_deg={:.3} velocity_x_fps={:.3} path_deg={:.3} wheel_deg={:.3}",
                state.yaw.to_degrees(),
                state.bank.to_degrees(),
                state.velocity[0],
                state.velocity[0].atan2(state.velocity[2]).to_degrees(),
                state.nosewheel_angle().to_degrees()
            );
        }
        println!("{}", watch.report());
        println!(
            "loss: cause={}",
            state
                .systems
                .structure
                .cause
                .map_or("none", |cause| cause.label())
        );
        println!(
            "fuel_end: internal_lb={:.1} external_lb={:.1}",
            state.fuel,
            state.systems.external_lbs()
        );
        if !flight_faults.is_empty() {
            println!(
                "systems: fatal={} pilot_dead={} engine={} {}",
                state.systems.fatal(),
                state.systems.pilot.dead,
                state.engine,
                state.systems.summary(state.damage_fraction)
            );
        }
        if let Some(probe) = &spin_recovery {
            println!("{}", probe.report(&state));
        }
        if let Some(probe) = &climb {
            println!("{}", probe.report(&state));
        }
        if let Some(devices) = &devices {
            println!("{}", devices.report(&state));
        }
        if let Some(probe) = &stall_recovery {
            println!("{}", probe.report(&state));
        }
        if let Some(probe) = &landing_probe {
            println!("{}", probe.report(&state));
            println!("{}", ils_probe.report());
        }
        if let Some(pilot) = &state.escape {
            println!(
                "ejection={:?} pilot_alive={} pilot_position={:?}",
                pilot.phase, !state.systems.pilot.dead, pilot.position
            );
            // A two-seater's second crew member comes down on his own chute.
            if let Some(crew) = &state.crew_escape {
                println!(
                    "crew_ejection={:?} crew_position={:?}",
                    crew.phase, crew.position
                );
            }
            if state.model().configuration().multi_crew {
                println!("two_seater=true");
            }
        }
        return Ok(Outcome::Done);
    }
    if let Some(path) = hud_snapshot {
        write_hud_snapshot(&hornet, &hud_snapshot_state, &path)?;
        return Ok(Outcome::Done);
    }
    if let Some(path) = panel_snapshot {
        use std::io::Write;
        let mut state = flight::State::new(&hornet.profile, [0., 5000., 0.])?;
        let mut combat = combat::Combat::new(&hornet, &assets.theater_resources, false)?;
        combat.reset(&mut state)?;
        if let Some(throttle) = flight_throttle {
            state.throttle = throttle;
        }
        for index in &systems_preview {
            state.systems.hit(*index, state.throttle);
        }
        if !systems_preview.is_empty() {
            for _ in 0..flight_probe_ticks.unwrap_or(1200) {
                state
                    .systems
                    .advance(true, state.throttle, 1., 0., false, &mut state.fuel);
                state.ticks += 1;
            }
            println!("{}", state.systems.summary(0.));
        }
        let mut panels = instruments::Instruments::new(instrument_layout, None);
        panels.palette = hornet.daylight_palette();
        let target_cam_preview = target_cam_preview.or_else(|| {
            window_layout_preview
                .is_some()
                .then(|| "seeker".to_string())
        });
        if let Some(mode) = &target_cam_preview {
            // The AC-130 gunsight page from a synthetic readout on a synthetic
            // scene, drawn by the same CPU raster as the flight screen.
            if let Some(preview) = instruments::targeting::preview(mode) {
                // The targeting page of a laser or FLIR aircraft, or its buddy form.
                panels.camera_target = preview.target.as_ref().map(|(target, _)| target.id);
                panels.cameras.insert(4, preview.scene);
                let (target, link) = preview.target.unzip();
                panels.combat = Some(instruments::CombatReadout {
                    targeting: Some(preview.page),
                    target,
                    target_link: link.unwrap_or_default(),
                    ..Default::default()
                });
            } else {
                let preview = instruments::gunsight::preview(mode)
                    .ok_or("unknown --target-cam-preview mode")?;
                panels.camera_target = preview.target.as_ref().map(|(target, _)| target.id);
                panels.cameras.insert(4, preview.scene);
                panels.sight_bloom = preview.bloom;
                let (target, link) = preview.target.unzip();
                panels.combat = Some(instruments::CombatReadout {
                    gunsight: Some(preview.page),
                    target,
                    target_link: link.unwrap_or_default(),
                    ..Default::default()
                });
            }
        }
        if let Some(mode) = &window_layout_preview {
            return write_window_layout_preview(&path, mode, panels, &hornet, &state, window_size);
        }
        let r = panels.page(
            instrument_page.unwrap_or(if target_cam_preview.is_some() { 4 } else { 7 }),
            &hornet,
            &state,
        );
        let mut f = std::fs::File::create(path)?;
        write!(
            f,
            "P6\n{} {}\n255\n",
            instruments::WIDTH,
            instruments::HEIGHT
        )?;
        for p in r.pixels.chunks_exact(4) {
            f.write_all(&p[..3])?;
        }
        return Ok(Outcome::Done);
    }
    diagnostics::stage("audio initialization");
    let audio = if no_audio
        || smoke_test
        || validate_creator
        || validate_weather
        || snapshot.is_some()
        || std::env::var_os("TORE_ENVIRONMENT_PROBE").is_some()
    {
        None
    } else {
        match audio::Audio::new(
            std::mem::take(&mut assets.sounds),
            &assets.music_scores,
            &assets.theater_resources,
        ) {
            Ok(audio) => Some(audio),
            Err(error) => {
                log::warn!("Continuing without audio: {error}");
                None
            }
        }
    };
    diagnostics::stage_done();
    // Saved previews stay reproducible; normal launches randomly select all five.
    if snapshot.is_some() && background.is_none() {
        background = Some("CHOOSEV".into());
    }
    diagnostics::stage("terrain construction");
    let mut world =
        scenery::launch_terrain(&assets.theater_resources, &theater_code, weather_condition)?;
    let mut scenery = scenery::Scenery::build_for(
        &assets.theater_resources,
        &world,
        !launch_creator && matches!(initial_screen, Screen::Flight | Screen::Viewer),
    )?;
    diagnostics::stage_done();
    let ground_start = ground_object(&world)?;
    if std::env::var_os("TORE_AIRPORT_PROBE").is_some() {
        println!(
            "airport scene: theater={} airports={} runways={} objects={} targets={}",
            theater_code,
            world.airport_scene.airports.len(),
            world.airport_scene.runways.len(),
            world.airport_scene.objects.len(),
            world.airport_scene.objects.len()
        );
        for runway in &world.airport_scene.runways {
            let allegiance = world
                .airport_scene
                .airports
                .iter()
                .find(|a| a.id == runway.airport)
                .map(|a| a.allegiance);
            println!(
                "airport runway: airport={} name={:?} length_ft={:.0} short_strip={} allegiance={:?}",
                runway.airport,
                runway.name,
                runway.length_ft,
                runway.short_strip(),
                allegiance
            );
        }
    }
    if validate_creator {
        // TORE_CREATOR_STAGE=loadouts|matrix|render (render includes the input fuzz) runs one part of the probe.
        let stage = std::env::var("TORE_CREATOR_STAGE").unwrap_or_default();
        let wanted = |name: &str| stage.is_empty() || stage == name;
        if wanted("loadouts") {
            ordnance::validate_sources(&assets.theater_resources, &world, &scenery)?;
        }
        if wanted("matrix") {
            quick_mission::matrix::validate(
                &assets.theater_resources,
                assets.creator_options.clone(),
            )?;
        }
        if wanted("menu") {
            quick_mission::matrix::flight_menu_table(&assets.theater_resources)?;
        }
        if wanted("render") {
            let resources = assets.theater_resources.clone();
            let options = assets.creator_options.clone();
            let mut menu = Menu::new(assets, Some("CHOOSEV"))?;
            quick_mission::matrix::render(
                &resources,
                options.clone(),
                &menu.quick_sprites,
                &world,
            )?;
            quick_mission::matrix::fuzz(&resources, options, &menu.quick_sprites, &world)?;
            quick_mission::matrix::fuzz_screens(&resources, &mut menu)?;
        }
        return Ok(Outcome::Done);
    }
    if validate_weather {
        weather::validate_sources(&assets.theater_resources, &world.environment)?;
        return Ok(Outcome::Done);
    }
    let theater_resources = assets.theater_resources.clone();
    net::chat::load_quick_messages(&assets.multiplayer_resources);
    net::session::remember_source(&assets.multiplayer_resources);
    let creator_options = assets.creator_options.clone();
    if let Some((airport_id, mut aircraft, angles)) = airport_probe
        && !(smoke_test && initial_screen == Screen::Flight)
    {
        let mut combat = combat::Combat::new(&hornet, &theater_resources, false)?;
        combat.add_airport_targets(&world.airport_scene)?;
        let mut flight = hornet.start(&world);
        flight.position = aircraft.position;
        flight.gear_down = aircraft.gear_down;
        flight.gear = f64::from(aircraft.gear_down);
        if let Some([heading, pitch]) = angles {
            flight.yaw = heading.to_radians();
            flight.pitch = pitch.to_radians();
            flight.bank = 0.;
        }
        aircraft.forward = attitude::Basis::new(flight.yaw, flight.pitch, flight.bank).forward;
        combat.reset(&mut flight)?;
        let live_targets = combat
            .state
            .targets
            .iter()
            .filter(|target| target.role == tore_sim::combat::missiles::TargetRole::Surface)
            .count();
        let visible_vertices = scenery.visible_static_vertices(&combat.state.targets).len() / 10;
        let mut service =
            tore_sim::airport::Service::new(&world.airport_scene).map_err(std::io::Error::other)?;
        service.command(
            &world.airport_scene,
            aircraft,
            tore_sim::airport::Command::SelectAirport(airport_id),
        );
        let reply = service.command(
            &world.airport_scene,
            aircraft,
            tore_sim::airport::Command::RequestLanding,
        );
        let guidance = service.guidance(&world.airport_scene, aircraft);
        println!(
            "airport probe: theater={theater_code} scene_objects={} live_targets={live_targets} visible_vertices={visible_vertices} forward={:?} heading_deg={:.3} pitch_deg={:.3} selected={:?} clearance={:?} reply={reply:?} guidance={guidance:?}",
            world.airport_scene.objects.len(),
            aircraft.forward,
            flight.yaw.to_degrees(),
            flight.pitch.to_degrees(),
            service.selected(),
            service.clearance(),
        );
        return Ok(Outcome::Done);
    }
    diagnostics::stage("menu construction");
    let mut menu = Menu::new(assets, background.as_deref())?;
    diagnostics::stage_done();
    diagnostics::stage("menu and flight state setup");
    if let Some(path) = snapshot {
        if matches!(initial_screen, Screen::Viewer | Screen::Flight) {
            return Err(
                "Use --capture-terrain for a GPU terrain capture; CPU snapshots support menus only"
                    .into(),
            );
        }
        if initial_screen == Screen::Quick {
            let mut quick = quick_mission::QuickMission::new(
                aircraft_id,
                creator_options.clone(),
                &theater_resources,
            );
            quick.ai_mission = ai_mission;
            quick.choose_theater_code(&theater_code, &theater_resources);
            if matches!(
                snapshot_state.as_str(),
                "ordnance"
                    | "ordnance-tanks"
                    | "ordnance-empty"
                    | "ordnance-drag"
                    | "ordnance-message"
                    | "ordnance-message-long"
                    | "lobby-ordnance"
                    | "lobby-ordnance-refused"
                    | "lobby-ordnance-cheat"
                    | "lobby-ordnance-gaps"
                    | "ordnance-laser"
                    | "lobby-ordnance-laser"
                    | "ordnance-pod"
                    | "ordnance-pod-yellow"
                    | "ordnance-gun-pod"
                    | "ordnance-tip-amraam"
                    | "ordnance-wing-amraam"
                    | "ordnance-authored-tank"
            ) {
                quick.ordnance = Some(ordnance::Ordnance::new(
                    tore_sim::combat::loadout::Loadout::new(&hornet.profile, |n| {
                        theater_resources
                            .get(n)
                            .cloned()
                            .ok_or_else(|| std::io::Error::other("missing loadout resource"))
                    })?,
                    &theater_resources,
                )?);
            }
            // Load Ordnance as a lobby opens it (EF8): Accept and Cancel,
            // the mission's Guns only on the page, and the host's own rule
            // (the words Accept would show) when a missile is loaded.
            if snapshot_state.starts_with("lobby-ordnance")
                && let Some(page) = quick.ordnance.as_mut()
            {
                page.lobby = true;
                page.message = Some(
                    "Guns only: this mission allows the gun alone. Accept sends your loadout to the lobby."
                        .into(),
                );
                if snapshot_state == "lobby-ordnance-refused" {
                    let refused = mission::LoadoutSpec::of(&page.loadout).check_for_plane(
                        &hornet.profile,
                        &theater_resources,
                        true,
                    );
                    page.message = Some(
                        refused
                            .err()
                            .map_or_else(|| "(accepted)".to_owned(), |e| e.to_string()),
                    );
                }
                if snapshot_state == "lobby-ordnance-cheat" {
                    page.message = Some(ordnance::LOBBY_CHEAT_NOTICE.into());
                }
                if snapshot_state == "lobby-ordnance-laser" {
                    // The warning shows when no message is up.
                    page.message = None;
                }
            }
            if let Some(page) = snapshot_state.strip_prefix("debrief") {
                let mut report = debrief::Report::sample();
                if page == "-success" {
                    report.outcome = debrief::Outcome::Success;
                }
                let mut debrief =
                    debrief::Debrief::new(report, &theater_resources, Some("DEBSCV.PIC"))?;
                debrief.page = page
                    .trim_start_matches('-')
                    .parse::<usize>()
                    .map_or(0, |page| page.clamp(1, 5) - 1);
                quick.debrief = Some(debrief);
            }
            // The map's mission: the ground target and both ground starts
            // the command line gives (slice CL-R).
            if let Some(stem) = &ground_target {
                quick.choose_ground_target(stem, target_defenses, target_seed)?;
            } else if target_defenses_given {
                quick.choose_area_defenses(target_defenses, target_seed);
            }
            if let Some(object) = ground_start {
                quick.choose_ground_runway(object)?;
            }
            if let Some(id) = enemy_ground_start_airport {
                let object = world
                    .airport_scene
                    .airport(id)
                    .and_then(|a| a.runway_objects.first())
                    .copied()
                    .ok_or("Selected enemy ground-start airport is unavailable")?;
                quick.choose_enemy_runway(object)?;
            }
            if let Some(nm) = separation_nm {
                quick.draft.values[17] = mission_layout::SEPARATION_NM
                    .iter()
                    .position(|choice| *choice == nm)
                    .unwrap_or(quick.draft.values[17]);
            }
            quick.set_resources(Arc::new(theater_resources.clone()));
            quick.settle_distance();
            quick.render(&mut menu.pixels, &menu.quick_sprites, &world);
            quick.preview_selector(&snapshot_state)?;
            quick.settle_distance();
            quick.render(&mut menu.pixels, &menu.quick_sprites, &world);
            println!("quick-snapshot: {}", quick.placement_snapshot_report());
            if snapshot_state.starts_with("map") {
                if let Some(words) = quick.notice.as_deref().filter(|_| !quick.map_open()) {
                    return Err(format!("the map did not open: {words}").into());
                }
                if let Some(plan) = quick.map_plan() {
                    for marker in &plan.markers {
                        println!(
                            "creator map: {} at x={:.0} z={:.0}",
                            marker.label, marker.position[0], marker.position[2]
                        );
                    }
                    if let Some((center, radius)) = plan.objective {
                        println!(
                            "creator map: objective area x={:.0} z={:.0} radius={radius:.0}",
                            center[0], center[1]
                        );
                    }
                }
            }
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if let Some(scroll) = match snapshot_state.as_str() {
            "graphics" | "graphics-scale" | "graphics-shadows" => Some(0),
            "graphics-middle" => Some(-4),
            "graphics-bottom" => Some(-100),
            _ => None,
        } {
            // No GPU here: a fixed support list stands in for the adapter,
            // with 8x unavailable so the struck-through style is visible, and
            // a 3840x2160 window for the render scale's resolution line. The
            // middle and bottom states scroll the list.
            let mut editor = graphics_screen::Editor::new(
                graphics::Options::default(),
                [true, true, true, false],
                "Main menu",
            );
            editor.set_window(Some([3840, 2160]));
            editor.wheel(scroll);
            if snapshot_state == "graphics-shadows" {
                // Focus Shadow distance, bringing its whole group into view.
                for _ in 0..10 {
                    editor.key("ArrowDown", false);
                }
            }
            if snapshot_state == "graphics-scale" {
                // The render scale row focused, its About note showing the sizes.
                editor.key("ArrowDown", false);
            }
            menu.preview_state("normal")?;
            menu.render();
            editor.draw(&mut menu.pixels, &hornet.font);
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if snapshot_state == "sound" {
            // Non-default levels and the switch at YES, so every knob and
            // the switch's other position can be inspected.
            let mut settings = sound_prefs::Settings::default();
            for (i, slider) in sound_prefs::Slider::ALL.into_iter().enumerate() {
                settings.set(slider, [80, 70, 55, 45, 70, 90, 20, 60, 75][i]);
            }
            settings.swap = true;
            menu.preview_state("normal")?;
            menu.render();
            let mut screen = sound_screen::Screen::new(settings, false);
            screen.animate();
            screen.draw(&mut menu.pixels, &menu.sprites);
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if let Some(tab) = snapshot_state.strip_prefix("controls") {
            // A synthetic Xbox-layout pad with its defaults, so the screen
            // can be inspected without hardware.
            let pad = controls_editor::preview_device();
            let defaults = input::gamepad_defaults(&pad);
            let profile = tore_input::Profile {
                bindings: defaults.bindings,
                modifiers: defaults.modifiers,
                gamepad_defaults: true,
                ..Default::default()
            };
            let mut editor = controls_editor::Editor::new(profile, vec![pad], "Main menu");
            editor.head_status = "Waiting for opentrack on UDP 4242".into();
            editor.preview(tab.trim_start_matches('-'))?;
            menu.preview_state("normal")?;
            menu.render();
            editor.draw(&mut menu.pixels, &hornet.font);
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if snapshot_state.starts_with("respawn-") {
            let spec = tore_world::mission::MissionSpec::new(
                &world.layout,
                tore_formats::aircraft::AircraftId::F18,
            );
            let allowed = tore_formats::aircraft::AircraftId::SELECTABLE
                .into_iter()
                .filter(|id| {
                    tore_world::aircraft_type::AircraftType::load(&theater_resources, *id).is_ok()
                })
                .collect();
            let mut overlay = net::respawn::Menu::new(
                tore_formats::aircraft::AircraftId::F18,
                allowed,
                &theater_resources,
                &spec,
                tore_world::world::revive::RevivalWeapons::Missiles,
                |_| false,
            )?;
            for (index, pixel) in menu.pixels.chunks_exact_mut(4).enumerate() {
                let sky = 55 + (index / 640 / 8) as u8;
                pixel.copy_from_slice(&[sky, sky + 20, sky + 35, 255]);
            }
            if snapshot_state.starts_with("respawn-aircraft") {
                overlay.preview_aircraft();
            } else if snapshot_state.starts_with("respawn-station") {
                // The F/A-18D missile station, after its gun and tank.
                overlay.key("ArrowDown", &theater_resources, &spec);
                overlay.key("ArrowDown", &theater_resources, &spec);
                overlay.key("Enter", &theater_resources, &spec);
            }
            if snapshot_state.ends_with("-scrolled") {
                overlay.key("End", &theater_resources, &spec);
            }
            if snapshot_state.ends_with("-filtered") {
                overlay.key("Tab", &theater_resources, &spec);
                overlay.text(if snapshot_state.starts_with("respawn-aircraft") {
                    "hornet"
                } else {
                    "120"
                });
            }
            overlay.draw(
                &mut menu.pixels,
                &hornet.font,
                "You can fly again in 0:25 (2 lives left)",
                &menu.quick_sprites,
            );
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for pixel in menu.pixels.chunks_exact(4) {
                f.write_all(&pixel[..3])?;
            }
        } else if snapshot_state.starts_with("lobby") {
            // The lobby screen with a synthetic lobby and lines.
            lobby_screen::preview::render(&menu.kit_source, &snapshot_state, &mut menu.pixels)?;
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if snapshot_state.starts_with("direct") {
            // The Direct Connection screen with synthetic games and lines.
            direct_screen::preview::render(&menu.kit_source, &snapshot_state, &mut menu.pixels)?;
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if snapshot_state.starts_with("internet") {
            // The Internet Lobby screen with synthetic games and lines.
            internet_screen::preview::render(&menu.kit_source, &snapshot_state, &mut menu.pixels)?;
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else if let Some(state) = snapshot_state.strip_prefix("replays") {
            // A synthetic list: no recordings are read or needed.
            let mut screen = replay::screen::Replays::preview("Main menu");
            screen.preview_state(state.trim_start_matches('-'))?;
            menu.preview_state("normal")?;
            menu.render();
            screen.draw(&mut menu.pixels, &hornet.font);
            use std::io::Write;
            let mut f = std::fs::File::create(&path)?;
            write!(f, "P6\n640 480\n255\n")?;
            for p in menu.pixels.chunks_exact(4) {
                f.write_all(&p[..3])?;
            }
        } else {
            menu.preview_state(&snapshot_state)?;
            menu.save_ppm(&path)?;
        }

        println!("Menu preview: {}", path.display());
        return Ok(Outcome::Done);
    }
    let turbulence_enabled = match std::env::var("TORE_TURBULENCE").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("1") => true,
        Ok("0") => false,
        _ => return Err("TORE_TURBULENCE needs 0 or 1".into()),
    };
    let mut camera = camera::Camera::for_world(&world);
    if let Ok(pose) = std::env::var("TORE_WEATHER_VIEW") {
        let values = pose
            .split(',')
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()?;
        if !(5..=6).contains(&values.len())
            || values.iter().any(|v| !v.is_finite())
            || values[..3].iter().any(|v| v.abs() > 2_000_000.)
            || values[3].abs() > 360.
            || values[4].abs() > 90.
            || values.get(5).is_some_and(|roll| roll.abs() > 360.)
        {
            return Err("TORE_WEATHER_VIEW needs x,y,z,yaw,pitch[,roll] in feet/degrees".into());
        }
        camera.position = std::array::from_fn(|i| f64::from(values[i]));
        camera.yaw = values[3].to_radians();
        camera.pitch = values[4].to_radians();
        camera.roll = values.get(5).copied().unwrap_or(0.).to_radians();
    }
    let mut quick =
        quick_mission::QuickMission::new(aircraft_id, creator_options.clone(), &theater_resources);
    quick.ai_mission = ai_mission;
    quick.choose_theater_code(&theater_code, &theater_resources);
    if probe_script.redfor {
        // The Redfor seat's fields are the Red and neutral ones.
        quick.set_player_redfor(true);
    }
    if let Some(stem) = &ground_target {
        quick.choose_ground_target(stem, target_defenses, target_seed)?;
    } else if target_defenses_given {
        quick.choose_area_defenses(target_defenses, target_seed);
    }
    if let Some(object) = ground_start {
        quick.choose_ground_runway(object)?;
    }
    if let Some(id) = enemy_ground_start_airport {
        // Enemy Wing 1's ground start at a Redfor field (slice CL-R).
        let object = world
            .airport_scene
            .airport(id)
            .and_then(|a| a.runway_objects.first())
            .copied()
            .ok_or("Selected enemy ground-start airport is unavailable")?;
        quick.choose_enemy_runway(object)?;
    }
    if let Some(nm) = separation_nm {
        quick.draft.values[17] = mission_layout::SEPARATION_NM
            .iter()
            .position(|choice| *choice == nm)
            .unwrap_or(quick.draft.values[17]);
    }
    if let Some(size) = probe_script.wing_size {
        quick.draft.values[4] = size;
    }
    if launch_creator && let Some((friendly, enemy)) = probe_script.fight {
        for (fields, total) in [([4, 7, 10], friendly), ([21, 24, 27], enemy)] {
            for (n, field) in fields.into_iter().enumerate() {
                quick.draft.values[field] = total.saturating_sub(n * 5).min(5);
            }
        }
    }
    if probe_script.wing_only {
        for field in [7, 10, 21, 24, 27] {
            quick.draft.values[field] = 0;
        }
    }
    probe_script.takeoff = maneuver == "takeoff";
    if rotor_probe {
        // The AI's rotorcraft and jet-lift control on each imported
        // powered-lift PT (slice H1, docs/testing/lane-flight.md).
        for id in tore_formats::aircraft::AircraftId::ALL {
            let Some(bytes) = theater_resources.get(id.pt()) else {
                continue;
            };
            let aircraft = tore_formats::aircraft::Aircraft::parse(bytes)?;
            for line in tore_sim::ai::rotor_probe::report(&aircraft) {
                println!("{line}");
            }
        }
        return Ok(Outcome::Done);
    }
    if let Some(ticks) = ai_probe {
        if ai_roster_probe {
            ai_roster_probe::roster_probe(ticks, &theater_resources, &world)?;
            return Ok(Outcome::Done);
        }
        if let Some(directory) = &probe_script.matrix {
            if directory.exists() {
                return Err(
                    "--probe-matrix needs a new directory; existing evidence is never overwritten"
                        .into(),
                );
            }
            if ground_start.is_some() || probe_script.wing_only || enemy_skill.is_some() {
                return Err("--probe-matrix requires an airborne opposing wing and uses its own four skill settings".into());
            }
            std::fs::create_dir_all(directory)?;
            for player in [
                tore_formats::aircraft::AircraftId::F22,
                tore_formats::aircraft::AircraftId::F22n,
                tore_formats::aircraft::AircraftId::Faxx,
            ] {
                let airframe = aircraft::Airframe::load(&theater_resources, player)?;
                // The matrix is fighters against fighters: the helicopters, the
                // V-22, the AV-8 and the Yak-141 have been the AI's to fly
                // since slice H3, but they stay out of it as before (their
                // own probes are the `fight-heli-*` and `fight-vtol-*`
                // scenarios).
                for enemy in tore_formats::aircraft::AircraftId::SELECTABLE
                    .into_iter()
                    .filter(|enemy| enemy.ai_flyable() && !enemy.is_powered_lift())
                {
                    for skill in 0..4 {
                        for geometry in [
                            ProbeGeometry::Head,
                            ProbeGeometry::Side,
                            ProbeGeometry::Rear,
                        ] {
                            for researched in [false, true] {
                                let mut script = probe_script.clone();
                                script.matrix = None;
                                script.enemy_aircraft = Some(enemy);
                                script.enemy_skill = Some(skill);
                                script.geometry = geometry;
                                script.researched = researched;
                                let mut setup = quick_mission::QuickMission::new(
                                    player,
                                    creator_options.clone(),
                                    &theater_resources,
                                );
                                setup.choose_theater_code(&theater_code, &theater_resources);
                                setup.draft.values[17] = quick.draft.values[17];
                                let name =
                                    probe_case_name(player, enemy, skill, geometry, researched);
                                println!("AI probe matrix: {name}");
                                let recording = ProbeRecord {
                                    path: directory.join(format!("{name}.tore-replay")),
                                    verify: verify_render,
                                };
                                world = ai_probe_run(
                                    ticks,
                                    &mut setup,
                                    &airframe,
                                    &theater_resources,
                                    world,
                                    None,
                                    ai_mission,
                                    &script,
                                    Some(&recording),
                                )?;
                            }
                        }
                    }
                }
            }
            return Ok(Outcome::Done);
        }
        ai_probe_run(
            ticks,
            &mut quick,
            &hornet,
            &theater_resources,
            world,
            enemy_skill,
            ai_mission,
            &probe_script,
            probe_record.as_ref(),
        )?;
        return Ok(Outcome::Done);
    }
    if initial_screen == Screen::Quick && snapshot_state == "ordnance" {
        quick.ordnance = Some(ordnance::Ordnance::new(
            tore_sim::combat::loadout::Loadout::new(&hornet.profile, |n| {
                theater_resources
                    .get(n)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("missing loadout resource"))
            })?,
            &theater_resources,
        )?);
    }
    let animation_capture = capture_terrain.is_some()
        && (flight_devices.is_some()
            || flight_controls.is_some()
            || (flight_throttle.is_some() || flight_bay.is_some())
            || flight_probe_ticks.is_some()
            || damage_preview.is_some()
            || countermeasure_preview.is_some());
    let mut flight = hornet.start(&world);
    if let Ok(value) = std::env::var("TORE_FLIGHT_AGL") {
        let agl = value.parse::<f64>()?;
        if !agl.is_finite() || !(10. ..=90000.).contains(&agl) {
            return Err("TORE_FLIGHT_AGL needs 10..90000 feet".into());
        }
        flight.position[1] =
            f64::from(world.height(flight.position[0] as f32, flight.position[2] as f32)) + agl;
    }
    flight.jammer = jammer_on;
    if researched_flight {
        flight.enable_research(1)?;
        if ground_start.is_none() {
            flight.start_airborne(world.wind());
        }
    }
    if let Some(tables) = &native_tables {
        flight.enable_native(tables.clone(), 1)?;
    }
    if let Some(object) = ground_start {
        let (position, heading) = mission_layout::runway_pose(&world, object)?;
        flight.position = [
            position[0],
            flight.position[1].max(position[1] + 5000.),
            position[2],
        ];
        flight.yaw = heading;
        let basis = attitude::Basis::new(heading, 0., 0.);
        flight.velocity =
            std::array::from_fn(|i| basis.forward[i] * flight.speed + world.wind()[i]);
    }
    // The probe advances weather and vapor with the flight so captures taken
    // after it show the same environment and trail history a live run would.
    let mut probe_vapor =
        tore_sim::vapor::Vapor::seeded(hornet.streamer_points(&flight).unwrap_or([[0.; 3]; 2]));
    let mut probe_turbulence = tore_sim::turbulence::Turbulence::default();
    let mut probe_turbulence_rng = tore_formats::flight_model::clock_rng::NativeRng::seeded(1)?;
    if ground_start.is_none()
        && let Some(ticks) = flight_probe_ticks
    {
        let keys = setup_maneuver(&mut flight);
        if matches!(maneuver.as_str(), "stall" | "spin") {
            for (v, wind) in flight.velocity.iter_mut().zip(world.wind()) {
                *v += wind;
            }
        }
        for _ in 0..ticks {
            flight.step_surface(&keys, |x, z| world.surface(x, z));
            if let Some(error) = flight.native_fault() {
                return Err(error.into());
            }
            let mut weather_view = hornet.camera(&flight, flight_view, Default::default());
            look::apply(
                &mut weather_view,
                flight.position,
                flight_look.map(f32::to_radians),
                matches!(flight_view, 1 | 2),
            );
            world.weather.step();
            scenery.step_view_weather(&world, &weather_view, flight.speed);
            scenery.step_view_weather(&world, &mirrors::camera(&flight), flight.speed);
            for page in [2, 3] {
                scenery.step_view_weather(
                    &world,
                    &hornet.panel_camera(&flight, page),
                    flight.speed,
                );
            }
            step_turbulence(
                &mut probe_turbulence,
                &mut probe_turbulence_rng,
                &mut flight,
                &world,
                turbulence_enabled,
            );
            if let Some(points) = hornet.streamer_points(&flight) {
                probe_vapor.step(world.weather.ticks(), points);
            }
        }
    }
    // Bounded wing-vapor probe: prints the resolved trail without a window.
    if std::env::var_os("TORE_VAPOR_PROBE").is_some() {
        if let Some(def) = &hornet.streamer {
            for side in 0..2 {
                let p = def.attachment(side, 0)?;
                let nearest = hornet.poses[0]
                    .faces
                    .iter()
                    .flat_map(|f| &f.positions)
                    .map(|v| {
                        let v = v.map(f64::from);
                        let distance =
                            ((v[0] - p[0]).powi(2) + (v[1] - p[2]).powi(2) + (v[2] - p[1]).powi(2))
                                .sqrt()
                                / 3.;
                        (distance, v)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                println!(
                    "CE side={side} right/up/forward={p:?}; nearest mesh distance_ft/vertex={nearest:?}"
                );
            }
        }
        println!(
            "wing vapor: g={:.2} roll_rate_deg={:.1} position={:?}",
            flight.g,
            flight.roll_rate.to_degrees(),
            flight.position.map(|v| v.round())
        );
        for side in 0..2 {
            let hazing = world
                .weather
                .sample(flight.position[1])
                .is_some_and(|l| l.night_hazing());
            match probe_vapor.trail(side, flight.g, flight.roll_rate.to_degrees(), hazing) {
                Some(trail) => {
                    for (i, p) in trail.iter().enumerate() {
                        println!("  side {side} point {i}: {:?}", p.map(|v| v.round()));
                    }
                }
                None => println!("  side {side}: no trail"),
            }
        }
    }
    if std::env::var_os("TORE_ENVIRONMENT_PROBE").is_some() {
        println!(
            "Environment: wind={:?} world_fps={:?} air_data={:?}",
            world.weather.configuration().wind(),
            world.wind(),
            world.air_data(&flight)
        );
        println!(
            "Turbulence: enabled={turbulence_enabled} state={probe_turbulence:?}; position={:?} attitude={:?}",
            flight.position,
            [flight.yaw, flight.pitch, flight.bank]
        );
        return Ok(Outcome::Done);
    }
    if let Some(v) = flight_devices {
        if v[4] > 0.
            && flight
                .model()
                .configuration()
                .propulsion
                .afterburner_thrust_lbf
                <= 0.
        {
            return Err(
                "the selected aircraft has no afterburner; set the fifth device fraction to 0"
                    .into(),
            );
        }
        if v[3] > 0. && !flight.hook_available() {
            return Err(
                "the selected aircraft has no hook; set the fourth device fraction to 0".into(),
            );
        }
        flight.gear = v[0];
        flight.flaps = v[1];
        flight.brake = v[2];
        flight.hook = v[3];
        flight.exhaust = v[4];
        flight.gear_down = v[0] > 0.;
        flight.flaps_down = v[1] > 0.;
        flight.brake_out = v[2] > 0.;
        flight.hook_down = v[3] > 0.;
        flight.burner = v[4] > 0.;
        if flight.burner {
            flight.throttle = 1.;
        }
    }
    if let Some(value) = flight_throttle {
        flight.throttle = value;
    }
    if let Some(v) = flight_controls {
        flight.elevator = v[0];
        flight.aileron = v[1];
        flight.rudder = v[2];
    }

    // Headless probes and captures have no instrument panel to read, so the
    // command line supplies the same sensor controls a player would set.
    flight.sensors = sensor_controls(sensor_channel, scope_range, scope_history);
    if stripped_loadout.is_some() && live_fire {
        return Err(
            "--loadout cannot be combined with --live-fire, which starts the PT-default range"
                .into(),
        );
    }
    let mut combat = build_combat(
        &hornet,
        &theater_resources,
        live_fire,
        stripped_loadout.as_deref(),
    )?;
    let mut combat_view =
        combat_view::CombatView::new(&combat, combat.own_id(), &theater_resources)?;
    combat.add_airport_targets(&world.airport_scene)?;
    let combat_tape = match record_combat {
        Some(ref path) => {
            let mut writer = tape_file::Recorder::new(
                path,
                &theater_resources,
                combat.state.own().configuration(),
                &theater_code,
            )?;
            writer.model(&combat.state)?;
            combat.start_tape();
            Some(writer)
        }
        None => None,
    };
    combat.clean_recording = record_input.is_some();
    if combat.clean_recording {
        log::warn!(
            "Pilot-only recording keeps the existing clean-aircraft load; use combat recording for weapons."
        );
    }
    combat_view.mission_dummies(&mut combat, &dummy_aircraft, 5280., &theater_resources)?;
    combat.reset(&mut flight)?;
    for name in &flight_cheats {
        apply_probe_cheat(&mut flight.cheats, name);
        apply_probe_cheat(&mut combat.state.cheats, name);
    }
    let normal_startup_defaults = !live_fire
        && record_input.is_none()
        && replay_frames.is_none()
        && combat_probe.is_none()
        && record_combat.is_none();
    if normal_startup_defaults {
        combat.apply_startup_weapons();
    }
    if let Some(object) = ground_start {
        mission_layout::apply_ground_start(&world, &mut flight, object)?;
        probe_vapor =
            tore_sim::vapor::Vapor::seeded(hornet.streamer_points(&flight).unwrap_or([[0.; 3]; 2]));
        if let Some(ticks) = flight_probe_ticks {
            let keys = setup_maneuver(&mut flight);
            for _ in 0..ticks {
                flight.step_surface(&keys, |x, z| world.surface(x, z));
                if let Some(error) = flight.native_fault() {
                    return Err(error.into());
                }
                let mut weather_view = hornet.camera(&flight, flight_view, Default::default());
                look::apply(
                    &mut weather_view,
                    flight.position,
                    flight_look.map(f32::to_radians),
                    matches!(flight_view, 1 | 2),
                );
                world.weather.step();
                scenery.step_view_weather(&world, &weather_view, flight.speed);
                scenery.step_view_weather(&world, &mirrors::camera(&flight), flight.speed);
                for page in [2, 3] {
                    scenery.step_view_weather(
                        &world,
                        &hornet.panel_camera(&flight, page),
                        flight.speed,
                    );
                }
                step_turbulence(
                    &mut probe_turbulence,
                    &mut probe_turbulence_rng,
                    &mut flight,
                    &world,
                    turbulence_enabled,
                );
                if let Some(points) = hornet.streamer_points(&flight) {
                    probe_vapor.step(world.weather.ticks(), points);
                }
            }
        }
    }
    if let Some((airport, mut aircraft, angles)) = airport_probe {
        flight.position = aircraft.position;
        flight.gear_down = aircraft.gear_down;
        flight.gear = f64::from(aircraft.gear_down);
        if let Some([heading, pitch]) = angles {
            flight.yaw = heading.to_radians();
            flight.pitch = pitch.to_radians();
            flight.bank = 0.;
        }
        aircraft.forward = attitude::Basis::new(flight.yaw, flight.pitch, flight.bank).forward;
        airport_probe = Some((airport, aircraft, angles));
        println!(
            "airport_probe_pose position={:?} forward={:?} heading_deg={:.3} pitch_deg={:.3}",
            aircraft.position,
            aircraft.forward,
            flight.yaw.to_degrees(),
            flight.pitch.to_degrees()
        );
    }
    if let Some(value) = flight_bay {
        if !flight.bay_available() {
            return Err("selected aircraft has no reviewed weapon bay".into());
        }
        flight.bay = value;
        flight.bay_open = value > 0.;
    }

    if let Some(mode) = std::env::var_os("TORE_EFFECT_PREVIEW") {
        if mode == "hill" {
            // Start short of the nearest hillside, facing uphill, so the
            // preview's craters lie on a slope.
            let (site, yaw) =
                combat::Combat::preview_hillside(&world, [flight.position[0], flight.position[2]])
                    .ok_or("no hillside near the start")?;
            let (sin, cos) = yaw.sin_cos();
            flight.yaw = yaw;
            flight.position = [site[0] - sin * 2600., site[1] + 700., site[2] - cos * 2600.];
            println!(
                "Effect preview hillside at {site:?}, facing {:.0} degrees",
                yaw.to_degrees()
            );
        }
        combat.preview_effects(&flight, &world);
        combat.refresh_render(combat.own_id(), &flight, None);
    }
    if let Some(weapon_slot) = weapon_slot {
        if weapon_slot == 0 || weapon_slot > combat.state.own().ammo.len() {
            return Err("weapon slot outside this aircraft's PT loadout".into());
        }
        // The cycle skips a station that was never loaded (a retained
        // selection station), so it may never land on the one asked for: one
        // full turn of the ring (the stations and NAV) is the limit.
        for _ in 0..=combat.state.own().ammo.len() {
            if combat.state.own().selected == weapon_slot - 1 {
                break;
            }
            combat.command(
                tore_sim::combat::live::Command::NextWeapon,
                combat::launcher(&flight),
            );
        }
        // A station that carries nothing cannot be selected; start on NAV.
        if !combat
            .state
            .own()
            .carries(weapon_slot - 1, combat.state.cheats.unlimited_ammo)
        {
            eprintln!("Weapon slot {weapon_slot} carries nothing; starting on NAV");
            combat.command(
                tore_sim::combat::live::Command::SelectNav,
                combat::launcher(&flight),
            );
        }
    }
    // Scripted setup keeps the render history as live flight does: a changed
    // scene is retaken at once and each combat step ends a tick.
    if live_fire {
        // An empty station is never armed.
        let unlimited = combat.state.cheats.unlimited_ammo;
        let own = combat.state.own_mut();
        own.armed = own.carries(own.selected, unlimited);
        combat.command(
            tore_sim::combat::live::Command::ReplaceTarget,
            combat::launcher(&flight),
        );
        combat.refresh_render(combat.own_id(), &flight, None);
        // A scripted designation needs a current observation first, exactly as
        // a player's click does.
        combat.step(&mut flight, &world)?;
        combat.advance_render(combat.own_id(), &flight, None);
    }
    let payload_start = combat.state.own().payload_lbs();
    for command in combat_commands {
        combat.command(command, combat::launcher(&flight));
    }
    combat.refresh_render(combat.own_id(), &flight, None);
    // Lets released chaff and flares develop before a capture.
    if let Some(ticks) = countermeasure_preview {
        for _ in 0..ticks {
            flight.step(&tore_input::PilotInput::default(), |x, z| {
                f64::from(world.height(x as f32, z as f32))
            });
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        let devices = &combat.state.devices;
        println!(
            "Countermeasure preview: ticks={ticks} flares={} burning={} puffs={} chaff={} carried_chaff={} carried_flares={} capacity_chaff={} capacity_flares={}",
            devices.flares.len(),
            devices.flares.iter().filter(|f| f.burning()).count(),
            devices.puffs().count(),
            devices.chaff.len(),
            combat.state.own().chaff,
            combat.state.own().flares,
            combat.state.own().configuration().ecm.chaff[0],
            combat.state.own().configuration().ecm.flare[0]
        );
        for flare in &devices.flares {
            let p = flare.position;
            println!(
                "  flare at {:.0},{:.0},{:.0}, {:.0} ft above ground; aircraft {:.0},{:.0},{:.0}",
                p[0],
                p[1],
                p[2],
                p[1] - f64::from(world.height(p[0] as f32, p[2] as f32)),
                flight.position[0],
                flight.position[1],
                flight.position[2]
            );
        }
    }
    if let Some(ticks) = combat_probe {
        combat.command(
            tore_sim::combat::live::Command::Designate,
            combat::launcher(&flight),
        );
        // Half a second of tracking before the probe fires, so a radar weapon
        // has the same support a player would wait for.
        for _ in 0..tore_sim::sensors::track::ACQUISITION_STEPS {
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        combat.own_trigger().input.space(true, false, false);
        let mut feedback = tore_input::FeedbackMixer::default();
        let mut cues = std::collections::BTreeMap::<String, usize>::new();
        let mut pulses = 0;
        for _ in 0..ticks {
            flight.step(&flight::PilotInput::default(), |x, z| {
                f64::from(world.height(x as f32, z as f32))
            });
            for event in combat.step(&mut flight, &world)? {
                if let Some(cue) =
                    combat::feedback(&event, combat.own_id(), combat.state.own().configuration())
                {
                    *cues.entry(format!("{cue:?}")).or_default() += 1;
                    feedback.event(cue);
                }
            }
            combat.advance_render(combat.own_id(), &flight, None);
            if matches!(
                feedback.tick(),
                Some(tore_input::FeedbackUpdate::Pulse { .. })
            ) {
                pulses += 1;
            }
        }
        let status = {
            let readout = combat
                .cockpit_readout(combat.own_id(), combat::launcher(&flight), None, None)
                .expect("the probe's plane has an ownship");
            combat_view::status(
                &combat,
                combat.state.own().configuration(),
                &readout,
                &flight,
            )
        };
        println!(
            "Combat probe feedback generation (no hardware playback): {cues:?}, pulses={pulses}; {status}"
        );
        combat.cancel();
        println!(
            "Combat probe: {} shots={} hits={} kills={} active={} ammo={:?} payload_start_lb={:.0} payload_lb={:.0} flight_payload_lb={:.0} external_fuel_lb={:.0}",
            hornet.profile.name,
            combat.state.own().shots,
            combat.state.own().hits,
            combat.state.own().kills,
            combat.state.projectiles.len(),
            combat.state.own().ammo,
            payload_start,
            combat.state.own().payload_lbs(),
            flight.carried_lbs(),
            flight.systems.external_lbs()
        );
    }
    if let Some([bearing, elevation, range]) = hud_target_preview {
        for _ in 0..120 {
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        let id = combat
            .state
            .targets
            .iter()
            .find(|t| t.role == tore_sim::combat::missiles::TargetRole::Aircraft)
            .ok_or("HUD preview requires range aircraft")?
            .id;
        combat.command(
            tore_sim::combat::live::Command::DesignateTarget(id),
            combat::launcher(&flight),
        );
        if combat.state.own_view().designated() != Some(id) {
            return Err("HUD preview target could not be observed before repositioning".into());
        }
        let body = tore_sim::attitude::Basis::new(flight.yaw, flight.pitch, flight.bank);
        let (bearing, elevation) = (bearing.to_radians(), elevation.to_radians());
        let target = combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap();
        target.position = std::array::from_fn(|i| {
            flight.position[i]
                + range
                    * (body.forward[i] * bearing.cos() * elevation.cos()
                        + body.right[i] * bearing.sin() * elevation.cos()
                        + body.up[i] * elevation.sin())
        });
        combat.refresh_render(combat.own_id(), &flight, None);
        for _ in 0..24 {
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        println!(
            "HUD target preview: bearing={} elevation={} display={:?} sensor={:?}",
            bearing.to_degrees(),
            elevation.to_degrees(),
            combat.state.own_view().display_target().map(|t| t.id),
            combat.state.own_view().designated()
        );
    }
    if let Some([heading, elevation, ticks]) = sight_preview {
        // Hold the AC-130's sight at a body-relative look and let the guns
        // follow for a while, so a capture shows the aim-point marks of a
        // slewing, parked or trained gun. A pending pin resolves on the first
        // step, at this look.
        {
            let group = combat
                .state
                .own_mut()
                .gunship
                .as_mut()
                .ok_or("--sight-preview needs an AC-130")?;
            group.look = [heading.to_radians(), elevation.to_radians()];
        }
        for _ in 0..ticks as u64 {
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        let group = combat.state.own().gunship.as_ref().unwrap();
        println!(
            "Sight preview: sight={:?} look={:?} aim={:?} status={:?} impacts={:?}",
            group.sight, group.look, group.aim, group.status, group.impacts
        );
    }
    if let Some(fraction) = damage_preview {
        combat
            .state
            .preview_localized_damage(damage_preview_section, fraction);
        combat.state.own_mut().hp = (f64::from(combat.state.own().configuration().damage_capacity)
            * (1. - fraction))
            .round() as i32;
        for target in &mut combat.state.targets {
            target.hp = (f64::from(target.initial_hp) * (1. - fraction)).round() as i32;
        }
        combat.refresh_render(combat.own_id(), &flight, None);
        for _ in 0..damage_preview_ticks {
            flight.step(&tore_input::PilotInput::default(), |x, z| {
                f64::from(world.height(x as f32, z as f32))
            });
            combat.step(&mut flight, &world)?;
            combat.advance_render(combat.own_id(), &flight, None);
        }
        println!(
            "Damage preview: ticks={} debris={} impacts={} smoke={}",
            damage_preview_ticks,
            combat.state.debris.len(),
            combat
                .state
                .effects
                .iter()
                .filter(|e| e.kind == tore_sim::combat::live::EffectKind::DebrisImpact)
                .count(),
            combat.state.smoke.puffs.len()
        );
    }
    if damage_preview.is_some()
        && let Some(wreck) = &flight.wreck
    {
        println!(
            "Wreck preview: phase={:?} ticks={} polls={} engine_acceleration={:?} position={:?} attitude={:?}",
            wreck.phase,
            wreck.ticks,
            wreck.polls,
            wreck.power.acceleration,
            flight.position,
            [flight.yaw, flight.pitch, flight.bank]
        );
    }
    if let Some(phase) = &ejection_preview {
        if !flight.eject() {
            return Err("Ejection preview requires a living pilot and available seat".into());
        }
        let pilot = flight.escape.as_mut().unwrap();
        pilot.phase = match phase.as_str() {
            "seat" => tore_sim::ejection::Phase::Seat,
            "freefall" => tore_sim::ejection::Phase::Freefall,
            _ => tore_sim::ejection::Phase::Parachute,
        };
        // Isolate the original pilot model from the abandoned aircraft in this art fixture.
        pilot.position[0] += 300.;
        // A two-seater's second crew member leaves with him: the same phase,
        // beside him and clear of the aircraft.
        let escape_phase = pilot.phase;
        if let Some(crew) = flight.crew_escape.as_mut() {
            crew.phase = escape_phase;
            crew.position[0] += 300. + 40.;
        }
        flight_view = 1;
        println!(
            "Ejection preview: {phase}, pilot_alive={}",
            !flight.systems.pilot.dead
        );
    }
    if flight.systems.pilot.dead {
        flight_view = 1;
        if damage_preview.is_some() {
            println!("Pilot death preview: exterior view {flight_view}");
        }
    }
    // The first frame draws the prepared scene, ejected pilot included.
    combat.refresh_render(combat.own_id(), &flight, None);
    // The saved controls file loads without being asked for; a damaged one
    // falls back to the default controls with a warning. A file named with
    // `--input-profile` is what the player asked for and fails loudly.
    let explicit_input_profile = input_profile.is_some();
    if input_profile.is_none() {
        let default = assets::data_directory()?.join("input-v1.conf");
        if default.exists() {
            input_profile = Some(default);
        }
    }
    if record_input.is_some()
        && (initial_screen != Screen::Flight || animation_capture || flight_probe_ticks.is_some())
    {
        return Err(
            "--record-input requires direct --free-flight without a capture or flight probe".into(),
        );
    }
    let input_recording = if let Some(path) = record_input {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
        );
        writeln!(file, "{}", tore_input::recording::HEADER)?;
        Some(file)
    } else {
        None
    };
    let preferences_enabled = !smoke_test
        && capture_terrain.is_none()
        && !animation_capture
        && std::env::var_os("TORE_PERF_FRAMES").is_none()
        && std::env::var_os("TORE_PERF_TICKS").is_none();
    // Diagnostics ignore saved choices, like the other display preferences.
    let graphics_path = if preferences_enabled {
        Some(assets::data_directory()?.join("graphics-v1.conf"))
    } else {
        None
    };
    let mut graphics = graphics_path
        .as_deref()
        .map_or_else(graphics::Options::default, graphics::Options::load);
    graphics.apply_flags(&graphics_flags)?;
    let mut airport_service =
        tore_sim::airport::Service::new(&world.airport_scene).map_err(std::io::Error::other)?;
    let airport_nav_mode = airport_probe.map_or_else(
        || {
            (ground_start.is_some() && weapon_slot.is_none() && !live_fire)
                || (stripped_loadout.is_some()
                    && !combat.state.own().carries(
                        combat.state.own().selected,
                        combat.state.cheats.unlimited_ammo,
                    ))
        },
        |(_, aircraft, _)| aircraft.nav_mode,
    );
    if airport_nav_mode {
        combat.state.command(
            0,
            tore_sim::combat::live::Command::SelectNav,
            combat::launcher(&flight),
        );
    }
    if let Some(object) = ground_start {
        let airport = world.airport_scene.runway(object).unwrap().airport;
        airport_service.command(
            &world.airport_scene,
            airport_aircraft(&world, &flight, airport_nav_mode, false),
            tore_sim::airport::Command::SelectAirport(airport),
        );
    }
    if let Some((airport, aircraft, _)) = airport_probe {
        airport_service.command(
            &world.airport_scene,
            aircraft,
            tore_sim::airport::Command::SelectAirport(airport),
        );
        airport_service.command(
            &world.airport_scene,
            aircraft,
            tore_sim::airport::Command::RequestLanding,
        );
    }
    diagnostics::stage_done();
    let mut replay = match &watch_replay {
        Some(path) => {
            diagnostics::stage("replay loading");
            let mut viewer =
                replay::viewer::Viewer::open(path, &theater_resources, &replay_options)?;
            let capture = replay_capture.map(|path| replay::host::Capture { path });
            // A capture shows a finished picture: trails, fallen buildings
            // and the weather need the whole recording read.
            if capture.is_some() {
                viewer.finish_tracks();
            }
            diagnostics::stage_done();
            Some(Box::new(replay::host::Replay { viewer, capture }))
        }
        None => None,
    };
    diagnostics::stage("controller and input initialization");
    let input = if explicit_input_profile {
        input::Input::new(input_profile.as_deref(), native_input)?
    } else {
        input::Input::new_automatic(input_profile.as_deref(), native_input)?
    };
    diagnostics::stage_done();
    if let Some(replay) = &mut replay {
        replay.viewer.set_input_profile(&input.resolver.profile);
    }
    diagnostics::stage("application state construction");
    let mut airfield_radio = airfield_radio::AirfieldRadio::default();
    airfield_radio.reset(ground_start.and_then(|id| world.runway_view(id)));
    let world = world::World {
        choices: Default::default(),
        setup: world::Setup {
            mission: None,
            ground_start,
            enemy_ground_start: None,
            researched_flight,
            native_tables: native_tables.clone(),
            ai: None,
        },
        phrases: comms::phrases(&theater_resources),
        roster: seats::Roster::single_player(comms::crew(&hornet.profile), []),
        cockpits: vec![world::Cockpit {
            plane: seats::PlaneId(0),
            previous_flight: flight.clone(),
            flight,
            airport_service,
            airport_nav_mode,
            turbulence: probe_turbulence,
            turbulence_rng: probe_turbulence_rng,
            airfield_radio,
            crew_voice: crew_voice::CrewVoice::new(&hornet.profile),
            result: Default::default(),
            overspeed_message_at: None,
            edge_message_at: None,
        }],
        terrain: world,
        combat,
        ai_wings: None,
        comms: comms::Comms::new(1),
        wing_status: Default::default(),
        datalink: Default::default(),
        score: None,
        revival: Default::default(),
        radio: Default::default(),
    };
    let presented_plane = world.picture_plane().0;
    // The creator opens as the profile left it, or on random aircraft the
    // first time (slice BQ, B22). Only an ordinary interactive start: the
    // diagnostic, scripted and flag-driven runs keep the creator they asked
    // for, so their missions stay the same every time.
    let ordinary_start = preferences_enabled
        && input_script_steps.is_none()
        && !launch_creator
        && matches!(initial_screen, Screen::Main | Screen::Quick)
        && ground_start.is_none()
        && ground_target.is_none()
        && enemy_ground_start_airport.is_none()
        && separation_nm.is_none()
        && !target_defenses_given
        && probe_script.wing_size.is_none()
        && !probe_script.wing_only
        && !probe_script.redfor;
    // The custom loadouts flown come back for the same mission (item 19).
    let quick_loadouts = if ordinary_start && stripped_loadout.is_none() {
        Some(quick_mission::loadout_memory::LoadoutMemory::open(
            assets::data_directory()?.join("quick-loadouts-v1.conf"),
        ))
    } else {
        None
    };
    let quick_memory = if ordinary_start {
        let memory = quick_mission::remember::Memory::open(
            &mut quick,
            assets::data_directory()?.join("quick-mission-v1.conf"),
            aircraft_requested,
        );
        if memory.as_ref().is_some_and(|memory| memory.recalled) {
            // The command line still wins over the saved draft.
            if std::env::args().any(|a| a == "--theater") {
                quick.choose_theater_code(&theater_code, &theater_resources);
            }
            if aircraft_requested {
                quick.choose_player_aircraft(aircraft_id);
            }
        }
        memory
    } else {
        None
    };
    let updates = updates::Updates::new(if ordinary_start {
        Some(assets::data_directory()?)
    } else {
        None
    });
    menu.state.updates_enabled = updates.enabled;
    let mut app = App {
        updates,
        creator_loading_view: None,
        network_loading_stage: None,
        loading_heartbeat: None,
        loading_max_gap: Duration::ZERO,
        background_finishes: Vec::new(),
        pending_host: None,
        loading_pixels: Vec::new(),
        pending_join: None,
        net: None,
        net_built: None,
        mission_markup: None,
        mission_brief_sentences: None,
        net_flight: None,
        net_ending: None,
        observing: None,
        direct: Default::default(),
        internet: Default::default(),
        lobby: Default::default(),
        multiplayer_menu: Default::default(),
        leave_dialog: None,
        leave_approved: None,
        connect,
        launch_creator,
        quick_loadout: stripped_loadout.clone(),
        ai_wings_enabled: !fixture_wings,
        enemy_skill,
        ai_mission,
        world: loading::WorldSlot(Some(world)),
        scenery,
        combat_view,
        combat_tape,
        preference_path: if preferences_enabled {
            Some(assets::data_directory()?.join("preferences-v1.conf"))
        } else {
            None
        },
        loading: None,
        mission_selection_ready: false,
        mission_loading_progress: None,
        restart_recorded: false,
        prepared_recording: None,
        prepared_restart: None,
        prepared_scenery: None,
        preference_saved: String::new(),
        graphics,
        graphics_path,
        sound: sound_prefs::Settings::default(),
        sound_path: if preferences_enabled {
            Some(assets::data_directory()?.join("sound-v1.conf"))
        } else {
            None
        },
        input_recording,
        recorded_ticks: 0,
        input,
        focused: true,
        performance: performance::Performance::from_env()?,
        hornet,
        researched_flight,
        native_tables,
        flight_clock: flight::Clock { remainder: 0. },
        sight: gunsight_view::Sight::default(),
        console: sensor_console::Console::default(),
        seeker: seeker_view::SeekerCam::default(),
        gun_flash: Default::default(),
        flight_music: Default::default(),
        rwr_warnings: Default::default(),
        launch_watch: Default::default(),
        engine_sounds: Default::default(),
        spatial_steps: Vec::new(),
        vapor: probe_vapor,
        g_effects: Default::default(),
        flight_view,
        view_rig: {
            let mut rig = flight_views::Rig::for_plane(presented_plane);
            rig.select(flight_reference);
            rig
        },
        flight_canvas: Default::default(),
        window_size,
        fullscreen: window.fullscreen,
        fullscreen_preference: window.preference,
        flight_ui: {
            let mut ui = flight_ui::FlightUi::default();
            ui.cheats.no_turbulence = !turbulence_enabled;
            ui.menu = flight_menu;
            ui.map.open = flight_map;
            ui.map.mission_tab = mission_brief;
            ui.weapon_diagnostics = weapon_diagnostics;
            ui.debug_panels = debug_panels;
            ui.ai_test = probe_script.ai_test;
            ui.paused = animation_capture || combat_probe.is_some() || ejection_preview.is_some();
            ui.look = flight_look.map(f32::to_radians);
            ui.zoom = flight_zoom;
            if !matches!(flight_view, 1 | 2) {
                ui.look[1] = ui.look[1].clamp(0., std::f32::consts::FRAC_PI_2);
            }
            ui
        },
        instruments: {
            let mut i = instruments::Instruments::new(instrument_layout, instrument_page);
            apply_sensor_controls(&mut i, sensor_channel, scope_range, scope_history);
            i
        },
        pointer: None,
        theater_resources: {
            // The creator's Map builds its mission from the same data
            // (slice CL-R).
            let resources = Arc::new(theater_resources);
            quick.set_resources(Arc::clone(&resources));
            if launch_creator {
                // A command-line launch starts from the draft as the
                // creator's first frame leaves it.
                quick.settle_distance();
            }
            resources
        },
        seat_commands: Vec::new(),
        gun_key_sent: false,
        cheats_sent: None,
        camera,
        quick,
        quick_memory,
        quick_loadouts,
        screen: initial_screen,
        frame_time: Instant::now(),
        instrument_time: Instant::now(),
        target_refresh: target_preview::Refresh::new(),
        formation_trace: None,
        menu,
        audio,
        renderer: None,
        modifiers: ModifiersState::empty(),
        smoke_test,
        capture_terrain,
        reimport: None,
        controls: None,
        feedback: None,
        graphics_screen: None,
        sound_screen: None,
        replay,
        mouse_look: None,
        wheel: 0.,
        head_look: [0.; 2],
        finished: false,
        exit_after_net_debrief: false,
        next_frame: None,
        menu_pace: net::pace::MenuPace::new(Instant::now()),
        pacer: Default::default(),
        stillness: Default::default(),
        occluded: false,
        stopped_frame: false,
        error: None,
        replay_library: match std::env::var("TORE_RECORD_MISSIONS").as_deref() {
            Err(std::env::VarError::NotPresent) => preferences_enabled,
            Ok("1") => true,
            Ok("0") => false,
            _ => return Err("TORE_RECORD_MISSIONS needs 0 or 1".into()),
        }
        .then(|| assets::data_directory().map(|data| replay::library::Library::new(&data)))
        .transpose()?,
        replay_recorder: None,
        live_debug: Default::default(),
        replays_screen: None,
        script: input_script_steps.map(input_script::Runner::new),
    };
    app.live_debug.requests = flight_panels;
    diagnostics::stage_done();
    diagnostics::stage("saved preferences");
    if let Some(path) = app.preference_path.clone() {
        match preferences::read(&path) {
            Ok(text) => match preferences::Preferences::parse(&text) {
                Ok(saved) => {
                    saved.apply(&mut app.flight_ui, &mut app.instruments);
                    if std::env::args().any(|a| a == "--instrument-layout")
                        && app.instruments.layout != instrument_layout
                    {
                        app.instruments.toggle_layout();
                    }
                    if let Some(page) = instrument_page {
                        app.instruments.pages = vec![page];
                        app.instruments.selected = 0;
                    }
                    apply_sensor_controls(
                        &mut app.instruments,
                        sensor_channel,
                        scope_range,
                        scope_history,
                    );
                    if std::env::args().any(|a| a == "--flight-zoom") {
                        app.flight_ui.zoom = flight_zoom;
                    }
                    if weapon_diagnostics {
                        app.flight_ui.weapon_diagnostics = true;
                    }
                    if debug_panels {
                        app.flight_ui.debug_panels = true;
                    }
                }
                Err(e) => {
                    log::warn!("Preferences not loaded: {e}; preserving original file");
                    app.preference_path = None;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!("Preferences not loaded: {e}; preserving original file");
                app.preference_path = None;
            }
        }
    }
    if let Some(path) = app.sound_path.clone() {
        // A profile saved before the Sound screen carries its Music and
        // Effects switches over once.
        let legacy = app
            .preference_path
            .as_deref()
            .and_then(|p| preferences::read(p).ok())
            .and_then(|text| preferences::legacy_sound(&text))
            .map(|(music, effects)| sound_prefs::Settings::from_legacy(music, effects));
        app.sound = sound_prefs::Settings::load(&path, legacy.unwrap_or_default());
        if legacy.is_some()
            && !path.exists()
            && let Err(e) = app.sound.save(&path)
        {
            log::warn!("Sound settings not saved: {e}");
        }
    }
    if app.native_tables.is_some() {
        app.flight_ui.cheats.no_turbulence = true;
    }
    if let Some(mode) = &window_layout_preview {
        // A capture of the real flight screen with the seeker window opened.
        app.instruments.preview_window_layout(mode);
        // No seeker weapon is flying here, so keep the window up.
        app.instruments.keep_seeker_window();
    }
    if let Ok(value) = std::env::var("TORE_COCKPIT_LIGHTING") {
        (app.flight_ui.lighting, app.flight_ui.brightness) = cockpit_lighting::preview(&value)?;
    }
    app.preference_saved = preferences::Preferences::capture(
        &app.flight_ui,
        &app.instruments,
        app.fullscreen_preference,
    )
    .text();
    if let Some(audio) = &app.audio {
        audio.scene(match app.screen {
            Screen::Flight | Screen::Replay => audio::music::Scene::Score(0),
            Screen::Main => audio::music::Scene::Main,
            _ => audio::music::Scene::Brief,
        });
        audio.set_volumes(app.sound.volumes());
        // A replay starts silent; its sound follows the playhead.
        if app.screen == Screen::Replay {
            audio.restart_flight();
        }
    }
    if controls_menu {
        app.flight_command(flight_ui::Command::ControlsOpen);
    }
    // `--replay-menu graphics|sound|controls`: that screen over the replay's Escape
    // menu, for captures.
    match replay_options.menu {
        Some(replay::pause::Start::Graphics) => app.open_graphics("Replay paused"),
        Some(replay::pause::Start::Sound) => app.open_sound(true),
        Some(replay::pause::Start::Controls) => app.open_controls("Replay paused"),
        _ => {}
    }
    app.input.context(
        app.screen != Screen::Flight || app.flight_ui.frozen(),
        app.focused,
    );
    diagnostics::stage_done();
    shared_event_loop(event_loop)?.run_app_on_demand(&mut app)?;
    // The window is closed before waiting for disk flushes, footer writing
    // and any load or graphics retirement still running.
    if let Some(job) = app.loading.take()
        && let Some(handle) = job.into_thread()
    {
        let _ = handle.join();
    }
    for finish in app.background_finishes.drain(..) {
        let _ = finish.join();
    }
    if let Some(error) = app.error {
        return Err(error);
    }
    match app.reimport {
        Some(frame) => Ok(Outcome::Reimport(frame)),
        None => Ok(Outcome::Done),
    }
}

/// Headless sensor control overrides, so a capture or probe can exercise the
/// infrared channel, another scope setting and the history trail.
fn sensor_controls(
    channel: Option<usize>,
    range: Option<usize>,
    history: bool,
) -> tore_sim::sensors::Controls {
    tore_sim::sensors::Controls {
        channel: if channel == Some(1) {
            tore_sim::sensors::Channel::Infrared
        } else {
            tore_sim::sensors::Channel::Radar
        },
        range_index: range.unwrap_or(tore_sim::sensors::DEFAULT_RANGE_INDEX),
        history,
    }
}
fn apply_sensor_controls(
    instruments: &mut instruments::Instruments,
    channel: Option<usize>,
    range: Option<usize>,
    history: bool,
) {
    if let Some(channel) = channel {
        instruments.channel = channel;
    }
    if let Some(range) = range {
        instruments.radar_range = range;
    }
    if history {
        instruments.history = true;
    }
}
/// Profile names of the mouse buttons other than the left one.
/// The AC-130's sensor console is open on the presented flight: a client's
/// prediction carries the host's autopilot mode.
fn console_open(
    input: &input::Input,
    world: &world::World,
    net_flight: &Option<net::play::NetFlight>,
) -> bool {
    let flight = net_flight
        .as_ref()
        .and_then(|f| f.frame.as_ref())
        .map_or(&world.cockpits[OWN].flight, |client| &client.presented);
    input.gunsight() && flight.autopilot.mode() == tore_sim::autopilot::Mode::Orbit
}

fn mouse_control(button: MouseButton) -> Option<&'static str> {
    Some(match button {
        MouseButton::Right => "button:right",
        MouseButton::Middle => "button:middle",
        MouseButton::Back => "button:back",
        MouseButton::Forward => "button:forward",
        _ => return None,
    })
}
fn flight_key(physical: winit::keyboard::PhysicalKey, fallback: &str) -> String {
    use winit::keyboard::{KeyCode, PhysicalKey};
    if let PhysicalKey::Code(code) = physical {
        let name = format!("{code:?}");
        if let Some(letter) = name.strip_prefix("Key") {
            return letter.to_ascii_lowercase();
        }
        if let Some(digit) = name.strip_prefix("Digit") {
            return digit.into();
        }
        return match code {
            KeyCode::BracketLeft => "[",
            KeyCode::BracketRight => "]",
            KeyCode::Equal => "=",
            KeyCode::Minus => "-",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Semicolon => ";",
            KeyCode::Backslash => "\\",
            KeyCode::Quote => "'",
            KeyCode::Slash => "/",
            // FA reads scan codes without the extended bit, so the keypad is
            // the navigation cluster whatever NumLock says (docs/spec/keyboard.md).
            KeyCode::Numpad0 => "Insert",
            KeyCode::NumpadDecimal => "Delete",
            KeyCode::Numpad1 => "End",
            KeyCode::Numpad2 => "ArrowDown",
            KeyCode::Numpad3 => "PageDown",
            KeyCode::Numpad4 => "ArrowLeft",
            KeyCode::Numpad5 => "Numpad5",
            KeyCode::Numpad6 => "ArrowRight",
            KeyCode::Numpad7 => "Home",
            KeyCode::Numpad8 => "ArrowUp",
            KeyCode::Numpad9 => "PageUp",
            KeyCode::NumpadAdd => "=",
            KeyCode::NumpadSubtract => "-",
            KeyCode::NumpadEnter => "Enter",
            KeyCode::NumpadDivide => "/",
            _ => fallback,
        }
        .into();
    }
    fallback.into()
}

/// Turns the weapon page to the selected weapon.
fn show_selected_weapon_page(
    frame: &frame::FlightFrame,
    combat: &combat::Combat,
    instruments: &mut instruments::Instruments,
) {
    let readout = combat_view::readout(
        combat,
        frame.config,
        &frame.readout,
        frame.flight,
        instruments.controls(),
        instruments.rcs_scale_nmi(),
    );
    if let Some(index) = readout.weapons.iter().position(|row| row.2) {
        instruments.weapon_page = index / 6;
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    fn path(text: &str) -> Option<PathBuf> {
        Some(PathBuf::from(text))
    }

    #[test]
    fn a_bad_option_number_names_the_option_and_the_text() {
        assert_eq!(option_number::<u64>("--replay-tick", " 12 ").unwrap(), 12);
        assert_eq!(option_number::<f64>("--rate", "2.5").unwrap(), 2.5);
        let error = option_number::<u64>("--replay-tick", "-5").unwrap_err();
        assert_eq!(error.to_string(), "--replay-tick needs a number, not '-5'");
        assert!(option_number::<f64>("--rate", "abc").is_err());
    }

    #[test]
    fn a_readable_pack_starts_the_game() {
        assert_eq!(
            next_step(true, true, path("/games/fa"), path("/mnt/disc1")),
            Step::Play
        );
        assert_eq!(next_step(true, false, None, None), Step::Play);
    }

    #[test]
    fn a_known_source_is_imported_without_asking() {
        // The remembered source, or a developer checkout, needs no click.
        assert_eq!(
            next_step(false, true, path("/games/fa"), path("/mnt/disc1")),
            Step::ImportNow(PathBuf::from("/games/fa"))
        );
        assert_eq!(
            next_step(false, false, path("/games/fa"), None),
            Step::ImportNow(PathBuf::from("/games/fa"))
        );
    }

    #[test]
    fn an_unknown_source_asks_the_player_when_there_is_a_window() {
        assert_eq!(
            next_step(false, true, None, path("/mnt/disc1")),
            Step::Ask(path("/mnt/disc1"))
        );
        assert_eq!(next_step(false, true, None, None), Step::Ask(None));
        // Headless runs keep the terminal message package C wrote.
        assert_eq!(
            next_step(false, false, None, path("/mnt/disc1")),
            Step::Fail
        );
    }

    #[test]
    fn fullscreen_is_the_default_and_the_flags_win() {
        use super::{WindowMode::*, initial_window_mode};
        // An ordinary interactive start.
        assert_eq!(initial_window_mode(false, false, false, true), Fullscreen);
        // Each opt-out on its own.
        assert_eq!(initial_window_mode(true, false, false, true), Windowed);
        assert_eq!(initial_window_mode(false, true, false, true), Windowed);
        assert_eq!(initial_window_mode(false, false, true, true), Windowed);
        assert_eq!(initial_window_mode(false, false, false, false), Windowed);
        // A saved fullscreen preference never overrides a flag.
        assert_eq!(initial_window_mode(true, false, true, true), Windowed);
        // And a windowed preference is not undone by a fixed-size run.
        assert_eq!(initial_window_mode(false, false, true, false), Windowed);
        assert!(Fullscreen.fullscreen());
        assert!(!Windowed.fullscreen());
        assert!(super::fullscreen_attribute(false).is_none());
        assert!(super::fullscreen_attribute(true).is_some());
    }
    #[test]
    fn the_smoke_test_imports_the_prefilled_source() {
        let ask = Step::Ask(path("/mnt/disc1"));
        assert_eq!(auto_import_path(&ask, None, false), None);
        // Under --smoke-test the field's own path is imported, so a first run
        // can be checked end to end without clicks.
        assert_eq!(
            auto_import_path(&ask, Some(Path::new("/mnt/disc1")), true),
            path("/mnt/disc1")
        );
        assert_eq!(auto_import_path(&Step::Ask(None), None, true), None);
        // A known source starts on its own either way.
        let known = Step::ImportNow(PathBuf::from("/games/fa"));
        assert_eq!(auto_import_path(&known, None, false), path("/games/fa"));
        assert_eq!(
            auto_import_path(&known, Some(Path::new("/mnt/disc1")), true),
            path("/games/fa")
        );
    }

    #[test]
    fn named_keys_reach_the_locate_screen_and_letters_do_not() {
        use winit::keyboard::NamedKey;
        assert_eq!(locate_key_name(&Key::Named(NamedKey::Enter)), Some("Enter"));
        assert_eq!(
            locate_key_name(&Key::Named(NamedKey::Backspace)),
            Some("Backspace")
        );
        // Printable characters go through text_input, with their case intact.
        assert_eq!(locate_key_name(&Key::Character("D".into())), None);
        assert_eq!(locate_key_name(&Key::Named(NamedKey::Space)), None);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::world::airport_reply_audio;
    use winit::keyboard::{KeyCode, PhysicalKey};
    #[test]
    fn physical_keys_survive_shift_and_option_translations() {
        assert_eq!(flight_key(PhysicalKey::Code(KeyCode::Digit1), "!"), "1");
        // The keypad follows FA, whatever NumLock reports.
        for (code, logical, name) in [
            (KeyCode::Numpad0, "0", "Insert"),
            (KeyCode::NumpadDecimal, ".", "Delete"),
            (KeyCode::Numpad3, "3", "PageDown"),
            (KeyCode::Numpad5, "5", "Numpad5"),
            (KeyCode::Numpad8, "ArrowUp", "ArrowUp"),
        ] {
            assert_eq!(flight_key(PhysicalKey::Code(code), logical), name);
        }
        assert_eq!(
            flight_key(PhysicalKey::Code(KeyCode::BracketLeft), "{"),
            "["
        );
        assert_eq!(flight_key(PhysicalKey::Code(KeyCode::KeyE), "é"), "e");
        assert_eq!(flight_key(PhysicalKey::Code(KeyCode::Equal), "+"), "=");
        assert_eq!(flight_key(PhysicalKey::Code(KeyCode::Slash), "?"), "/");
    }

    #[test]
    fn airport_replies_route_verified_recordings_and_text_fallback() {
        use tore_sim::airport::{ApproachEnd, DeclineReason, Reply};
        let cleared = Reply::Cleared {
            airport: 1,
            runway: 2,
            end: ApproachEnd::Near,
        };
        assert_eq!(airport_reply_audio(&cleared), Some("^CLRLAND"));
        assert_eq!(
            airport_reply_audio(&Reply::Repeated(Box::new(cleared))),
            Some("^CLRLAND")
        );
        assert_eq!(
            airport_reply_audio(&Reply::Declined {
                airport: Some(1),
                reason: DeclineReason::RunwayDisabled,
            }),
            None
        );
        assert_eq!(
            airport_reply_audio(&Reply::Cancelled { airport: Some(1) }),
            None
        );
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use tore_sim::ai::wing::PlayerOrder;

    #[test]
    fn probe_attack_takes_a_tick_and_an_optional_repeat() {
        assert_eq!(
            ProbeScript::parse_attack("600").unwrap(),
            ProbeAttack {
                from: 600,
                repeat: 0
            }
        );
        assert_eq!(
            ProbeScript::parse_attack("600:10").unwrap(),
            ProbeAttack {
                from: 600,
                repeat: 1200
            }
        );
        assert_eq!(ProbeScript::parse_attack("0:0.5").unwrap().repeat, 60);
        for bad in ["", "x", "600:", "600:0", "600:-1", "600:3601", "-1:10"] {
            assert!(ProbeScript::parse_attack(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_probe_pilot_climbs_ten_degrees_and_eases_off_only_when_slow() {
        // Room over the ground and speed to spare: the old 10 degree hold.
        assert_eq!(climb_pitch_for(0., 3., 1.5), PROBE_CLIMB_PITCH_DEG);
        assert_eq!(climb_pitch_for(2., 5., 1.12), PROBE_CLIMB_PITCH_DEG);
        // Short of the aircraft's 1 G minimum: halfway is half the pitch,
        // and at or under the floor the climb is level.
        let halfway = (PROBE_SPEED_MARGIN + PROBE_LEVEL_SPEED_FLOOR) / 2.;
        assert!((climb_pitch_for(0., 3., halfway) - PROBE_CLIMB_PITCH_DEG / 2.).abs() < 1e-9);
        assert_eq!(climb_pitch_for(0., 3., PROBE_LEVEL_SPEED_FLOOR), 0.);
        assert_eq!(climb_pitch_for(0., 3., 0.8), 0.);
        // Rising ground asks for more, as far as 25 degrees and no further.
        assert_eq!(climb_pitch_for(8., 4., 1.5), 14.);
        assert_eq!(climb_pitch_for(40., 4., 1.5), PROBE_MAX_CLIMB_PITCH_DEG);
        // Speed still comes first when the ground asks for more.
        assert_eq!(climb_pitch_for(40., 4., PROBE_LEVEL_SPEED_FLOOR), 0.);
    }

    #[test]
    fn the_probe_roll_steers_only_when_it_has_drifted() {
        // On the line and on the heading, or within the dead bands: no rudder.
        assert_eq!(roll_rudder(0., 0., 100.), 0.);
        assert_eq!(roll_rudder(1.5, 10., 100.), 0.);
        assert_eq!(roll_rudder(-1.5, -10., 100.), 0.);
        // Nose left of the runway (error positive) turns right, and back.
        assert!(roll_rudder(10., 0., 100.) > 0.);
        assert!(roll_rudder(-10., 0., 100.) < 0.);
        // Off the line to the right with the nose straight: steer left.
        assert!(roll_rudder(0., 100., 100.) < 0.);
        assert!(roll_rudder(0., -100., 100.) > 0.);
        // The nosewheel has no authority when stopped, so the command is
        // bounded, and it saturates at full deflection.
        assert!(roll_rudder(30., 0., 0.).abs() <= 1.);
        assert_eq!(roll_rudder(30., 0., 100.), 1.);
        // Wrapped: 350 degrees of error is 10 degrees the other way.
        assert!(roll_rudder(350., 0., 100.) < 0.);
    }

    #[test]
    fn probe_wing_orders_include_the_attack_orders() {
        assert_eq!(
            ProbeScript::parse_order("600:attack-on-contact").unwrap(),
            (600, PlayerOrder::AttackOnContact, None)
        );
        assert_eq!(
            ProbeScript::parse_order("601:engage-my-target").unwrap(),
            (601, PlayerOrder::EngageMyTarget, None)
        );
        assert_eq!(
            ProbeScript::parse_order("9000:bug-out").unwrap(),
            (9000, PlayerOrder::BugOut, None)
        );
        assert_eq!(
            ProbeScript::parse_order("602:sort").unwrap(),
            (602, PlayerOrder::Sort, None)
        );
        assert_eq!(
            ProbeScript::parse_order("602:sort@2").unwrap(),
            (602, PlayerOrder::Sort, Some(2))
        );
        assert!(ProbeScript::parse_order("600:attack").is_err());
    }

    #[test]
    fn probe_wing_orders_can_name_one_member() {
        assert_eq!(
            ProbeScript::parse_order("600:engage-my-target@2").unwrap(),
            (600, PlayerOrder::EngageMyTarget, Some(2))
        );
        assert_eq!(
            ProbeScript::parse_order("1:bug-out@1").unwrap(),
            (1, PlayerOrder::BugOut, Some(1))
        );
        for bad in [
            "600:bug-out@0",
            "600:bug-out@5",
            "600:bug-out@x",
            "600:bug-out@",
        ] {
            assert!(ProbeScript::parse_order(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn probe_player_locks_name_a_tick_and_an_aircraft() {
        assert_eq!(ProbeScript::parse_player_lock("300:4").unwrap(), (300, 4));
        for bad in ["300", "x:4", "300:x", "300:-1", ":4"] {
            assert!(ProbeScript::parse_player_lock(bad).is_err(), "{bad}");
        }
    }
}

#[cfg(test)]
mod encounter_probe_tests {
    use super::*;

    #[test]
    fn every_matrix_recording_has_a_unique_exact_identity() {
        use tore_formats::aircraft::AircraftId;
        let mut names = std::collections::BTreeSet::new();
        for player in [AircraftId::F22, AircraftId::F22n, AircraftId::Faxx] {
            for enemy in AircraftId::SELECTABLE {
                for skill in 0..4 {
                    for geometry in [
                        ProbeGeometry::Head,
                        ProbeGeometry::Side,
                        ProbeGeometry::Rear,
                    ] {
                        for researched in [false, true] {
                            let name = probe_case_name(player, enemy, skill, geometry, researched);
                            assert!(names.insert(name));
                        }
                    }
                }
            }
        }
        assert_eq!(names.len(), 3 * AircraftId::SELECTABLE.len() * 4 * 3 * 2);
        assert!(names.contains("FAXX-SU27-skill2-Rear-researched"));
        assert!(names.contains("F22N-SU27-skill2-Rear-researched"));
    }
}
