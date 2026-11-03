//! Desktop input and the imported in-flight menu. Unported commands stay explicit.
use crate::hud::Paint;
use crate::menu::Canvas;
use crate::pause_menu::{self, Event, Look, PauseMenu};
use std::time::{Duration, Instant};
use tore_formats::text::GlyphCodes;
use tore_formats::{font::Font, ui::MenuNode};
use tore_input::Switch;
use tore_sim::cheats::Damage;
#[derive(Debug, PartialEq)]
pub enum Command {
    Wing(tore_sim::ai::wing::PlayerOrder),
    /// Alt+T: the next formation after the one the wing flies.
    WingFormationCycle,
    WingRecipient(Option<u8>),
    Airport(tore_sim::airport::Command),
    AirportNav,
    None,
    NextWeapon,
    PreviousWeapon,
    /// T: next radar target.
    Target,
    /// Shift-T: previous radar target.
    TargetPrevious,
    /// Enter: the visible sensor contact nearest the nose.
    TargetVisual,
    TargetCentre,
    RangeReset,
    /// Backslash: designate whatever is under the AC-130 gunsight's
    /// crosshair, an object or else the ground.
    SightDesignate,
    /// Shift-Backslash: pin the ground point under the crosshair.
    SightPinGround,
    /// Shift-' (in) and Shift-; (out): the gunsight's zoom, by steps.
    SightZoom(i8),
    DamageReport,
    Eject,
    Combat(tore_sim::combat::live::Command),
    /// Release one chaff cartridge.
    Chaff,
    /// Release one flare.
    Flare,
    Click,
    End,
    Exit,
    Restart,
    Toggle(Switch),
    View(u8),
    ViewRelative(u8, crate::flight_views::Reference),
    StoreView,
    CenterLook,
    Panel(u8),
    WindowLayout,
    /// FA keys 1 to 6: a throttle setting, with the afterburner on (6) or off.
    ThrottlePreset(f64, bool),
    /// FA keys 7 and 8: move the throttle down or up by a fraction.
    ThrottleStep(f64),
    /// W / Shift-W: the next or previous NAV destination.
    Waypoint(bool),
    Range(i32),
    /// Cycle the available sensor channels.
    Mode,
    /// Toggle the scope contact history trail.
    SensorHistory,
    /// Request the passive infrared channel.
    SensorInfrared,
    ControlsOpen,
    FeedbackOpen,
    RespawnAircraft,
    RespawnLoadout,
    /// Pref > Sound...: the Sound/Music Prefs screen over the paused flight.
    SoundOpen,
    /// Pref > Graphics...: the Graphics options screen over the paused flight.
    GraphicsOpen,
    InstrumentSelect(usize),
    InstrumentCycle(i32),
    InstrumentControl(usize),
    /// Alt-S: toggle radio silence.
    RadioSilence,
    /// Alt+N: monitor the battle net, or stop (stage G).
    BattleNet,
    /// Ctrl+9: open or close the AC-130's sensor console (orbit hold).
    SensorConsole,
    /// P: pin the AC-130 gunsight's point or let it go (in the console,
    /// ORBIT or SEARCH).
    SightPinToggle,
    /// Shift+P: lock the AC-130's guns on what the sight holds and let the
    /// sight slew away, or let them follow the sight again.
    GunLock,
    /// Ctrl+V: the Valkyries situation score.
    Valkyries,
    /// Ctrl+B: mark this moment in the mission recording.
    Bookmark,
    /// U: squawk IFF on the displayed target (retail's key).
    Iff,
    /// K: the score board on or off (a networked flight's).
    ScoreBoard,
    /// Alt+Shift+E, W, B or H: a reply or request to the player's flight.
    Reply(Reply),
}
/// The four replies and requests a wingman can make to its flight (the
/// guide's reply keys, John 2026-10-05): Alt+Shift+E, W, B and H.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// "Engaging".
    Engaging,
    /// "Winchester": out of missiles (text only, no recording says it).
    Winchester,
    /// "Bingo fuel".
    BingoFuel,
    /// "Need help".
    NeedHelp,
}
impl Reply {
    /// The reply the Alt+Shift+`key` chord makes.
    pub fn of_key(key: &str) -> Option<Self> {
        Some(match key {
            "e" => Self::Engaging,
            "w" => Self::Winchester,
            "b" => Self::BingoFuel,
            "h" => Self::NeedHelp,
            _ => return None,
        })
    }
    /// The mission core's reply, which the seat command carries (slice F2-R).
    pub fn world(self) -> tore_world::world::replies::Reply {
        use tore_world::world::replies::Reply as World;
        match self {
            Self::Engaging => World::Engaging,
            Self::Winchester => World::Winchester,
            Self::BingoFuel => World::BingoFuel,
            Self::NeedHelp => World::NeedHelp,
        }
    }
}
/// What K says until the scoring slice draws the board: in single player there
/// is no score board, and in a networked flight it is on its way.
pub fn score_board_answer(session: bool) -> &'static str {
    if session {
        "Score board: not available yet"
    } else {
        "Score board: network games only"
    }
}
/// What a reply key says in single player: the plane leads its flight and has
/// no one to answer (John, 2026-10-05). In a networked flight the world
/// answers, with this line for a lead and the call to the flight for a
/// wingman (slice F2-R).
pub const SINGLE_PLAYER_REPLY: &str = "You lead this flight.";
/// The flight menu's bottom buttons, left to right.
const BUTTONS: [&str; 3] = ["Resume flight", "Restart free flight", "Keyboard shortcuts"];
/// How flight presents the shared paused menu.
const LOOK: Look = Look {
    title: "GAME PAUSED",
    buttons: &BUTTONS,
    help: flight_help,
};
/// A session's bottom buttons: nothing restarts, and the mission ends here.
const SESSION_BUTTONS: [&str; 3] = ["Resume flight", "End mission", "Keyboard shortcuts"];
/// A session's menu is not a pause: the flight goes on behind it, so it says
/// so (agent decision).
const SESSION_LOOK: Look = Look {
    title: "FLIGHT MENU - THE MISSION GOES ON",
    buttons: &SESSION_BUTTONS,
    help: flight_help,
};
/// The flight keyboard help: the stock keys, then every shortcut in the
/// imported menu.
fn flight_help(tree: &[MenuNode]) -> Vec<String> {
    let mut lines: Vec<String> = vec![
        "DESKTOP KEYBOARD COMMANDS - Esc returns".into(),
        "Arrows: pitch/bank | End/PgDn or Z/X: rudder | 7/8: throttle -/+5%".into(),
        "1..5: idle/25/50/75/100% | 6: afterburner | Shift-B: burner".into(),
        "G: gear | F: flaps | B: brake | H: hook | O: bays | E: engine".into(),
        "Shift-E twice within 2 seconds: eject (release between presses)".into(),
        "F1 front / F2 back / F3 up / F4 track / F5 inbound missile".into(),
        "F6 wing / F7 player-target / F8 target-player / F9 fly-by".into(),
        "F10 external / F12 missile-target / V save Other View".into(),
        "Alt+view target / Ctrl+view last missile (Alt-F4 exits)".into(),
        "Shift-arrows look/orbit / Shift-/ or keypad 5 center".into(),
        "Shift-M: map | M: sensor channel | Shift-U: HUD".into(),
        "Ctrl-Tab/Ctrl-Shift-Tab: instrument | Ctrl-1..6: slot".into(),
        "Ctrl-Shift-1..4: stock instrument buttons (T.O.R.E)".into(),
        "T/Shift-T: radar target | Enter/apostrophe: visual target | Space: fire".into(),
        "A: heading/altitude | Ctrl-A: waypoint | Ctrl-Alt-A: hover hold".into(),
        "I: infrared | R: radar | Y: contact history | J: own ECM".into(),
        "N: NAV/ILS mode | W/Shift-W: next/previous waypoint".into(),
        "Insert: chaff | Delete: flare (keypad 0 and . also work)".into(),
        "Click a contact to designate it; ; or L clears the designation".into(),
        "D damage report | Range: Ctrl-Shift-I incoming | Shift-Y target ECM".into(),
        "[ / ] NAV/weapons | Shift-K tanks / Ctrl-Shift-K selected store | Tab: fire the gun (hold)".into(),
        "Ctrl-R or Shift-R: ground radar | Ctrl-M: delivery mode".into(),
        "Sight: \\ designate | Shift-\\ pin | / next | Alt-arrows slew".into(),
        "Shift-' / Shift-; sight zoom | Ctrl-L: laser | Ctrl-X: nearest ground".into(),
        "Tower: Shift-N airport | Shift-L landing | Ctrl-Shift-R/C repeat/cancel".into(),
        "Wing: Alt-1 straight, 2-5 break, 6-9 approach, E/R engage, B bug out".into(),
        "Wing (ground): Alt-F IR/laser target | Alt-W target class | Alt-G lase".into(),
        "Alt-T formation | Alt-H/V spacing/stack | Alt-0/Alt-Shift-1..4 address".into(),
        "U: IFF | Ctrl-T: target info | K: scores | Alt-Shift-E/W/B/H: replies".into(),
        "Ctrl-B: mark a replay moment | AC-130: Ctrl-7 next gun, Ctrl-8 link".into(),
        "Pad: hold Select, RB fire / LB weapon / A target / B clear".into(),
        "Select+X previous weapon / Y ECM / L3 radar / R3 jettison".into(),
        "Select+Dpad: up target / down hit / left chaff / right flare".into(),
        "Select+Start target ECM / Guide incoming; F10 external".into(),
        "AC-130 pad: Select+A designate or pin, right stick slews".into(),
        "Right-drag: mouse look | Esc > Control: remap any key".into(),
        "Stock keys shown here; docs/CONTROLS.md lists them all".into(),
        "SOURCE MENU SHORTCUTS:".into(),
    ];
    pause_menu::shortcut_lines(tree, &mut lines);
    lines
}
/// Flight messages: HUD-colored text in the HUD's font and size, with no
/// background, centered at the bottom of the view, newest at the bottom, each
/// shown for five seconds, at most seven lines. Opinionated, requested by John
/// on 2026-09-26 as the retail look; the original's exact timing, line count
/// and size are untraced.
const NOTICE_LIFETIME: Duration = Duration::from_secs(5);
const NOTICE_LINES: usize = 7;
/// The gap between the newest line's text and the bottom of the window, in
/// 640x480 layer units.
const NOTICE_MARGIN: f64 = 5.;
/// What became of one cockpit message line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A new line at the bottom of the messages.
    Now,
    /// The same text was still on screen: that line moved to the bottom with
    /// a fresh timer.
    Repeated,
    /// An earlier line that newer lines pushed off the top before its five
    /// seconds were up.
    PushedOff,
}
/// Cockpit message lines kept for a mission recording. Nothing in flight
/// reads them; the recorder drains them each tick.
const MAX_NOTES: usize = 256;
/// Authored Pref row for the weapon diagnostic panel. It is not in the
/// retail menu; opinionated, requested by John on 2026-09-23.
pub const WEAPON_DIAGNOSTICS: &str = "Weapon diagnostics?";
/// Authored Pref row for the debug panels in flight: the mission timer, the
/// right-click menu and the AI thinking, Telemetry, Guidance and Comms
/// panels. Not in the retail menu; opinionated, requested by John on
/// 2026-09-26 (the label is an agent choice).
pub const DEBUG_PANELS: &str = "Debug panels?";
/// The authored Pref row choosing the powered-lift aircraft's stability
/// level (VTOL overhaul, design 5.2 and 5.7); retail has no such row.
pub const STABILITY_LEVEL: &str = "Stability level";
/// The Debug menu's labels (slice S7b, John 2026-10-19): two AI test
/// settings for trying how much CPU the AI's observation takes and how the AI
/// fights without some of it. Defaults are the game as it plays.
pub const DEBUG_TAB: &str = "Debug";
pub const AI_OBSERVATION_RATE: &str = "AI observation rate";
pub const LEAN_AI_SENSORS: &str = "Lean AI sensors";
pub const FRAME_LOG: &str = "Slow-frame log";
pub const DEBUG_DEFAULTS: &str = "Back to defaults";
/// What a joined game's Debug rows say: the host's settings govern the AI.
pub const AI_TEST_LOCKED: &str = "The host's AI test settings apply in a joined game";
/// The Debug rows, in order.
const DEBUG_ROWS: [&str; 4] = [
    AI_OBSERVATION_RATE,
    LEAN_AI_SENSORS,
    DEBUG_DEFAULTS,
    FRAME_LOG,
];
/// The rows a joined game dims.
const AI_TEST_ROWS: [&str; 3] = [AI_OBSERVATION_RATE, LEAN_AI_SENSORS, DEBUG_DEFAULTS];
/// The name an observation rate is shown by.
pub fn observation_rate_name(hz: u32) -> &'static str {
    match hz {
        60 => "60 Hz",
        40 => "40 Hz",
        30 => "30 Hz",
        20 => "20 Hz",
        _ => "120 Hz",
    }
}
/// Appends the Debug tab to the flight menu tree: the AI test settings, off
/// by default. The retail tabs and their order are unchanged.
pub fn add_debug_tab(tree: &mut Vec<MenuNode>) {
    if tree.iter().any(|node| node.label == DEBUG_TAB) {
        return;
    }
    tree.push(MenuNode {
        label: DEBUG_TAB.into(),
        shortcut: String::new(),
        children: DEBUG_ROWS
            .iter()
            .map(|label| MenuNode {
                label: (*label).into(),
                shortcut: String::new(),
                children: vec![],
            })
            .collect(),
    });
}
/// The name a stability level is shown and announced by.
pub fn stability_name(level: tore_input::StabilityLevel) -> &'static str {
    match level {
        tore_input::StabilityLevel::Off => "Off",
        tore_input::StabilityLevel::Damper => "Damper",
        tore_input::StabilityLevel::Attitude => "Attitude",
    }
}
/// Whether a Pref row is retail's Show Target Info (Ctrl+T).
pub fn is_target_info(label: &str) -> bool {
    label
        .trim()
        .trim_end_matches('?')
        .eq_ignore_ascii_case("show target info")
}
/// The authored Cheat row that removes the powered-lift aircraft's flight
/// hazards (VTOL overhaul, design 4.12). Retail has no such row; opinionated,
/// requested by John on 2026-10-08 (the label is the design's).
pub const EASY_PHYSICS: &str = "Easy flight physics?";
/// Append the authored display and lighting rows to Pref and the
/// Easy flight physics row after the imported Cheat rows. The retail rows
/// and their order are unchanged.
pub fn add_authored_rows(tree: &mut [MenuNode]) {
    let authored = [
        (
            "Pref",
            &[
                WEAPON_DIAGNOSTICS,
                DEBUG_PANELS,
                STABILITY_LEVEL,
                "HUD colour",
            ][..],
        ),
        ("Cheat", &[EASY_PHYSICS][..]),
    ];
    for (menu, labels) in authored {
        let Some(menu) = tree.iter_mut().find(|node| node.label == menu) else {
            continue;
        };
        for label in labels {
            if !menu.children.iter().any(|row| row.label == *label) {
                menu.children.push(MenuNode {
                    label: (*label).into(),
                    shortcut: String::new(),
                    children: vec![],
                });
            }
        }
    }
    // Keep Pref above its bottom buttons. Level controls have their own page.
    if let Some(pref) = tree.iter_mut().find(|node| node.label == "Pref")
        && !pref
            .children
            .iter()
            .any(|node| node.label == "Cockpit lighting")
    {
        pref.children.push(MenuNode {
            label: "Cockpit lighting".into(),
            shortcut: String::new(),
            children: [
                "Dim instruments",
                "Brighten instruments",
                "Dim flood",
                "Brighten flood",
            ]
            .into_iter()
            .map(|label| MenuNode {
                label: label.into(),
                shortcut: String::new(),
                children: vec![],
            })
            .collect(),
        });
    }
}
pub struct FlightUi {
    pub menu: bool,
    pub map: crate::flight_map::Map,
    pub paused: bool,
    pub cockpit: bool,
    pub hud: bool,
    pub ladder: bool,
    /// The upper-right weapon diagnostic panel; hidden unless chosen in Pref.
    pub weapon_diagnostics: bool,
    /// The mission timer, right-click menu and debug panels; off unless
    /// chosen in Pref.
    pub debug_panels: bool,
    /// Show Target Info: identities under every visible aircraft and object,
    /// from Pref or Ctrl+T. Off by default, as in retail.
    pub target_info: bool,
    /// Session-only cheats; they survive Restart but are not saved.
    pub cheats: tore_sim::cheats::Cheats,
    /// The stability level the powered-lift aircraft fly at, a saved
    /// preference: Pref's row or Ctrl+Shift+A changes it, and the flight
    /// follows it (VTOL overhaul, design 5.2).
    pub stability: tore_input::StabilityLevel,
    pub brightness: i16,
    pub lighting: crate::cockpit_lighting::Lighting,
    lighting_chord: Option<(bool, Instant)>,
    pub zoom: f32,
    pub look: [f32; 2],
    pub time_scale: f64,
    /// Message lines shown at the bottom of the flight view, oldest first.
    pub notices: std::collections::VecDeque<(String, Instant)>,
    /// Every message line and what became of it, for the recorder.
    notes: std::collections::VecDeque<(String, Shown)>,
    /// Where the Escape menu is: tab, submenu, focus and help pages.
    pub pause: PauseMenu,
    /// The HUD bank scale's gyro; a new flight starts it at the aircraft's bank.
    pub bank_gyro: crate::hud::BankGyro,
    /// Whether this flight is a session on a server. The host's clock never
    /// stops, so there is no pause or time compression, the Restart key is
    /// refused, and the Cheat menu keeps only what changes the screen alone
    /// (John, 2026-09-30). See [`session_menu`].
    pub session: bool,
    /// The AI test settings the Debug tab edits: how often AI aircraft
    /// observe, and whether their sensors are lean. They apply to the
    /// simulation this game runs (single player, or the game it hosts) at
    /// the next tick; a flight keeps them across a restart, and nothing
    /// saves them.
    pub ai_test: tore_sim::ai::test_settings::AiTestSettings,
    /// The Debug rows are locked: a joined game has the host's settings.
    pub ai_test_locked: bool,
}
impl Default for FlightUi {
    fn default() -> Self {
        Self {
            menu: false,
            map: Default::default(),
            paused: false,
            cockpit: true,
            hud: true,
            ladder: true,
            weapon_diagnostics: false,
            debug_panels: false,
            stability: tore_input::StabilityLevel::Damper,
            target_info: false,
            cheats: Default::default(),
            brightness: 0,
            lighting: Default::default(),
            lighting_chord: None,
            zoom: 1.,
            look: [0.; 2],
            time_scale: 1.,
            notices: Default::default(),
            notes: Default::default(),
            pause: PauseMenu::default(),
            bank_gyro: Default::default(),
            session: false,
            ai_test: Default::default(),
            ai_test_locked: false,
        }
    }
}
/// Whether a click on this row turns a value, so that a right click turns it
/// back: the three rows with several values (Stability level, HUD colour, AI
/// observation rate) and the two-state switches, which a right click flips as
/// a left click does. The rows that choose one of a set (Damage, Enemy AI,
/// Time) and the commands are not among them.
fn turns_back(label: &str) -> bool {
    matches!(
        label,
        STABILITY_LEVEL
            | "HUD colour"
            | AI_OBSERVATION_RATE
            | LEAN_AI_SENSORS
            | FRAME_LOG
            | WEAPON_DIAGNOSTICS
            | DEBUG_PANELS
            | "HUD pitch ladder?"
            | "Show cockpit?"
            | "Large windows?"
    ) || is_target_info(label)
        || cheat_switch(&mut Default::default(), label).is_some()
}
/// The Cheat rows a session keeps: the three that change nothing but the
/// player's own screen. Every other cheat changes the mission, which the
/// server alone decides (John, 2026-09-30).
pub const SESSION_CHEATS: [&str; 3] = [
    "No sun whiteout?",
    "No redout or blackout?",
    "No screen-shaking?",
];
/// What the Restart key says in a session: the mission is the server's.
pub const RESTART_REFUSED: &str = "Restart is not available in a multiplayer flight";
/// The rows that stop or change time, which a session has no use for.
const TIME_ROWS: [&str; 6] = ["Paused", "Slow-motion", "1x", "2x", "4x", "8x"];

/// Why a menu row does nothing in a session, or `None` when it works. The
/// rows are named by their imported labels.
pub fn session_refusal(label: &str) -> Option<&'static str> {
    let label = label.trim();
    if TIME_ROWS.contains(&label) {
        return Some("Time cannot be paused or sped up in a multiplayer flight");
    }
    let mission_cheat = matches!(
        label,
        "Invulnerable" | "Normal" | "Realistic" | "Novice" | "Average" | "Unchanged"
    ) || cheat_switch(&mut Default::default(), label).is_some();
    (mission_cheat && !SESSION_CHEATS.contains(&label))
        .then_some("The server sets the cheats in a multiplayer flight")
}

/// The flight menu a session shows: `tree` without the time rows, without
/// the Cheat rows that change the mission (and the submenus they leave
/// empty), and without the Pos menu that moves the aircraft.
pub fn session_menu(tree: &[MenuNode]) -> Vec<MenuNode> {
    fn without_time(node: &MenuNode) -> Option<MenuNode> {
        if TIME_ROWS.contains(&node.label.trim()) {
            return None;
        }
        if node.children.is_empty() {
            return Some(node.clone());
        }
        let children: Vec<_> = node.children.iter().filter_map(without_time).collect();
        (!children.is_empty()).then(|| MenuNode {
            children,
            ..node.clone()
        })
    }
    tree.iter()
        .filter(|node| node.label != "Pos")
        .filter_map(|node| {
            if node.label == "Cheat" {
                let children: Vec<_> = node
                    .children
                    .iter()
                    .filter(|row| SESSION_CHEATS.contains(&row.label.as_str()))
                    .cloned()
                    .collect();
                return Some(MenuNode {
                    children,
                    ..node.clone()
                });
            }
            without_time(node)
        })
        .collect()
}

/// The on/off Cheat menu rows that work, by their imported label.
fn cheat_switch<'a>(cheats: &'a mut tore_sim::cheats::Cheats, label: &str) -> Option<&'a mut bool> {
    Some(match label {
        "Unlimited ammo?" => &mut cheats.unlimited_ammo,
        "Unlimited fuel?" => &mut cheats.unlimited_fuel,
        "No spins?" => &mut cheats.no_spins,
        "No turbulence?" => &mut cheats.no_turbulence,
        "Pull extra G?" => &mut cheats.extra_g,
        "Ignore weapon weights?" => &mut cheats.ignore_weapon_weights,
        "No sun whiteout?" => &mut cheats.no_sun_whiteout,
        "No redout or blackout?" => &mut cheats.no_g_effects,
        "No crashes?" => &mut cheats.no_crashes,
        "Easy aiming?" => &mut cheats.easy_aiming,
        "Ignore midair collisions?" => &mut cheats.ignore_midair_collisions,
        "Easy targeting?" => &mut cheats.easy_targeting,
        "Air combat guns only?" => &mut cheats.guns_only,
        "No screen-shaking?" => &mut cheats.no_screen_shake,
        EASY_PHYSICS => &mut cheats.easy_physics,
        _ => return None,
    })
}
impl FlightUi {
    /// Reset transient flight UI while retaining the session-only cheats.
    /// Saved display preferences are reapplied by the caller.
    pub fn reset_for_flight(&mut self) {
        *self = Self {
            cheats: self.cheats,
            ai_test: self.ai_test,
            ..Default::default()
        };
    }

    /// Whether the weapon diagnostic panel is drawn and takes clicks: only
    /// when chosen, with the HUD shown, in the cockpit, back and up views.
    pub fn weapon_diagnostics_shown(&self, flight_view: u8) -> bool {
        self.weapon_diagnostics
            && self.hud
            && matches!(flight_view, 0 | 3 | 4 | crate::flight_views::TRACK)
    }
    /// On/Off for a working cheat row or an authored diagnostics row; the
    /// selected Damage choice reads On.
    fn cheat_state(&self, label: &str) -> Option<&'static str> {
        if label == STABILITY_LEVEL {
            return Some(stability_name(self.stability));
        }
        if label == AI_OBSERVATION_RATE {
            return Some(if self.ai_test_locked {
                "Host"
            } else {
                observation_rate_name(self.ai_test.observation_hz())
            });
        }
        if label == LEAN_AI_SENSORS {
            return Some(if self.ai_test_locked {
                "Host"
            } else if self.ai_test.lean_sensors {
                "On"
            } else {
                "Off"
            });
        }
        if label == DEBUG_DEFAULTS {
            return Some("");
        }
        let mut cheats = self.cheats;
        let on = match label {
            FRAME_LOG => crate::frame_log::enabled(),
            WEAPON_DIAGNOSTICS => self.weapon_diagnostics,
            DEBUG_PANELS => self.debug_panels,
            _ if is_target_info(label) => self.target_info,
            "Invulnerable" => cheats.damage == Damage::Invulnerable,
            "Normal" => cheats.damage == Damage::Normal,
            "Realistic" => cheats.damage == Damage::Realistic,
            "Novice" => cheats.enemy_ai == Some(tore_sim::ai::Experience::Novice),
            "Average" => cheats.enemy_ai == Some(tore_sim::ai::Experience::Average),
            "Unchanged" => cheats.enemy_ai.is_none(),
            _ => *cheat_switch(&mut cheats, label)?,
        };
        Some(if on { "On" } else { "Off" })
    }
    /// Whether a menu or the pause is up, so the flight takes no input. In a
    /// session the flight runs on behind it: see [`FlightUi::stopped`].
    pub fn frozen(&self) -> bool {
        self.menu || self.paused
    }
    /// Whether the flight's time is stopped. A pause or a menu stops it in
    /// single player; a session's never stops, whatever is open over it.
    pub fn stopped(&self) -> bool {
        self.frozen() && !self.session
    }
    /// Starts a session's flight: no pause or time compression, and only the
    /// cheats that change the screen alone survive from earlier flights.
    pub fn enter_session(&mut self) {
        self.session = true;
        self.paused = false;
        self.time_scale = 1.;
        let kept = self.cheats;
        self.cheats = tore_sim::cheats::Cheats {
            no_sun_whiteout: kept.no_sun_whiteout,
            no_g_effects: kept.no_g_effects,
            no_screen_shake: kept.no_screen_shake,
            ..Default::default()
        };
    }
    /// Pauses the flight because the window lost the player's attention. A
    /// session cannot pause; its controls go neutral instead (the input
    /// context does that).
    pub fn pause_for_focus(&mut self) {
        if !self.session {
            self.paused = true;
        }
    }
    pub fn steps(&self, clock: &mut crate::flight::Clock, elapsed: f64) -> usize {
        if self.stopped() {
            0
        } else if self.time_scale == 1. {
            clock.steps(elapsed)
        } else {
            clock.steps_scaled(elapsed, self.time_scale)
        }
    }
    /// Match F10 once on the pilot-death transition, without pausing the wreck.
    pub fn pilot_death_view(&mut self, was_dead: bool, dead: bool) -> Option<u8> {
        if !was_dead && dead {
            self.map.open = false;
            self.look = [0.; 2];
            self.zoom = 1.;
            Some(1)
        } else {
            None
        }
    }
    pub fn cancel_press(&mut self) {
        self.map.cancel_press();
        self.pause.cancel_press();
    }
    /// A joined game's Debug rows are locked and drawn dim, with the host's
    /// settings governing the AI; anywhere else they edit this game's.
    pub fn lock_ai_test(&mut self, locked: bool) {
        if self.ai_test_locked == locked {
            return;
        }
        self.ai_test_locked = locked;
        self.pause
            .dim
            .retain(|label| !AI_TEST_ROWS.contains(&label.as_str()));
        if locked {
            self.pause
                .dim
                .extend(AI_TEST_ROWS.iter().map(|label| (*label).to_owned()));
        }
    }
    pub fn message(&mut self, text: impl Into<String>) {
        let text = text.into();
        let shown = if self
            .notices
            .iter()
            .any(|(shown, at)| *shown == text && at.elapsed() < NOTICE_LIFETIME)
        {
            Shown::Repeated
        } else {
            Shown::Now
        };
        // A repeated line moves to the bottom with a fresh timer.
        self.notices.retain(|(shown, _)| *shown != text);
        self.notices.push_back((text.clone(), Instant::now()));
        self.note(text, shown);
        while self.notices.len() > NOTICE_LINES {
            if let Some((old, at)) = self.notices.pop_front()
                && at.elapsed() < NOTICE_LIFETIME
            {
                self.note(old, Shown::PushedOff);
            }
        }
    }
    fn note(&mut self, text: String, shown: Shown) {
        if self.notes.len() == MAX_NOTES {
            self.notes.pop_front();
        }
        self.notes.push_back((text, shown));
    }
    /// Message lines since the last call, oldest first, each with what
    /// became of it. For mission recordings; flight never reads them.
    pub fn take_notes(&mut self) -> Vec<(String, Shown)> {
        self.notes.drain(..).collect()
    }
    fn unavailable(&mut self, label: &str) -> Command {
        self.message(format!("{label}: not implemented yet"));
        Command::Click
    }
    pub fn activate(&mut self, label: &str, shortcut: &str) -> Command {
        if let Some(view) = crate::flight_views::key(shortcut) {
            return Command::View(view);
        }
        if self.session
            && let Some(refusal) = session_refusal(label)
        {
            self.message(refusal);
            return Command::Click;
        }
        match label {
            "Restart free flight" if self.session => {
                self.message(RESTART_REFUSED);
                Command::Click
            }
            "Resume flight" => {
                self.menu = false;
                self.paused = false;
                Command::Click
            }
            "Restart free flight" => Command::Restart,
            "Keyboard" | "Controls..." | "Controls" => Command::ControlsOpen,
            "End mission" => Command::End,
            "Exit to Windows" => Command::Exit,
            "Keyboard shortcuts" => {
                self.pause.open_help();
                Command::Click
            }
            "Paused" => {
                self.paused = !self.frozen();
                self.menu = false;
                Command::Click
            }
            "Large windows?" => Command::WindowLayout,
            "Show cockpit?" => {
                self.cockpit = !self.cockpit;
                Command::Click
            }
            "No turbulence?" => {
                self.cheats.no_turbulence = !self.cheats.no_turbulence;
                self.message(if self.cheats.no_turbulence {
                    "Turbulence: off"
                } else {
                    "Turbulence: on"
                });
                Command::Click
            }
            "No sun whiteout?" => {
                self.cheats.no_sun_whiteout = !self.cheats.no_sun_whiteout;
                self.message(if self.cheats.no_sun_whiteout {
                    "Sun glare: off"
                } else {
                    "Sun glare: on"
                });
                Command::Click
            }
            "Novice" | "Average" | "Unchanged" => {
                use tore_sim::ai::Experience;
                self.cheats.enemy_ai = match label {
                    "Novice" => Some(Experience::Novice),
                    "Average" => Some(Experience::Average),
                    _ => None,
                };
                self.message(format!("Enemy AI: {}", label.to_lowercase()));
                Command::Click
            }
            "Invulnerable" | "Normal" | "Realistic" => {
                self.cheats.damage = match label {
                    "Invulnerable" => Damage::Invulnerable,
                    "Realistic" => Damage::Realistic,
                    _ => Damage::Normal,
                };
                self.message(format!("Damage: {}", label.to_lowercase()));
                Command::Click
            }
            _ if cheat_switch(&mut self.cheats, label).is_some() => {
                let on = cheat_switch(&mut self.cheats, label).is_some_and(|on| {
                    *on = !*on;
                    *on
                });
                let name = label.trim_end_matches('?');
                self.message(format!("{name}: {}", if on { "on" } else { "off" }));
                Command::Click
            }
            "HUD pitch ladder?" => {
                self.ladder = !self.ladder;
                Command::Click
            }
            WEAPON_DIAGNOSTICS => {
                self.weapon_diagnostics = !self.weapon_diagnostics;
                self.message(if self.weapon_diagnostics {
                    "Weapon diagnostics: on"
                } else {
                    "Weapon diagnostics: off"
                });
                Command::Click
            }
            _ if is_target_info(label) => {
                self.target_info = !self.target_info;
                self.message(if self.target_info {
                    "Show target info: on"
                } else {
                    "Show target info: off"
                });
                Command::Click
            }
            STABILITY_LEVEL => {
                self.stability = self.stability.next();
                self.message(format!("Stability: {}", stability_name(self.stability)));
                Command::Click
            }
            DEBUG_PANELS => {
                self.debug_panels = !self.debug_panels;
                self.message(if self.debug_panels {
                    "Debug panels: on (right-click an aircraft)"
                } else {
                    "Debug panels: off"
                });
                Command::Click
            }
            "HUD colour" => {
                self.lighting.colour = (self.lighting.colour + 1) % 4;
                self.message(format!(
                    "HUD {}",
                    crate::cockpit_lighting::Lighting::COLOURS[usize::from(self.lighting.colour)]
                ));
                Command::Click
            }
            "Dim instruments" | "Brighten instruments" | "Dim flood" | "Brighten flood" => {
                self.adjust_lighting(
                    label.ends_with("flood"),
                    if label.starts_with("Dim") { -1 } else { 1 },
                );
                Command::Click
            }
            "Dim HUD" => {
                self.brightness = (self.brightness - 16).max(-256);
                self.message(format!("HUD {}/33", (self.brightness + 256) / 16 + 1));
                Command::Click
            }
            "Brighten HUD" => {
                self.brightness = (self.brightness + 16).min(256);
                self.message(format!("HUD {}/33", (self.brightness + 256) / 16 + 1));
                Command::Click
            }
            FRAME_LOG => {
                crate::frame_log::toggle();
                self.message(if crate::frame_log::enabled() {
                    "Slow-frame log: on (50 ms), send this session's log"
                } else {
                    "Slow-frame log: off"
                });
                Command::Click
            }
            AI_OBSERVATION_RATE | LEAN_AI_SENSORS | DEBUG_DEFAULTS => {
                if self.ai_test_locked {
                    self.message(AI_TEST_LOCKED);
                    return Command::Click;
                }
                self.ai_test = match label {
                    AI_OBSERVATION_RATE => self.ai_test.next_rate(),
                    LEAN_AI_SENSORS => self.ai_test.with_lean_sensors(!self.ai_test.lean_sensors),
                    _ => Default::default(),
                };
                self.message(format!("AI test settings: {}", self.ai_test.describe()));
                Command::Click
            }
            "Feedback..." => Command::FeedbackOpen,
            crate::net::respawn::AIRCRAFT => Command::RespawnAircraft,
            crate::net::respawn::LOADOUT => Command::RespawnLoadout,
            "Sound..." => Command::SoundOpen,
            "Graphics..." => Command::GraphicsOpen,
            "1x" | "2x" | "4x" | "8x" | "Slow-motion" => {
                self.time_scale = match label {
                    "2x" => 2.,
                    "4x" => 4.,
                    "8x" => 8.,
                    "Slow-motion" => 0.5,
                    _ => 1.,
                };
                self.message(format!("Time {}x", self.time_scale));
                Command::Click
            }
            "Current" => Command::Panel(1),
            _ => match shortcut {
                "A" | "a" => Command::Toggle(Switch::Autopilot),
                "Ctrl-A" | "Ctrl-a" => Command::Toggle(Switch::WaypointAutopilot),
                "Shift-0" => Command::Panel(0),
                s if s.starts_with("Shift-")
                    && s.len() == 7
                    && s.as_bytes()[6].is_ascii_digit() =>
                {
                    Command::Panel(s.as_bytes()[6] - b'0')
                }
                _ => self.unavailable(label),
            },
        }
    }
    fn select(&mut self, tree: &[MenuNode], index: usize) -> Command {
        let Some(node) = self.pause.focus_row(tree, index) else {
            return Command::None;
        };
        if node.label == "Controls" || node.label == "Controls..." {
            return Command::ControlsOpen;
        }
        if !node.children.is_empty() {
            self.pause.enter(index);
            Command::Click
        } else {
            self.activate(&node.label, &node.shortcut)
        }
    }
    /// The menu is showing tab `index`: the Control tab opens the controls
    /// screen.
    fn switched(tree: &[MenuNode], index: usize) -> Command {
        if tree[index].label == "Control" {
            Command::ControlsOpen
        } else {
            Command::None
        }
    }
    fn adjust_lighting(&mut self, flood: bool, delta: i8) {
        self.lighting.step(flood, delta);
        self.message(if flood {
            format!("FLOOD {}/5", self.lighting.flood)
        } else {
            format!("INSTRUMENTS {}/5", self.lighting.instruments)
        });
    }
    /// Called before ordinary bindings so the chord's +/- cannot also zoom.
    pub fn lighting_key(&mut self, key: &str, alt: bool) -> bool {
        let Some((flood, at)) = self.lighting_chord else {
            return false;
        };
        if self.menu
            || self.map.open
            || alt
            || at.elapsed().as_secs_f32() > 4.
            || !matches!(key, "-" | "_" | "+" | "=")
        {
            self.lighting_chord = None;
            return false;
        }
        self.adjust_lighting(flood, if matches!(key, "-" | "_") { -1 } else { 1 });
        self.lighting_chord = Some((flood, Instant::now()));
        true
    }
    pub fn key(
        &mut self,
        key: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        tree: &[MenuNode],
    ) -> Command {
        if !self.menu && !ctrl && !alt && shift && key == "m" {
            self.map.open = !self.map.open;
            return Command::Click;
        }
        if self.map.open && !self.menu {
            if key == "Escape" {
                self.map.open = false;
                return Command::Click;
            }
            if !ctrl
                && !alt
                && matches!(
                    key,
                    "+" | "="
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
                )
            {
                self.map.key(key);
                return Command::None;
            }
        }
        if key == "Escape" {
            if !self.pause.back() {
                self.menu = !self.menu;
            }
            return Command::None;
        }
        if ctrl && !shift && !alt && key == "p" {
            return self.activate("Paused", "");
        }
        if ctrl && !shift && !alt && key == "q" {
            return self.activate("End mission", "");
        }
        if self.menu {
            return match self.pause.key(key, tree) {
                Event::Select(index) => self.select(tree, index),
                Event::Switched(index) => Self::switched(tree, index),
                _ => Command::None,
            };
        }
        if ctrl
            && !alt
            && ((key.eq_ignore_ascii_case("i") && !shift)
                || (key.eq_ignore_ascii_case("f") && shift))
        {
            let flood = key.eq_ignore_ascii_case("f");
            self.lighting_chord = Some((flood, Instant::now()));
            self.message(if flood {
                "FLOOD: -/+ TO ADJUST"
            } else {
                "INSTRUMENTS: -/+ TO ADJUST"
            });
            return Command::Click;
        }
        if ctrl && !shift && !alt && key.eq_ignore_ascii_case("h") {
            return self.activate("HUD colour", "");
        }
        if alt && !ctrl && !shift && key == "F4" {
            return Command::Exit;
        }
        if !(shift || ctrl && alt)
            && let Some(view) = crate::flight_views::key(key)
        {
            return if alt {
                Command::ViewRelative(view, crate::flight_views::Reference::Target)
            } else if ctrl {
                Command::ViewRelative(view, crate::flight_views::Reference::Missile)
            } else {
                Command::View(view)
            };
        }
        // Addressing one wingman is a T.O.R.E addition: FA sends every order
        // to the whole flight and leaves Alt+Shift free.
        if alt
            && !ctrl
            && shift
            && let Ok(n @ 1..=4) = key.parse::<u8>()
        {
            return Command::WingRecipient(Some(n));
        }
        // The wingman's replies and requests to its flight sit on Alt+Shift
        // with a letter; the Alt letters are the lead's orders and stay put.
        if alt
            && !ctrl
            && shift
            && let Some(reply) = Reply::of_key(key)
        {
            return Command::Reply(reply);
        }
        if alt && !ctrl && !shift {
            use tore_sim::ai::wing::{PlayerApproach as A, PlayerBreak as B, PlayerOrder as O};
            // The FA wingman keys (docs/spec/keyboard.md). Alt+L, Alt+0 and
            // Alt+A (the sort, John 2026-10-05) are T.O.R.E additions on keys
            // FA leaves free.
            let order = match key {
                "1" => Some(O::Break(B::Straight)),
                "2" => Some(O::Break(B::Left)),
                "3" => Some(O::Break(B::Right)),
                "4" => Some(O::Break(B::Low)),
                "5" => Some(O::Break(B::High)),
                "6" => Some(O::Approach(A::Left)),
                "7" => Some(O::Approach(A::Right)),
                "8" => Some(O::Approach(A::Low)),
                "9" => Some(O::Approach(A::High)),
                "e" => Some(O::EngageMyTarget),
                "r" => Some(O::EngageFromFormation),
                "a" => Some(O::Sort),
                "w" => Some(O::EngageClass),
                "p" => Some(O::ProtectMe),
                "d" => Some(O::Disengage),
                "b" => Some(O::BugOut),
                "c" => Some(O::ControlToggle),
                "h" => Some(O::Spacing),
                "v" => Some(O::Stacking),
                "l" => Some(O::LandAtSelected),
                "t" => return Command::WingFormationCycle,
                "f" => Some(O::EngageIrLaser),
                "g" => Some(O::LaseMyTarget),
                "s" => return Command::RadioSilence,
                "n" => return Command::BattleNet,
                "0" => return Command::WingRecipient(None),
                _ => None,
            };
            if let Some(order) = order {
                return Command::Wing(order);
            }
        }
        if ctrl && !alt {
            if key == "v" && !shift {
                return Command::Valkyries;
            }
            // Range fixture, kept off the retail Shift-I airbase inventory.
            if key == "i" && shift {
                return Command::Combat(tore_sim::combat::live::Command::Incoming);
            }
            if key == "b" && !shift {
                return Command::Bookmark;
            }
            // Range fixture, off Backslash for the gunsight's designate.
            if key == "\\" && shift {
                return Command::RangeReset;
            }
            // Retail's Show Target Info key, the Pref row's accelerator.
            if key == "t" && !shift {
                return self.activate("Show target info?", "Ctrl-T");
            }
            if key == "Tab" {
                return Command::InstrumentCycle(if shift { -1 } else { 1 });
            }
            if let Ok(n) = key.parse::<usize>() {
                if shift && (1..=4).contains(&n) {
                    return Command::InstrumentControl(n - 1);
                }
                if !shift && (1..=6).contains(&n) {
                    return Command::InstrumentSelect(n - 1);
                }
            }
        }
        let shortcut = format!(
            "{}{}{}{}",
            if ctrl { "Ctrl-" } else { "" },
            if alt { "Alt-" } else { "" },
            if shift { "Shift-" } else { "" },
            if key == "Backspace" { "BS" } else { key }
        );
        fn find<'a>(tree: &'a [MenuNode], key: &str) -> Option<&'a MenuNode> {
            for node in tree {
                if !node.shortcut.is_empty() && node.shortcut.eq_ignore_ascii_case(key) {
                    return Some(node);
                }
                if let Some(node) = find(&node.children, key) {
                    return Some(node);
                }
            }
            None
        }
        // Time cycle has four menu rows with the same accelerator.
        if !ctrl && !alt && !shift && key == "c" {
            if self.session {
                self.message("Time cannot be paused or sped up in a multiplayer flight");
                return Command::Click;
            }
            self.time_scale = match self.time_scale {
                x if x < 1. => 1.,
                1. => 2.,
                2. => 4.,
                4. => 8.,
                _ => 1.,
            };
            self.message(format!("Time {}x", self.time_scale));
            return Command::Click;
        }
        if key == "a" && !shift && !alt {
            return Command::Toggle(if ctrl {
                Switch::WaypointAutopilot
            } else {
                Switch::Autopilot
            });
        }
        if let Some(node) = find(tree, &shortcut) {
            return self.activate(&node.label, &node.shortcut);
        }
        if ctrl && shift && !alt && key == "k" {
            return Command::Combat(tore_sim::combat::live::Command::Jettison);
        }
        if ctrl || alt {
            return Command::None;
        }
        if shift {
            return match key {
                "/" => Command::CenterLook,
                "b" => Command::Toggle(Switch::Burner),
                "e" => Command::Eject,
                "u" => {
                    self.hud = !self.hud;
                    Command::Click
                }
                // John, 2026-10-15: Shift-K drops tanks.
                "k" => Command::Combat(tore_sim::combat::live::Command::JettisonTanks),
                // FA keys whose features T.O.R.E does not have yet.
                "j" => self.unavailable("Jettison external fuel"),
                "i" => self.unavailable("Airbase inventory"),
                "r" => self.unavailable("Air-to-ground radar"),
                "f" => self.unavailable("Target damage"),
                "a" => self.unavailable("AWACS radar link"),
                "g" => self.unavailable("Air-to-ground radar link"),
                "d" => self.unavailable("Message history"),
                // The development fixture moved aside for the sensor keys.
                "y" => Command::Combat(tore_sim::combat::live::Command::ToggleTargetJammer),
                "t" => Command::TargetPrevious,
                "w" => Command::Waypoint(false),
                _ => Command::None,
            };
        }
        match key {
            "g" => Command::Toggle(Switch::Gear),
            "f" => Command::Toggle(Switch::Flaps),
            "b" => Command::Toggle(Switch::Airbrake),
            "h" => Command::Toggle(Switch::Hook),
            "e" => Command::Toggle(Switch::Engine),
            "r" => Command::Toggle(Switch::Radar),
            "j" => Command::Toggle(Switch::Jammer),
            "o" => Command::Toggle(Switch::Bay),
            // FA keyboard throttle: 0, 25, 50, 75 and 100 percent, then
            // afterburner; 7 and 8 step 5 percent (docs/spec/keyboard.md).
            "1" => Command::ThrottlePreset(0., false),
            "2" => Command::ThrottlePreset(0.25, false),
            "3" => Command::ThrottlePreset(0.5, false),
            "4" => Command::ThrottlePreset(0.75, false),
            "5" => Command::ThrottlePreset(1., false),
            "6" => Command::ThrottlePreset(1., true),
            "7" => Command::ThrottleStep(-0.05),
            "8" => Command::ThrottleStep(0.05),
            "=" | "+" => {
                self.zoom = (self.zoom * 1.2).min(4.);
                Command::None
            }
            "-" => {
                self.zoom = (self.zoom / 1.2).max(0.5);
                Command::None
            }
            // Manual pp. 21, 94, 97: comma increases the range, period decreases it.
            "," => Command::Range(1),
            "." => Command::Range(-1),
            "Numpad5" => Command::CenterLook,
            "F11" => {
                self.menu = true;
                self.pause.help = true;
                Command::None
            }
            "d" => Command::DamageReport,
            "y" => Command::SensorHistory,
            ";" | "l" => Command::Combat(tore_sim::combat::live::Command::ClearDesignation),
            "]" => Command::NextWeapon,
            "[" => Command::PreviousWeapon,

            "t" => Command::Target,
            "w" => Command::Waypoint(true),
            "u" => Command::Iff,
            "k" => Command::ScoreBoard,
            "i" => Command::SensorInfrared,
            "m" => Command::Mode,

            "Enter" => Command::TargetVisual,
            "'" => Command::TargetCentre,
            // FA.EXE: scan codes 0x52 and 0x53 release chaff and flares.
            "Insert" => Command::Chaff,
            "Delete" => Command::Flare,
            "Space" => Command::None,
            "v" => Command::StoreView,
            _ => Command::None,
        }
    }
    /// How the menu looks and what its bottom buttons are, in a session or not.
    fn look(&self) -> (&'static Look, &'static [&'static str; 3]) {
        if self.session {
            (&SESSION_LOOK, &SESSION_BUTTONS)
        } else {
            (&LOOK, &BUTTONS)
        }
    }
    /// The menu's controls as drawn, for tests.
    #[cfg(test)]
    fn controls(&self, tree: &[MenuNode]) -> Vec<pause_menu::Control> {
        self.pause
            .controls(tree, &BUTTONS, &|label| self.cheat_state(label))
    }
    /// The controls screen closed; the flight menu reopens at its first tab.
    pub fn controls_closed(&mut self) {
        self.pause.show_tab(0);
    }
    pub fn pointer(&mut self, tree: &[MenuNode], point: Option<(f64, f64)>, down: bool) -> Command {
        let (look, buttons) = self.look();
        match self.pause.pointer(tree, look, point, down) {
            Event::None | Event::Switched(_) => Command::None,
            Event::Click => Command::Click,
            Event::Button(index) => self.activate(buttons[index], ""),
            Event::Tab(index) => {
                if tree[index].label == "Control" {
                    return Command::ControlsOpen;
                }
                self.pause.show_tab(index);
                Command::Click
            }
            Event::Select(index) => self.select(tree, index),
        }
    }
    /// The right button on the menu: a row that takes values turns back to
    /// its previous one, as the left button turns it to the next (the
    /// Quick Mission creator's and the lobby's rule); any other row, tab or
    /// button ignores it.
    pub fn right_pointer(
        &mut self,
        tree: &[MenuNode],
        point: Option<(f64, f64)>,
        down: bool,
    ) -> Command {
        let (look, _) = self.look();
        let Some(index) = self.pause.right_pointer(tree, look, point, down) else {
            return Command::None;
        };
        let Some(node) = self.pause.rows(tree).get(index) else {
            return Command::None;
        };
        if !node.children.is_empty() || !turns_back(&node.label) {
            return Command::None;
        }
        let label = node.label.clone();
        self.pause.focus = index;
        self.activate_back(&label)
    }
    /// The previous value of a row that cycles forward on a click: Stability
    /// level, HUD colour and the AI observation rate step back, wrapping; a
    /// two-state row flips. Other rows do nothing.
    pub fn activate_back(&mut self, label: &str) -> Command {
        match label {
            STABILITY_LEVEL => {
                // Off, Damper, Attitude: back is two steps on.
                self.stability = self.stability.next().next();
                self.message(format!("Stability: {}", stability_name(self.stability)));
                Command::Click
            }
            "HUD colour" => {
                let colours = crate::cockpit_lighting::Lighting::COLOURS.len() as u8;
                self.lighting.colour = (self.lighting.colour + colours - 1) % colours;
                self.message(format!(
                    "HUD {}",
                    crate::cockpit_lighting::Lighting::COLOURS[usize::from(self.lighting.colour)]
                ));
                Command::Click
            }
            AI_OBSERVATION_RATE => {
                if self.ai_test_locked {
                    self.message(AI_TEST_LOCKED);
                    return Command::Click;
                }
                let rates = tore_sim::ai::test_settings::OBSERVATION_RATES_HZ;
                let at = rates
                    .iter()
                    .position(|hz| *hz == self.ai_test.observation_hz())
                    .unwrap_or(0);
                let hz = rates[(at + rates.len() - 1) % rates.len()];
                self.ai_test = self.ai_test.with_observation_hz(hz).expect("a listed rate");
                self.message(format!("AI test settings: {}", self.ai_test.describe()));
                Command::Click
            }
            _ if turns_back(label) => self.activate(label, ""),
            _ => Command::None,
        }
    }
    pub fn draw(&mut self, pixels: &mut [u8], font: &Font, tree: &[MenuNode]) {
        if self.menu {
            self.pause
                .draw(pixels, font, tree, self.look().0, &|label| {
                    self.cheat_state(label)
                });
        } else if self.paused {
            Canvas(pixels).rect((222, 35, 196, 22), [20, 30, 40, 230]);
            Paint {
                pixels,
                clip: (0, 0, 640, 480),
                color: [230, 240, 245, 255],
            }
            .text(font, "PAUSED - Ctrl-P to resume", 232, 42);
        }
    }
    /// When the next notice on screen expires, for a still picture to be
    /// drawn again without it.
    pub fn notices_until(&self) -> Option<Instant> {
        self.notices
            .iter()
            .map(|(_, at)| *at + NOTICE_LIFETIME)
            .min()
    }
    /// Draw the message lines straight onto the flight view at the HUD's
    /// on-screen scale, with smoothed edges like the HUD's filtered symbols.
    /// Pass the HUD font. Lines are centered and grow upward from the bottom.
    pub fn draw_notices(
        &mut self,
        canvas: &mut crate::flight_canvas::FlightCanvas,
        font: &Font,
        hud_color: [u8; 3],
    ) {
        self.notices
            .retain(|(_, at)| at.elapsed() < NOTICE_LIFETIME);
        draw_messages(
            canvas,
            font,
            hud_color,
            self.notices.iter().map(|(text, _)| text.as_str()),
            0.,
        );
    }
}
/// How long a message line stays, for a replay rebuilding the lines on
/// screen from the ones it recorded.
pub const MESSAGE_LIFETIME: Duration = NOTICE_LIFETIME;
/// The most message lines shown at once.
pub const MESSAGE_LINES: usize = NOTICE_LINES;
/// How far the full seven message lines reach above the edge they sit on,
/// in 640x480 layer units, for the HUD font `font`: the band the debug
/// panels and a replay's subtitles leave free.
pub fn message_band(font: &Font) -> f64 {
    let scale = crate::flight_canvas::HUD_SCALE;
    NOTICE_MARGIN - scale + NOTICE_LINES as f64 * (font.height + 1) as f64 * scale
}
/// Draw message lines as flight shows them: in `hud_color` and the HUD's
/// font at its on-screen size, centered, newest at the bottom, at most seven
/// lines. They sit `raise` layer units higher than flight's, which a replay
/// uses to keep them above its transport bar.
pub fn draw_messages<'a>(
    canvas: &mut crate::flight_canvas::FlightCanvas,
    font: &Font,
    hud_color: [u8; 3],
    messages: impl IntoIterator<Item = &'a str>,
    raise: f64,
) {
    let [w, h] = canvas.size.map(f64::from);
    let layer = (w / 640.).min(h / 480.);
    let scale = layer * crate::flight_canvas::HUD_SCALE;
    let lines: Vec<String> = messages
        .into_iter()
        .flat_map(|text| wrap(font, text, (600. * layer / scale) as usize))
        .collect();
    let lines = &lines[lines.len().saturating_sub(NOTICE_LINES)..];
    if lines.is_empty() {
        return;
    }
    let line_height = (font.height + 1) as f64 * scale;
    // The last line's spacing row sits below its text, inside the margin.
    let bottom = h - (NOTICE_MARGIN + raise) * layer + scale;
    let top = (bottom - lines.len() as f64 * line_height).round();
    // Glyph pixels become boxes; each screen pixel takes the area covered.
    // The areas are summed in a buffer only as wide as the text, kept from
    // one frame to the next.
    let rows = (bottom - top).ceil() as usize + 1;
    let canvas_columns = canvas.size[0] as usize;
    let glyph_boxes = |lines: &[String], mut each: Box<dyn FnMut(f64, f64, f64, f64) + '_>| {
        for (i, line) in lines.iter().enumerate() {
            let mut x = ((w - text_width(font, line) as f64 * scale) / 2.).round();
            let y = i as f64 * line_height;
            for ch in line.glyph_codes() {
                let glyph = &font.glyphs[ch as usize];
                for &(gx, gy) in &glyph.pixels {
                    let (left, up) = (x + gx as f64 * scale, y + gy as f64 * scale);
                    each(left, up, left + scale, up + scale);
                }
                x += glyph.advance as f64 * scale;
            }
        }
    };
    let (mut first, mut last) = (canvas_columns, 0);
    glyph_boxes(
        lines,
        Box::new(|left, _, right, _| {
            first = first.min(left.max(0.).floor() as usize);
            last = last.max((right.ceil().max(0.) as usize).min(canvas_columns));
        }),
    );
    if first >= last {
        return;
    }
    let columns = last - first;
    COVER.with(|cover| {
        let mut cover = cover.borrow_mut();
        cover.clear();
        cover.resize(columns * rows, 0.);
        glyph_boxes(
            lines,
            Box::new(|left, up, right, down| {
                for row in up.floor() as usize..(down.ceil() as usize).min(rows) {
                    let dy = down.min(row as f64 + 1.) - up.max(row as f64);
                    for column in left.max(0.).floor() as usize
                        ..(right.ceil().max(0.) as usize).min(canvas_columns)
                    {
                        let dx = right.min(column as f64 + 1.) - left.max(column as f64);
                        cover[row * columns + column - first] += dx * dy;
                    }
                }
            }),
        );
        for (at, &amount) in cover.iter().enumerate() {
            if amount > 0. {
                let (column, row) = (at % columns + first, at / columns);
                canvas.blend(
                    column as i32,
                    top as i32 + row as i32,
                    hud_color,
                    amount.min(1.),
                );
            }
        }
    });
}
thread_local! {
    /// Coverage areas for `draw_messages`, kept to avoid a large clear
    /// buffer every frame.
    static COVER: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn text_width(font: &Font, text: &str) -> usize {
    text.glyph_codes()
        .map(|ch| font.glyphs[ch as usize].advance)
        .sum()
}
/// Break a message into lines no wider than `width` pixels, at spaces.
fn wrap(font: &Font, text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let last = lines.last_mut().unwrap();
        let candidate = if last.is_empty() {
            word.to_owned()
        } else {
            format!("{last} {word}")
        };
        if text_width(font, &candidate) > width && !last.is_empty() {
            lines.push(word.to_owned());
        } else {
            *last = candidate;
        }
    }
    lines
}
#[cfg(test)]
mod tests {
    use super::*;
    fn shown(ui: &FlightUi) -> Vec<&str> {
        ui.notices.iter().map(|(text, _)| text.as_str()).collect()
    }
    #[test]
    fn a_still_paused_picture_wakes_when_its_oldest_notice_expires() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.notices_until(), None);
        let before = Instant::now();
        ui.message("first");
        ui.message("second");
        let until = ui.notices_until().unwrap();
        assert!(until >= before + NOTICE_LIFETIME);
        assert!(until <= Instant::now() + NOTICE_LIFETIME);
        assert_eq!(until, ui.notices[0].1 + NOTICE_LIFETIME);
        // Pausing for focus loss stops single-player time; a session's never.
        ui.pause_for_focus();
        assert!(ui.stopped());
        let mut session = FlightUi::default();
        session.enter_session();
        session.pause_for_focus();
        assert!(!session.stopped());
    }
    #[test]
    fn messages_stack_to_seven_lines_and_repeats_move_to_the_bottom() {
        let mut ui = FlightUi::default();
        for n in 0..9 {
            ui.message(format!("Message {n}"));
        }
        assert_eq!(ui.notices.len(), NOTICE_LINES);
        assert_eq!(shown(&ui)[0], "Message 2");
        ui.message("Message 4");
        assert_eq!(shown(&ui).len(), NOTICE_LINES);
        assert_eq!(shown(&ui).last().unwrap(), &"Message 4");
        assert_eq!(shown(&ui).iter().filter(|m| **m == "Message 4").count(), 1);
    }
    #[test]
    fn notes_say_what_became_of_each_message_line() {
        let mut ui = FlightUi::default();
        for n in 0..8 {
            ui.message(format!("Message {n}"));
        }
        // The eighth line pushes the first off the top before its time.
        let notes = ui.take_notes();
        assert_eq!(notes.len(), 9);
        assert!(notes[..8].iter().all(|(_, shown)| *shown == Shown::Now));
        assert_eq!(notes[8], ("Message 0".into(), Shown::PushedOff));
        ui.message("Message 5");
        assert_eq!(ui.take_notes(), [("Message 5".into(), Shown::Repeated)]);
        // A line whose five seconds are up is gone, so its text is new again
        // and it pushes nothing still showing.
        for (_, at) in &mut ui.notices {
            *at -= NOTICE_LIFETIME;
        }
        ui.message("Message 6");
        ui.message("Message 9");
        assert_eq!(
            ui.take_notes(),
            [
                ("Message 6".into(), Shown::Now),
                ("Message 9".into(), Shown::Now)
            ]
        );
        assert!(ui.take_notes().is_empty());
    }
    #[test]
    fn messages_expire_after_five_seconds() {
        let mut ui = FlightUi::default();
        ui.message("Old");
        ui.message("New");
        ui.notices[0].1 -= NOTICE_LIFETIME;
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![(0, 0)],
                })
                .collect(),
        };
        let mut canvas = crate::flight_canvas::FlightCanvas::default();
        canvas.size = [1280, 960];
        canvas.pixels = vec![0; 1280 * 960 * 4];
        ui.draw_notices(&mut canvas, &font, [0, 255, 0]);
        assert_eq!(shown(&ui), ["New"]);
        // Three one-pixel glyphs at the HUD's scale on a 2x layer: each
        // covers 1.445 screen pixels square, in HUD green.
        let lit: Vec<_> = canvas
            .pixels
            .chunks_exact(4)
            .filter(|p| p[3] != 0)
            .collect();
        assert!(lit.iter().all(|p| p[..3] == [0, 255, 0]));
        let alpha: f64 = lit.iter().map(|p| f64::from(p[3]) / 255.).sum();
        assert!((alpha - 3. * 1.445f64.powi(2)).abs() < 0.05);
    }
    /// `draw_messages` with the canvas-wide buffer it used to have.
    fn draw_messages_reference<'a>(
        canvas: &mut crate::flight_canvas::FlightCanvas,
        font: &Font,
        hud_color: [u8; 3],
        messages: impl IntoIterator<Item = &'a str>,
        raise: f64,
    ) {
        let [w, h] = canvas.size.map(f64::from);
        let layer = (w / 640.).min(h / 480.);
        let scale = layer * crate::flight_canvas::HUD_SCALE;
        let lines: Vec<String> = messages
            .into_iter()
            .flat_map(|text| wrap(font, text, (600. * layer / scale) as usize))
            .collect();
        let lines = &lines[lines.len().saturating_sub(NOTICE_LINES)..];
        if lines.is_empty() {
            return;
        }
        let line_height = (font.height + 1) as f64 * scale;
        let bottom = h - (NOTICE_MARGIN + raise) * layer + scale;
        let top = (bottom - lines.len() as f64 * line_height).round();
        let (columns, rows) = (canvas.size[0] as usize, (bottom - top).ceil() as usize + 1);
        let mut cover = vec![0f64; columns * rows];
        for (i, line) in lines.iter().enumerate() {
            let mut x = ((w - text_width(font, line) as f64 * scale) / 2.).round();
            let y = i as f64 * line_height;
            for ch in line.glyph_codes() {
                let glyph = &font.glyphs[ch as usize];
                for &(gx, gy) in &glyph.pixels {
                    let (left, up) = (x + gx as f64 * scale, y + gy as f64 * scale);
                    let (right, down) = (left + scale, up + scale);
                    for row in up.floor() as usize..(down.ceil() as usize).min(rows) {
                        let dy = down.min(row as f64 + 1.) - up.max(row as f64);
                        for column in left.max(0.).floor() as usize
                            ..(right.ceil().max(0.) as usize).min(columns)
                        {
                            let dx = right.min(column as f64 + 1.) - left.max(column as f64);
                            cover[row * columns + column] += dx * dy;
                        }
                    }
                }
                x += glyph.advance as f64 * scale;
            }
        }
        for (at, amount) in cover.into_iter().enumerate() {
            if amount > 0. {
                let (column, row) = (at % columns, at / columns);
                canvas.blend(
                    column as i32,
                    top as i32 + row as i32,
                    hud_color,
                    amount.min(1.),
                );
            }
        }
    }
    #[test]
    fn messages_in_a_text_sized_buffer_match_the_canvas_wide_original() {
        let font = Font {
            height: 7,
            glyphs: (0..256)
                .map(|c| tore_formats::font::Glyph {
                    advance: 4 + c % 3,
                    pixels: (0..(c % 5))
                        .map(|k| ((k * 2 + c) % 6, (k + c / 3) % 7))
                        .collect(),
                })
                .collect(),
        };
        let texts = [
            vec!["Hello there"],
            vec![
                "A",
                "Bb Cc dd ee ff gg hh ii jj kk ll mm nn oo pp qq rr ss tt uu vv ww xx yy zz 0123456789 and so on for a good while to wrap lines",
            ],
            vec![],
            vec!["", "x"],
        ];
        for size in [[640, 480], [1280, 960], [1920, 1080], [300, 700]] {
            for (n, messages) in texts.iter().enumerate() {
                for raise in [0., 31.5] {
                    let mut actual = crate::flight_canvas::FlightCanvas::default();
                    actual.blank(size);
                    for (i, p) in actual.pixels.chunks_exact_mut(4).enumerate() {
                        p.copy_from_slice(&[(i % 251) as u8, 40, 90, [0, 255, 77][i % 3]]);
                    }
                    let mut expected = crate::flight_canvas::FlightCanvas::default();
                    expected.blank(size);
                    expected.pixels.clone_from(&actual.pixels);
                    draw_messages(
                        &mut actual,
                        &font,
                        [10, 255, 10],
                        messages.iter().copied(),
                        raise,
                    );
                    draw_messages_reference(
                        &mut expected,
                        &font,
                        [10, 255, 10],
                        messages.iter().copied(),
                        raise,
                    );
                    assert!(
                        actual.pixels == expected.pixels,
                        "{size:?} text {n} raise {raise}"
                    );
                }
            }
        }
    }
    #[test]
    fn messages_can_sit_higher_and_fill_a_known_band() {
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![(0, 0)],
                })
                .collect(),
        };
        let drawn_rows = |raise: f64, count: usize| {
            let mut canvas = crate::flight_canvas::FlightCanvas::default();
            canvas.size = [1280, 960];
            canvas.pixels = vec![0; 1280 * 960 * 4];
            let lines: Vec<String> = (0..count).map(|n| format!("Line {n}")).collect();
            draw_messages(
                &mut canvas,
                &font,
                [0, 255, 0],
                lines.iter().map(String::as_str),
                raise,
            );
            let rows: Vec<usize> = canvas
                .pixels
                .chunks_exact(1280 * 4)
                .enumerate()
                .filter(|(_, row)| row.chunks_exact(4).any(|p| p[3] != 0))
                .map(|(y, _)| y)
                .collect();
            (rows[0], *rows.last().unwrap())
        };
        // Raised by 50 layer units on a 2x layer: 100 pixels higher.
        let (low_top, low_bottom) = drawn_rows(0., 1);
        let (high_top, high_bottom) = drawn_rows(50., 1);
        assert_eq!((low_top - high_top, low_bottom - high_bottom), (100, 100));
        // Seven lines, and never more, stay inside the band.
        let band = message_band(&font);
        let (top, _) = drawn_rows(0., 7);
        assert_eq!(drawn_rows(0., 9), drawn_rows(0., 7));
        assert!(top as f64 >= 960. - band * 2. - 1., "{top} {band}");
        assert!(top as f64 <= 960. - band * 2. + 3., "{top} {band}");
    }
    #[test]
    fn retail_views_work_without_menu_data_and_modifiers_do_not_leak() {
        use crate::flight_views::{self, Reference};
        let mut ui = FlightUi::default();
        for key in [
            "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F12",
        ] {
            let view = flight_views::key(key).unwrap();
            assert_eq!(ui.key(key, false, false, false, &[]), Command::View(view));
            assert_eq!(ui.activate("Retail view", key), Command::View(view));
            assert_eq!(
                ui.key(key, false, true, false, &[]),
                Command::ViewRelative(view, Reference::Missile)
            );
            assert_eq!(
                ui.key(key, false, false, true, &[]),
                if key == "F4" {
                    Command::Exit
                } else {
                    Command::ViewRelative(view, Reference::Target)
                }
            );
            assert_eq!(ui.key(key, true, false, false, &[]), Command::None);
            assert_eq!(ui.key(key, false, true, true, &[]), Command::None);
        }
        assert_eq!(ui.key("v", false, false, false, &[]), Command::StoreView);
        assert_eq!(ui.activate("Current", ""), Command::Panel(1));
        ui.key("F11", false, false, false, &[]);
        assert!(ui.pause.help && ui.menu);
        assert_eq!(ui.key("F7", false, false, false, &[]), Command::None);
    }
    #[test]
    fn ejection_shortcut_alias_is_shift_only_and_does_not_fire_in_menus() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("e", true, false, false, &tree()), Command::Eject);
        assert_eq!(
            ui.key("e", false, false, false, &tree()),
            Command::Toggle(Switch::Engine)
        );
        assert_eq!(ui.key("e", false, true, false, &tree()), Command::None);
        assert_eq!(ui.key("e", true, true, false, &tree()), Command::None);
        // Alt+Shift+E is the Engaging reply, not an eject.
        assert_eq!(
            ui.key("e", true, false, true, &tree()),
            Command::Reply(Reply::Engaging)
        );
        ui.menu = true;
        assert_eq!(ui.key("e", true, false, false, &tree()), Command::None);
    }
    #[test]
    fn pilot_death_switches_to_f10_once_without_pausing() {
        let mut ui = FlightUi {
            look: [0.8, -0.3],
            zoom: 2.,
            ..Default::default()
        };
        ui.map.open = true;
        assert_eq!(ui.pilot_death_view(false, false), None);
        assert_eq!(ui.pilot_death_view(false, true), Some(1));
        assert_eq!(ui.look, [0.; 2]);
        assert_eq!(ui.zoom, 1.);
        assert!(!ui.paused && !ui.menu && !ui.map.open);
        assert_eq!(ui.pilot_death_view(true, true), None);
    }
    #[test]
    fn d_reports_damage_without_injecting_it_and_messages_do_not_overwrite() {
        let mut ui = FlightUi::default();
        assert_eq!(
            ui.key("d", false, false, false, &tree()),
            Command::DamageReport
        );
        for (shift, ctrl, alt) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_ne!(
                ui.key("d", shift, ctrl, alt, &tree()),
                Command::DamageReport
            );
        }
        // Shift+D is FA's message history, which reports itself unavailable.
        ui.notices.clear();
        ui.message("Wing order");
        ui.message("Oil leak");
        assert_eq!(shown(&ui), ["Wing order", "Oil leak"]);
        ui.menu = true;
        assert_ne!(
            ui.key("d", false, false, false, &tree()),
            Command::DamageReport
        );
    }
    #[test]
    fn map_shortcut_pan_and_escape_do_not_pause_or_switch_sensors() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("m", true, false, false, &[]), Command::Click);
        assert!(ui.map.open);
        assert!(!ui.frozen());
        assert_eq!(ui.key("Enter", false, false, false, &[]), Command::None);
        assert!(ui.map.mission_tab);
        assert!(!ui.frozen());
        assert_eq!(ui.key("PageDown", false, false, false, &[]), Command::None);
        assert_eq!(ui.key("Enter", false, false, false, &[]), Command::None);
        assert!(!ui.map.mission_tab);
        assert_eq!(ui.key("ArrowUp", false, false, false, &[]), Command::None);
        assert_eq!(ui.key("Escape", false, false, false, &[]), Command::Click);
        assert!(!ui.map.open);
        assert!(!ui.menu);
        ui.menu = true;
        ui.key("m", true, false, false, &tree());
        assert!(!ui.map.open);
    }
    fn tree() -> Vec<MenuNode> {
        vec![MenuNode {
            label: "?".into(),
            shortcut: String::new(),
            children: vec![MenuNode {
                label: "End mission".into(),
                shortcut: "Ctrl-Q".into(),
                children: vec![],
            }],
        }]
    }
    #[test]
    fn autopilot_shortcuts_and_menu_actions() {
        let mut ui = FlightUi::default();
        for (ctrl, switch) in [
            (false, Switch::Autopilot),
            (true, Switch::WaypointAutopilot),
        ] {
            assert_eq!(
                ui.key("a", false, ctrl, false, &tree()),
                Command::Toggle(switch)
            );
        }
        // Shift+A is FA's AWACS link; Alt+A is the data link's sort (slice
        // G3c); neither reaches the autopilot.
        assert_eq!(ui.key("a", true, false, false, &tree()), Command::Click);
        assert_eq!(
            ui.key("a", false, false, true, &tree()),
            Command::Wing(tore_sim::ai::wing::PlayerOrder::Sort)
        );
        assert_eq!(
            ui.activate("Autopilot", "A"),
            Command::Toggle(Switch::Autopilot)
        );
        ui.menu = true;
        assert_eq!(ui.key("a", false, false, false, &tree()), Command::None);
    }

    #[test]
    fn no_turbulence_is_a_session_toggle_preserved_by_restart() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.activate("No turbulence?", ""), Command::Click);
        assert!(ui.cheats.no_turbulence);
        ui.reset_for_flight();
        assert!(ui.cheats.no_turbulence);
        ui.activate("No turbulence?", "");
        assert!(!ui.cheats.no_turbulence);
    }

    #[test]
    fn t_enter_and_shift_t_are_separate_targeting_commands() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("t", false, false, false, &tree()), Command::Target);
        assert_eq!(
            ui.key("t", true, false, false, &tree()),
            Command::TargetPrevious
        );
        assert_eq!(
            ui.key("Enter", false, false, false, &tree()),
            Command::TargetVisual
        );
        assert_eq!(
            ui.key("'", false, false, false, &tree()),
            Command::TargetCentre
        );
    }
    #[test]
    fn cheat_rows_toggle_show_state_and_survive_restart() {
        let mut ui = FlightUi::default();
        for label in [
            "Unlimited ammo?",
            "Unlimited fuel?",
            "No spins?",
            "Pull extra G?",
            "Ignore weapon weights?",
            "No redout or blackout?",
            "No screen-shaking?",
            "No crashes?",
            "Easy aiming?",
            "Ignore midair collisions?",
            "Easy targeting?",
            "Air combat guns only?",
        ] {
            assert_eq!(ui.cheat_state(label), Some("Off"));
            assert_eq!(ui.activate(label, ""), Command::Click);
            assert_eq!(ui.cheat_state(label), Some("On"));
        }
        assert_eq!(ui.cheat_state("Normal"), Some("On"));
        ui.activate("Invulnerable", "");
        assert_eq!(ui.cheat_state("Invulnerable"), Some("On"));
        assert_eq!(ui.cheat_state("Normal"), Some("Off"));
        ui.reset_for_flight();
        let c = ui.cheats;
        assert!(c.invulnerable() && c.unlimited_ammo && c.unlimited_fuel);
        assert!(c.no_spins && c.extra_g && c.ignore_weapon_weights);
        ui.activate("Realistic", "");
        assert_eq!(ui.cheats.damage, Damage::Realistic);
        assert_eq!(ui.cheat_state("Realistic"), Some("On"));
        assert_eq!(ui.cheat_state("Invulnerable"), Some("Off"));
        assert_eq!(ui.cheat_state("Normal"), Some("Off"));
        ui.reset_for_flight();
        assert_eq!(ui.cheats.damage, Damage::Realistic);
        ui.activate("Normal", "");
        assert_eq!(ui.cheats.damage, Damage::Normal);
        assert_eq!(ui.cheat_state("Enemy AI?"), None);
        assert_eq!(ui.cheat_state("Unchanged"), Some("On"));
        ui.activate("Novice", "");
        assert_eq!(ui.cheat_state("Novice"), Some("On"));
        assert_eq!(ui.cheat_state("Unchanged"), Some("Off"));
        ui.reset_for_flight();
        assert_eq!(ui.cheats.enemy_ai, Some(tore_sim::ai::Experience::Novice));
        ui.activate("Unchanged", "");
        assert_eq!(ui.cheats.enemy_ai, None);
    }

    #[test]
    fn weapon_diagnostics_row_is_hidden_by_default_and_moves_the_top_right_window() {
        use crate::instruments::{Instruments, Layout};
        let mut t = tree();
        t.push(MenuNode {
            label: "Pref".into(),
            shortcut: String::new(),
            children: vec![MenuNode {
                label: "HUD pitch ladder?".into(),
                shortcut: String::new(),
                children: vec![],
            }],
        });
        add_authored_rows(&mut t);
        add_authored_rows(&mut t);
        let rows: Vec<_> = t[1].children.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            rows,
            [
                "HUD pitch ladder?",
                WEAPON_DIAGNOSTICS,
                DEBUG_PANELS,
                STABILITY_LEVEL,
                "HUD colour",
                "Cockpit lighting"
            ]
        );
        assert_eq!(
            t[1].children
                .last()
                .unwrap()
                .children
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Dim instruments",
                "Brighten instruments",
                "Dim flood",
                "Brighten flood"
            ]
        );
        // Other roots are untouched.
        assert_eq!(t[0].children.len(), 1);

        let mut ui = FlightUi::default();
        let mut i = Instruments::new(Layout::Large, None);
        let size = [1280., 960.];
        let normal = Layout::Large.rect_on(3, size);
        i.weapon_debug = ui.weapon_diagnostics_shown(0);
        assert!(!ui.weapon_diagnostics && !i.weapon_debug);
        assert_eq!(i.screen_rect(3, size), normal);

        // Escape, Right to Pref, Down to the authored row, Enter.
        ui.key("Escape", false, false, false, &t);
        ui.key("ArrowRight", false, false, false, &t);
        ui.key("ArrowDown", false, false, false, &t);
        let label = |ui: &FlightUi| {
            ui.controls(&t)
                .into_iter()
                .find(|(id, ..)| *id == 1)
                .map(|(.., label)| label)
                .unwrap()
        };
        assert_eq!(label(&ui), format!("{WEAPON_DIAGNOSTICS}  Off"));
        assert_eq!(ui.key("Enter", false, false, false, &t), Command::Click);
        assert!(ui.weapon_diagnostics && ui.menu);
        assert_eq!(label(&ui), format!("{WEAPON_DIAGNOSTICS}  On"));
        assert_eq!(*shown(&ui).last().unwrap(), "Weapon diagnostics: on");
        i.weapon_debug = ui.weapon_diagnostics_shown(0);
        let (x, y, w, h) = i.screen_rect(3, size);
        assert_eq!((x, w, h), (normal.0, normal.2, normal.3));
        assert_eq!(y, normal.1 + 104. * 2.);
        // Only the cockpit, back and up views with the HUD shown draw it.
        assert!(!ui.weapon_diagnostics_shown(1) && !ui.weapon_diagnostics_shown(2));
        ui.hud = false;
        assert!(!ui.weapon_diagnostics_shown(0));
        ui.hud = true;

        ui.key("Enter", false, false, false, &t);
        assert!(!ui.weapon_diagnostics);
        assert_eq!(*shown(&ui).last().unwrap(), "Weapon diagnostics: off");
        i.weapon_debug = ui.weapon_diagnostics_shown(0);
        assert_eq!(i.screen_rect(3, size), normal);
    }

    #[test]
    fn debug_panels_row_is_off_by_default_and_toggles_with_a_message() {
        let mut t = tree();
        t.push(MenuNode {
            label: "Pref".into(),
            shortcut: String::new(),
            children: vec![],
        });
        add_authored_rows(&mut t);
        let mut ui = FlightUi::default();
        assert!(!ui.debug_panels);
        assert_eq!(ui.cheat_state(DEBUG_PANELS), Some("Off"));
        // Escape, Right to Pref, Down to the second authored row, Enter.
        ui.key("Escape", false, false, false, &t);
        ui.key("ArrowRight", false, false, false, &t);
        ui.key("ArrowDown", false, false, false, &t);
        assert_eq!(ui.key("Enter", false, false, false, &t), Command::Click);
        assert!(ui.debug_panels && !ui.weapon_diagnostics);
        assert_eq!(ui.cheat_state(DEBUG_PANELS), Some("On"));
        assert_eq!(
            *shown(&ui).last().unwrap(),
            "Debug panels: on (right-click an aircraft)"
        );
        ui.activate(DEBUG_PANELS, "");
        assert!(!ui.debug_panels);
        // A new flight keeps the choice only through the saved preferences.
        ui.debug_panels = true;
        ui.reset_for_flight();
        assert!(!ui.debug_panels);
    }

    #[test]
    fn stability_row_cycles_the_level_with_a_message() {
        let mut t = tree();
        t.push(MenuNode {
            label: "Pref".into(),
            shortcut: String::new(),
            children: vec![],
        });
        add_authored_rows(&mut t);
        let mut ui = FlightUi::default();
        assert_eq!(ui.cheat_state(STABILITY_LEVEL), Some("Damper"));
        for (expected, shown_name) in [
            (tore_input::StabilityLevel::Attitude, "Attitude"),
            (tore_input::StabilityLevel::Off, "Off"),
            (tore_input::StabilityLevel::Damper, "Damper"),
        ] {
            assert_eq!(ui.activate(STABILITY_LEVEL, ""), Command::Click);
            assert_eq!(ui.stability, expected);
            assert_eq!(ui.cheat_state(STABILITY_LEVEL), Some(shown_name));
            assert_eq!(
                *shown(&ui).last().unwrap(),
                format!("Stability: {shown_name}")
            );
        }
    }

    /// A right click on `label` in the tab that holds it, through the menu.
    fn right_click_row(ui: &mut FlightUi, tree: &[MenuNode], label: &str) -> Command {
        let (tab, row) = tree
            .iter()
            .enumerate()
            .find_map(|(t, node)| {
                node.children
                    .iter()
                    .position(|n| n.label == label)
                    .map(|r| (t, r))
            })
            .expect("a row with that label");
        ui.pause.show_tab(tab);
        let point = Some((200., 50. + row as f64 * 19. + 5.));
        ui.right_pointer(tree, point, true);
        ui.right_pointer(tree, point, false)
    }
    fn left_click_row(ui: &mut FlightUi, tree: &[MenuNode], label: &str) -> Command {
        let (tab, row) = tree
            .iter()
            .enumerate()
            .find_map(|(t, node)| {
                node.children
                    .iter()
                    .position(|n| n.label == label)
                    .map(|r| (t, r))
            })
            .expect("a row with that label");
        ui.pause.show_tab(tab);
        let point = Some((200., 50. + row as f64 * 19. + 5.));
        ui.pointer(tree, point, true);
        ui.pointer(tree, point, false)
    }
    #[test]
    fn right_click_turns_the_cycling_rows_back_and_left_click_forward_wrapping() {
        let mut t = tree();
        t.push(MenuNode {
            label: "Pref".into(),
            shortcut: String::new(),
            children: vec![],
        });
        add_authored_rows(&mut t);
        add_debug_tab(&mut t);
        let mut ui = FlightUi {
            menu: true,
            ..Default::default()
        };

        // Stability: Damper, then back to Off, then wrapping to Attitude.
        assert_eq!(ui.stability, tore_input::StabilityLevel::Damper);
        for want in [
            tore_input::StabilityLevel::Off,
            tore_input::StabilityLevel::Attitude,
            tore_input::StabilityLevel::Damper,
        ] {
            assert_eq!(
                right_click_row(&mut ui, &t, STABILITY_LEVEL),
                Command::Click
            );
            assert_eq!(ui.stability, want);
        }
        assert_eq!(left_click_row(&mut ui, &t, STABILITY_LEVEL), Command::Click);
        assert_eq!(ui.stability, tore_input::StabilityLevel::Attitude);
        assert_eq!(*shown(&ui).last().unwrap(), "Stability: Attitude");

        // HUD colour: GREEN, wrapping back to CYAN and round again.
        assert_eq!(ui.lighting.colour, 0);
        for want in [3, 2, 1, 0] {
            right_click_row(&mut ui, &t, "HUD colour");
            assert_eq!(ui.lighting.colour, want);
        }
        assert_eq!(*shown(&ui).last().unwrap(), "HUD GREEN");
        for want in [1, 2, 3, 0] {
            left_click_row(&mut ui, &t, "HUD colour");
            assert_eq!(ui.lighting.colour, want);
        }

        // The AI observation rate: 60, then back to 120 and round to 20, 30
        // and 40, and forward again round the other way.
        for want in [120, 20, 30, 40, 60] {
            right_click_row(&mut ui, &t, AI_OBSERVATION_RATE);
            assert_eq!(ui.ai_test.observation_hz(), want);
        }
        for want in [40, 30, 20, 120, 60] {
            left_click_row(&mut ui, &t, AI_OBSERVATION_RATE);
            assert_eq!(ui.ai_test.observation_hz(), want);
        }
        // A joined game's host settings stay as they are.
        ui.lock_ai_test(true);
        let before = ui.ai_test;
        right_click_row(&mut ui, &t, AI_OBSERVATION_RATE);
        assert_eq!(ui.ai_test, before);
        assert_eq!(*shown(&ui).last().unwrap(), AI_TEST_LOCKED);
    }
    #[test]
    fn right_click_flips_two_state_rows_and_leaves_choices_and_commands_alone() {
        let mut t = tree();
        t.push(MenuNode {
            label: "Pref".into(),
            shortcut: String::new(),
            children: vec![],
        });
        add_authored_rows(&mut t);
        let mut ui = FlightUi {
            menu: true,
            ..Default::default()
        };
        assert_eq!(right_click_row(&mut ui, &t, DEBUG_PANELS), Command::Click);
        assert!(ui.debug_panels);
        right_click_row(&mut ui, &t, DEBUG_PANELS);
        assert!(!ui.debug_panels);
        // A command row, and the End mission row, take no right click.
        assert_eq!(right_click_row(&mut ui, &t, "End mission"), Command::None);
        assert!(ui.menu);
        // Only a press and release on one row count.
        ui.pause.show_tab(1);
        let on = Some((200., 50. + 1. * 19. + 5.));
        ui.right_pointer(&t, on, true);
        let off = Some((200., 50. + 0. * 19. + 5.));
        assert_eq!(ui.right_pointer(&t, off, false), Command::None);
        assert!(!ui.debug_panels);
        // The tab strip and the bottom buttons ignore the right button.
        for point in [(30., 10.), (30., 455.)] {
            ui.right_pointer(&t, Some(point), true);
            assert_eq!(ui.right_pointer(&t, Some(point), false), Command::None);
        }
    }

    /// E3: the Easy flight physics row follows the imported Cheat rows,
    /// toggles mid-flight, survives Restart and is the server's in a session.
    #[test]
    fn easy_physics_row_is_authored_after_the_cheat_rows_and_is_the_servers_in_a_session() {
        let mut t = retail_tree();
        let imported = labels(&t);
        let imported_cheat = labels(&t.iter().find(|n| n.label == "Cheat").unwrap().children);
        add_authored_rows(&mut t);
        add_authored_rows(&mut t);
        let cheat = t.iter().find(|n| n.label == "Cheat").unwrap();
        let rows: Vec<_> = cheat.children.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(rows.last(), Some(&EASY_PHYSICS), "after the imported rows");
        assert_eq!(rows.iter().filter(|r| **r == EASY_PHYSICS).count(), 1);
        // Nothing imported moved or went from the Cheat menu; only the row
        // was added at its end.
        let mut before_the_row = labels(&cheat.children);
        assert_eq!(before_the_row.pop().as_deref(), Some(EASY_PHYSICS));
        assert_eq!(before_the_row, imported_cheat);
        assert!(imported.iter().all(|label| authored_has(&t, label)));
        // Off to start; a toggle takes effect at once, with a line.
        let mut ui = FlightUi::default();
        assert_eq!(ui.cheat_state(EASY_PHYSICS), Some("Off"));
        assert!(!ui.cheats.easy_physics);
        assert_eq!(ui.activate(EASY_PHYSICS, ""), Command::Click);
        assert!(ui.cheats.easy_physics);
        assert_eq!(ui.cheat_state(EASY_PHYSICS), Some("On"));
        assert_eq!(*shown(&ui).last().unwrap(), "Easy flight physics: on");
        // It lasts for the session: a new flight keeps it.
        ui.reset_for_flight();
        assert!(ui.cheats.easy_physics);
        assert_eq!(ui.activate(EASY_PHYSICS, ""), Command::Click);
        assert!(!ui.cheats.easy_physics);
        assert_eq!(*shown(&ui).last().unwrap(), "Easy flight physics: off");
        // In a session it changes the simulation, so the server alone sets
        // it: refused with the usual line, gone from the session's menu, and
        // not carried in from single player.
        assert_eq!(
            session_refusal(EASY_PHYSICS),
            Some("The server sets the cheats in a multiplayer flight")
        );
        assert!(!labels(&session_menu(&t)).contains(&EASY_PHYSICS.to_string()));
        ui.cheats.easy_physics = true;
        ui.enter_session();
        assert!(!ui.cheats.easy_physics, "single player's choice stays out");
        assert_eq!(ui.activate(EASY_PHYSICS, ""), Command::Click);
        assert!(!ui.cheats.easy_physics, "a client cannot turn it on");
        assert!(
            ui.notices
                .iter()
                .any(|(line, _)| line == "The server sets the cheats in a multiplayer flight")
        );
    }

    #[test]
    fn source_whiteout_cheat_toggles_while_paused() {
        let mut ui = FlightUi {
            menu: true,
            ..Default::default()
        };
        assert_eq!(ui.activate("No sun whiteout?", ""), Command::Click);
        assert!(ui.cheats.no_sun_whiteout && ui.frozen());
        ui.reset_for_flight();
        assert!(ui.cheats.no_sun_whiteout);
        assert!(!ui.frozen());
        assert_eq!(ui.activate("No sun whiteout?", ""), Command::Click);
        assert!(!ui.cheats.no_sun_whiteout);
    }

    #[test]
    fn lighting_chords_expire_cancel_and_never_zoom() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("f", false, true, false, &[]), Command::None);
        assert_eq!(ui.key("i", false, true, false, &[]), Command::Click);
        assert!(ui.lighting_key("-", false));
        assert_eq!(ui.lighting.instruments, 3);
        assert!(ui.lighting_key("+", false));
        assert_eq!(ui.lighting.instruments, 4);
        assert_eq!(ui.zoom, 1.);
        ui.key("f", true, true, false, &[]);
        assert!(ui.lighting_key("=", false));
        assert_eq!(ui.lighting.flood, 4);
        assert!(!ui.lighting_key("x", false));
        assert!(!ui.lighting_key("-", false));
        ui.lighting_chord = Some((false, Instant::now() - std::time::Duration::from_secs(5)));
        assert!(!ui.lighting_key("-", false));
        ui.key("i", false, true, false, &[]);
        ui.menu = true;
        assert!(!ui.lighting_key("-", false));
        ui.menu = false;
        ui.key("h", false, true, false, &[]);
        assert_eq!(ui.lighting.colour, 1);
        assert_eq!(ui.brightness, 0);
    }
    #[test]
    fn hud_brightness_uses_source_steps_and_saturates() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.brightness, 0);
        ui.activate("Dim HUD", "");
        assert_eq!(ui.brightness, -16);
        for _ in 0..40 {
            ui.activate("Brighten HUD", "");
        }
        assert_eq!(ui.brightness, 256);
        for _ in 0..40 {
            ui.activate("Dim HUD", "");
        }
        assert_eq!(ui.brightness, -256);
    }
    #[test]
    fn control_root_opens_editor_without_changing_imported_tree() {
        let mut t = tree();
        t.push(MenuNode {
            label: "Control".into(),
            shortcut: String::new(),
            children: vec![MenuNode {
                label: "Keyboard".into(),
                shortcut: String::new(),
                children: vec![],
            }],
        });
        let mut u = FlightUi {
            menu: true,
            ..Default::default()
        };
        assert_eq!(
            u.key("ArrowRight", false, false, false, &t),
            Command::ControlsOpen
        );
        u.controls_closed();
        assert!(u.menu);
        assert_eq!(u.pause.root, 0);
        assert_eq!(t[1].children[0].label, "Keyboard");
    }
    #[test]
    fn escape_pauses_without_ending_and_modifiers_do_not_toggle_devices() {
        let mut u = FlightUi::default();
        let t = tree();
        assert_eq!(u.key("Escape", false, false, false, &t), Command::None);
        assert!(u.frozen());
        assert_eq!(u.key("g", false, false, false, &t), Command::None);
        u.key("Escape", false, false, false, &t);
        assert!(!u.frozen());
        assert_eq!(u.key("g", false, true, false, &t), Command::None);
        assert_eq!(
            u.key("g", false, false, false, &t),
            Command::Toggle(Switch::Gear)
        );
        assert_eq!(u.key("q", false, true, false, &t), Command::End);
    }
    fn node(label: &str, shortcut: &str, children: Vec<MenuNode>) -> MenuNode {
        MenuNode {
            label: label.into(),
            shortcut: shortcut.into(),
            children,
        }
    }
    /// The parts of the retail menu a session changes.
    fn retail_tree() -> Vec<MenuNode> {
        vec![
            node("?", "", vec![node("End mission", "Ctrl-Q", vec![])]),
            node(
                "Pref",
                "",
                vec![
                    node(
                        "Time ",
                        "",
                        ["Paused", "Slow-motion", "1x", "2x", "4x", "8x"]
                            .map(|label| node(label, "", vec![]))
                            .into(),
                    ),
                    node("Large windows?", "", vec![]),
                ],
            ),
            node(
                "Cheat",
                "",
                vec![
                    node("Damage", "", vec![node("Invulnerable", "", vec![])]),
                    node("Unlimited ammo?", "", vec![]),
                    node("No sun whiteout?", "", vec![]),
                    node("No redout or blackout?", "", vec![]),
                    node("No screen-shaking?", "", vec![]),
                    node("Enemy AI?", "", vec![node("Novice", "", vec![])]),
                ],
            ),
            node("Pos", "", vec![node("40,000 feet", "", vec![])]),
        ]
    }
    fn authored_has(tree: &[MenuNode], label: &str) -> bool {
        labels(tree).iter().any(|l| l == label)
    }
    fn labels(tree: &[MenuNode]) -> Vec<String> {
        tree.iter()
            .flat_map(|n| std::iter::once(n.label.clone()).chain(labels(&n.children)))
            .collect()
    }
    #[test]
    fn a_session_menu_drops_time_rows_mission_cheats_and_the_pos_menu() {
        let shown = labels(&session_menu(&retail_tree()));
        assert_eq!(
            shown,
            [
                "?",
                "End mission",
                "Pref",
                "Large windows?",
                "Cheat",
                "No sun whiteout?",
                "No redout or blackout?",
                "No screen-shaking?",
            ]
        );
        // Single player's menu is the retail one.
        assert_eq!(labels(&retail_tree()).len(), 22);
    }
    #[test]
    fn a_session_cannot_pause_or_compress_time_and_refuses_with_a_line() {
        let mut ui = FlightUi::default();
        ui.cheats.unlimited_ammo = true;
        ui.cheats.no_screen_shake = true;
        ui.time_scale = 4.;
        ui.enter_session();
        assert!(ui.session);
        // Carried over: the screen-only cheat. Gone: the mission cheat.
        assert!(ui.cheats.no_screen_shake && !ui.cheats.unlimited_ammo);
        assert_eq!(ui.time_scale, 1.);
        let tree = retail_tree();
        for label in ["Paused", "2x", "Slow-motion", "Unlimited ammo?", "Novice"] {
            let before = ui.cheats;
            assert_eq!(ui.activate(label, ""), Command::Click, "{label}");
            assert!(!ui.paused && ui.time_scale == 1. && ui.cheats == before);
        }
        assert_eq!(ui.activate("Restart free flight", ""), Command::Click);
        assert!(ui.notices.iter().any(|(line, _)| line == RESTART_REFUSED));
        // The screen-only cheats still toggle.
        assert_eq!(ui.activate("No sun whiteout?", ""), Command::Click);
        assert!(ui.cheats.no_sun_whiteout);
        // Control-P and C do nothing but say so.
        ui.key("p", false, true, false, &tree);
        ui.key("c", false, false, false, &tree);
        assert!(!ui.paused && ui.time_scale == 1.);
        // Losing the window's attention does not pause either.
        ui.pause_for_focus();
        assert!(!ui.paused);
    }
    #[test]
    fn the_flight_runs_on_behind_a_menu_in_a_session_and_stops_in_single_player() {
        let tree = tree();
        let mut clock = crate::flight::Clock { remainder: 0. };
        let mut single = FlightUi::default();
        single.key("Escape", false, false, false, &tree);
        assert!(single.frozen() && single.stopped());
        assert_eq!(single.steps(&mut clock, 0.1), 0);
        let mut session = FlightUi::default();
        session.enter_session();
        session.key("Escape", false, false, false, &tree);
        // A menu is up, so the player takes no flight input, but time moves.
        assert!(session.frozen() && !session.stopped());
        assert_eq!(session.steps(&mut clock, 0.1), 12);
        single.pause_for_focus();
        assert!(single.paused);
    }
    #[test]
    fn pause_discards_wall_time_and_time_scaling_keeps_fixed_ticks() {
        let mut u = FlightUi::default();
        let mut clock = crate::flight::Clock { remainder: 0. };
        let tree = tree();
        assert_eq!(u.steps(&mut clock, 0.1), 12);
        u.key("Escape", false, false, false, &tree);
        for _ in 0..100 {
            assert_eq!(u.steps(&mut clock, 10.), 0);
        }
        u.key("Escape", false, false, false, &tree);
        assert_eq!(u.steps(&mut clock, 0.1), 12);
        for hz in [30, 60, 144] {
            clock.remainder = 0.;
            u.time_scale = 8.;
            let count: usize = (0..hz).map(|_| u.steps(&mut clock, 1. / hz as f64)).sum();
            assert_eq!(count, 960);
        }
    }
    #[test]
    fn time_compression_keys_follow_the_manual() {
        // Manual p. 80: C cycles the rates, Shift-C is slow motion, C returns
        // from slow motion to normal speed.
        let tree = tree();
        let mut u = FlightUi::default();
        let mut rates = vec![];
        for _ in 0..5 {
            u.key("c", false, false, false, &tree);
            rates.push(u.time_scale);
        }
        assert_eq!(rates, [2., 4., 8., 1., 2.]);
        u.activate("Slow-motion", "Shift-C");
        assert_eq!(u.time_scale, 0.5);
        u.key("c", false, false, false, &tree);
        assert_eq!(u.time_scale, 1.);
    }
    #[test]
    fn nested_menu_keyboard_navigation_and_shortcuts() {
        let mut tree = tree();
        tree[0].children.push(MenuNode {
            label: "Time".into(),
            shortcut: String::new(),
            children: vec![MenuNode {
                label: "Paused".into(),
                shortcut: "Ctrl-P".into(),
                children: vec![],
            }],
        });
        let mut u = FlightUi::default();
        u.key("Escape", false, false, false, &tree);
        u.key("ArrowDown", false, false, false, &tree);
        u.key("Enter", false, false, false, &tree);
        assert_eq!(u.pause.path, vec![1]);
        u.key("Escape", false, false, false, &tree);
        assert!(u.menu && u.pause.path.is_empty());
        u.key("p", false, true, false, &tree);
        assert!(!u.frozen());
        assert_eq!(
            u.activate("Radar Cross Section", "Shift-0"),
            Command::Panel(0)
        );
        assert_eq!(u.activate("Back", "F2"), Command::View(3));
        assert_eq!(u.activate("External", "F10"), Command::View(1));
        assert_eq!(u.key("/", true, false, false, &tree), Command::CenterLook);
        assert_eq!(u.key("/", true, true, false, &tree), Command::None);
    }
    #[test]
    fn mouse_requires_same_press_release() {
        let mut u = FlightUi {
            menu: true,
            ..Default::default()
        };
        let t = tree();
        u.pointer(&t, Some((160., 55.)), true);
        assert_eq!(u.pointer(&t, Some((160., 100.)), false), Command::None);
        assert_eq!(u.pointer(&t, Some((160., 55.)), false), Command::None);
        u.pointer(&t, Some((160., 55.)), true);
        assert_eq!(u.pointer(&t, Some((160., 55.)), false), Command::End);
    }
    #[test]
    fn brackets_cycle_selection_and_retired_keys_are_inert() {
        let mut ui = FlightUi::default();
        let tree = tree();
        for (key, command) in [
            ("[", Command::PreviousWeapon),
            ("]", Command::NextWeapon),
            ("9", Command::None),
            ("0", Command::None),
        ] {
            assert_eq!(ui.key(key, false, false, false, &tree), command);
        }
    }
    #[test]
    fn retail_keys_reach_their_fa_commands() {
        use tore_sim::combat::live::Command as C;
        let tree = tree();
        let mut ui = FlightUi::default();
        for (key, shift, command) in [
            ("1", false, Command::ThrottlePreset(0., false)),
            ("2", false, Command::ThrottlePreset(0.25, false)),
            ("3", false, Command::ThrottlePreset(0.5, false)),
            ("4", false, Command::ThrottlePreset(0.75, false)),
            ("5", false, Command::ThrottlePreset(1., false)),
            ("6", false, Command::ThrottlePreset(1., true)),
            ("7", false, Command::ThrottleStep(-0.05)),
            ("8", false, Command::ThrottleStep(0.05)),
            ("Insert", false, Command::Chaff),
            ("Delete", false, Command::Flare),
            ("o", false, Command::Toggle(Switch::Bay)),
            ("k", true, Command::Combat(C::JettisonTanks)),
            (";", false, Command::Combat(C::ClearDesignation)),
            ("w", false, Command::Waypoint(true)),
            ("w", true, Command::Waypoint(false)),
            ("m", false, Command::Mode),
            // The manual (pp. 21, 94, 97): comma raises the range, period lowers it.
            (",", false, Command::Range(1)),
            (".", false, Command::Range(-1)),
            ("Numpad5", false, Command::CenterLook),
        ] {
            assert_eq!(ui.key(key, shift, false, false, &tree), command, "{key}");
        }
        // O no longer cycles the sensor channel; Shift-O no longer opens bays.
        assert_eq!(ui.key("o", true, false, false, &tree), Command::None);
        // The incoming fixture left Shift-I, the FA airbase inventory key.
        assert_eq!(ui.key("i", true, false, false, &tree), Command::Click);
        assert_eq!(
            ui.key("i", true, true, false, &tree),
            Command::Combat(C::Incoming)
        );
    }
    /// The live-fire range reset left Backslash (the gunsight's designate
    /// key, claimed by the stock key bindings before this table) for
    /// Ctrl+Shift+Backslash, John 2026-10-09.
    #[test]
    fn the_range_reset_is_ctrl_shift_backslash() {
        let tree = tree();
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("\\", true, true, false, &tree), Command::RangeReset);
        for (shift, ctrl, alt) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            assert_eq!(
                ui.key("\\", shift, ctrl, alt, &tree),
                Command::None,
                "shift {shift} ctrl {ctrl} alt {alt}"
            );
        }
    }
    #[test]
    fn manual_combat_commands_preserve_modifier_and_menu_isolation() {
        use tore_sim::combat::live::Command as C;
        let tree = tree();
        for (key, command) in [(";", C::ClearDesignation), ("l", C::ClearDesignation)] {
            let mut ui = FlightUi::default();
            assert_eq!(
                ui.key(key, false, false, false, &tree),
                Command::Combat(command)
            );
            assert!(!matches!(
                ui.key(key, true, false, false, &tree),
                Command::Combat(_)
            ));
            assert!(!matches!(
                ui.key(key, false, true, false, &tree),
                Command::Combat(_)
            ));
            assert!(!matches!(
                ui.key(key, false, false, true, &tree),
                Command::Combat(_)
            ));
            ui.menu = true;
            assert!(!matches!(
                ui.key(key, false, false, false, &tree),
                Command::Combat(_)
            ));
        }
    }
    #[test]
    fn player_wing_keys_follow_the_fa_layout() {
        use tore_sim::ai::wing::{PlayerApproach as A, PlayerBreak as B, PlayerOrder as O};
        let mut ui = FlightUi::default();
        for (key, order) in [
            ("1", O::Break(B::Straight)),
            ("2", O::Break(B::Left)),
            ("3", O::Break(B::Right)),
            ("4", O::Break(B::Low)),
            ("5", O::Break(B::High)),
            ("6", O::Approach(A::Left)),
            ("7", O::Approach(A::Right)),
            ("8", O::Approach(A::Low)),
            ("9", O::Approach(A::High)),
            ("e", O::EngageMyTarget),
            ("r", O::EngageFromFormation),
            ("a", O::Sort),
            ("w", O::EngageClass),
            ("f", O::EngageIrLaser),
            ("g", O::LaseMyTarget),
            ("p", O::ProtectMe),
            ("d", O::Disengage),
            ("b", O::BugOut),
            ("c", O::ControlToggle),
            ("h", O::Spacing),
            ("v", O::Stacking),
            ("l", O::LandAtSelected),
        ] {
            assert_eq!(
                ui.key(key, false, false, true, &[]),
                Command::Wing(order),
                "{key}"
            );
        }
        assert_eq!(
            ui.key("t", false, false, true, &[]),
            Command::WingFormationCycle
        );
        assert_eq!(ui.key("s", false, false, true, &[]), Command::RadioSilence);
    }
    #[test]
    fn the_bookmark_takes_ctrl_b_which_fa_leaves_free() {
        let mut ui = FlightUi::default();
        // key(name, shift, ctrl, alt, tree)
        assert_eq!(ui.key("b", false, true, false, &[]), Command::Bookmark);
        // B, Shift+B and Alt+B keep their own meanings.
        assert_eq!(
            ui.key("b", false, false, false, &[]),
            Command::Toggle(Switch::Airbrake)
        );
        assert_eq!(
            ui.key("b", true, false, false, &[]),
            Command::Toggle(Switch::Burner)
        );
        assert_eq!(
            ui.key("b", false, false, true, &[]),
            Command::Wing(tore_sim::ai::wing::PlayerOrder::BugOut)
        );
        assert_ne!(ui.key("b", true, true, false, &[]), Command::Bookmark);
        // No FA Ctrl command lands on it (docs/spec/keyboard.md).
        for key in ["a", "z", "x", "r", "p", "q", "v", "t", "f"] {
            assert_ne!(
                ui.key(key, false, true, false, &[]),
                Command::Bookmark,
                "{key}"
            );
        }
    }
    #[test]
    fn wing_addressing_uses_alt_zero_and_alt_shift_digits() {
        let mut ui = FlightUi::default();
        assert_eq!(
            ui.key("0", false, false, true, &[]),
            Command::WingRecipient(None)
        );
        for n in 1..=4u8 {
            assert_eq!(
                ui.key(&n.to_string(), true, false, true, &[]),
                Command::WingRecipient(Some(n))
            );
        }
        assert!(!matches!(
            ui.key("5", true, false, true, &[]),
            Command::WingRecipient(_)
        ));
        // Alt+U retired with bug out's move to Alt+B; unmodified and Shift
        // keys keep their own meanings.
        assert_eq!(ui.key("u", false, false, true, &[]), Command::None);
        assert!(!matches!(
            ui.key("b", false, false, false, &[]),
            Command::Wing(_)
        ));
        assert!(!matches!(
            ui.key("l", true, false, false, &[]),
            Command::Wing(_)
        ));
    }
    #[test]
    fn iff_and_the_score_board_take_u_and_k_and_keep_their_shifted_keys() {
        let mut ui = FlightUi::default();
        assert_eq!(ui.key("u", false, false, false, &[]), Command::Iff);
        assert_eq!(ui.key("k", false, false, false, &[]), Command::ScoreBoard);
        // Shift+U is the HUD, Shift+K jettisons, and no Ctrl or Alt chord
        // squawks or opens the board.
        assert_eq!(ui.key("u", true, false, false, &[]), Command::Click);
        assert!(!ui.hud);
        assert_eq!(
            ui.key("k", true, false, false, &[]),
            Command::Combat(tore_sim::combat::live::Command::JettisonTanks)
        );
        for (shift, ctrl, alt) in [(false, true, false), (false, false, true)] {
            for key in ["u", "k"] {
                assert_eq!(ui.key(key, shift, ctrl, alt, &[]), Command::None, "{key}");
            }
        }
        // A menu or a pause up: the keys do nothing to the flight.
        ui.menu = true;
        assert_eq!(ui.key("u", false, false, false, &tree()), Command::None);
    }
    #[test]
    fn the_reply_keys_are_alt_shift_letters_and_leave_the_lead_orders_alone() {
        use tore_sim::ai::wing::PlayerOrder as O;
        let mut ui = FlightUi::default();
        for (key, reply) in [
            ("e", Reply::Engaging),
            ("w", Reply::Winchester),
            ("b", Reply::BingoFuel),
            ("h", Reply::NeedHelp),
        ] {
            assert_eq!(
                ui.key(key, true, false, true, &[]),
                Command::Reply(reply),
                "{key}"
            );
        }
        // The Alt letters are still the lead's orders, Shift+E still ejects,
        // and the addressing keys keep Alt+Shift with a digit.
        assert_eq!(
            ui.key("e", false, false, true, &[]),
            Command::Wing(O::EngageMyTarget)
        );
        assert_eq!(
            ui.key("w", false, false, true, &[]),
            Command::Wing(O::EngageClass)
        );
        assert_eq!(ui.key("e", true, false, false, &[]), Command::Eject);
        assert_eq!(
            ui.key("1", true, false, true, &[]),
            Command::WingRecipient(Some(1))
        );
        // Alt+A is the data link's sort (slice G3c) and Alt+N monitors the
        // battle net (slice G8); Alt+Shift+A and Alt+Shift+N stay free.
        assert_eq!(ui.key("a", false, false, true, &[]), Command::Wing(O::Sort));
        assert_eq!(ui.key("a", true, false, true, &[]), Command::None);
        assert_eq!(ui.key("n", true, false, true, &[]), Command::None);
        assert_eq!(ui.key("n", false, false, true, &[]), Command::BattleNet);
        assert_eq!(Reply::NeedHelp.world().text(), "Need help");
        assert_eq!(Reply::of_key("x"), None);
        // Each key's reply is the mission core's, in wire order.
        for (key, reply) in [
            ("e", tore_world::world::replies::Reply::Engaging),
            ("w", tore_world::world::replies::Reply::Winchester),
            ("b", tore_world::world::replies::Reply::BingoFuel),
            ("h", tore_world::world::replies::Reply::NeedHelp),
        ] {
            assert_eq!(Reply::of_key(key).unwrap().world(), reply);
        }
    }
    #[test]
    fn single_player_replies_say_the_plane_leads_and_the_board_says_network_only() {
        assert_eq!(SINGLE_PLAYER_REPLY, "You lead this flight.");
        assert_eq!(score_board_answer(false), "Score board: network games only");
        assert_eq!(score_board_answer(true), "Score board: not available yet");
    }
    #[test]
    fn show_target_info_is_off_by_default_and_toggles_from_ctrl_t_and_the_pref_row() {
        let mut ui = FlightUi::default();
        assert!(!ui.target_info);
        assert_eq!(ui.cheat_state("Show target info?"), Some("Off"));
        // Ctrl+T, the retail accelerator, works with or without the menu.
        assert_eq!(ui.key("t", false, true, false, &[]), Command::Click);
        assert!(ui.target_info);
        assert_eq!(ui.cheat_state("Show target info?"), Some("On"));
        assert_eq!(shown(&ui).last().copied(), Some("Show target info: on"));
        // The Pref row toggles it back, and says it did not need research.
        assert_eq!(ui.activate("Show target info?", "Ctrl-T"), Command::Click);
        assert!(!ui.target_info);
        assert!(
            shown(&ui)
                .iter()
                .all(|line| !line.contains("not implemented"))
        );
        // Shift+Ctrl+T and Alt+Ctrl+T are not it; T alone still designates.
        assert!(!matches!(
            ui.key("t", true, true, false, &[]),
            Command::Click
        ));
        assert_eq!(ui.key("t", false, false, false, &[]), Command::Target);
    }
    #[test]
    fn a_session_keeps_the_target_info_row_and_the_keys() {
        let mut ui = FlightUi::default();
        ui.enter_session();
        assert_eq!(session_refusal("Show target info?"), None);
        assert_eq!(ui.key("t", false, true, false, &[]), Command::Click);
        assert!(ui.target_info);
        assert_eq!(ui.key("u", false, false, false, &[]), Command::Iff);
    }
    #[test]
    fn the_debug_tab_edits_the_ai_test_settings_and_a_joined_game_locks_it() {
        use tore_sim::ai::test_settings::AiTestSettings;
        let mut t = tree();
        add_debug_tab(&mut t);
        add_debug_tab(&mut t);
        assert_eq!(t.len(), 2, "added once, after the other tabs");
        assert_eq!(t[1].label, DEBUG_TAB);
        let rows: Vec<_> = t[1].children.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(rows, DEBUG_ROWS);

        // The defaults are 60 Hz with lean sensors and read so.
        let mut ui = FlightUi::default();
        assert!(ui.ai_test.is_default());
        assert_eq!(ui.cheat_state(AI_OBSERVATION_RATE), Some("60 Hz"));
        assert_eq!(ui.cheat_state(LEAN_AI_SENSORS), Some("On"));

        // Escape, Right to the tab, Enter on the rate row: the next rate.
        ui.key("Escape", false, false, false, &t);
        ui.key("ArrowRight", false, false, false, &t);
        assert_eq!(ui.key("Enter", false, false, false, &t), Command::Click);
        assert_eq!(ui.ai_test.observation_hz(), 40);
        assert_eq!(ui.cheat_state(AI_OBSERVATION_RATE), Some("40 Hz"));
        for hz in [30, 20, 120, 60] {
            ui.activate(AI_OBSERVATION_RATE, "");
            assert_eq!(ui.ai_test.observation_hz(), hz);
            assert_eq!(
                ui.cheat_state(AI_OBSERVATION_RATE),
                Some(observation_rate_name(hz))
            );
        }
        ui.activate(LEAN_AI_SENSORS, "");
        assert!(!ui.ai_test.lean_sensors);
        assert_eq!(ui.cheat_state(LEAN_AI_SENSORS), Some("Off"));
        // A restart keeps the test settings; Back to defaults clears them.
        ui.activate(AI_OBSERVATION_RATE, "");
        ui.reset_for_flight();
        assert_eq!(ui.ai_test.observation_hz(), 40);
        assert!(!ui.ai_test.lean_sensors);
        ui.activate(DEBUG_DEFAULTS, "");
        assert_eq!(ui.ai_test, AiTestSettings::default());
        assert_eq!(ui.cheat_state(LEAN_AI_SENSORS), Some("On"));

        // A joined game has the host's settings: the rows are dimmed, say so
        // and change nothing.
        ui.lock_ai_test(true);
        assert!(
            ui.pause
                .dim
                .iter()
                .any(|label| label == AI_OBSERVATION_RATE)
        );
        assert_eq!(ui.cheat_state(AI_OBSERVATION_RATE), Some("Host"));
        ui.activate(AI_OBSERVATION_RATE, "");
        ui.activate(LEAN_AI_SENSORS, "");
        assert_eq!(ui.ai_test, AiTestSettings::default());
        assert_eq!(shown(&ui).last().copied(), Some(AI_TEST_LOCKED));
        ui.lock_ai_test(false);
        assert!(ui.pause.dim.is_empty());
        ui.activate(LEAN_AI_SENSORS, "");
        assert!(!ui.ai_test.lean_sensors);
    }
    #[test]
    fn the_debug_rows_survive_the_session_menu_filter() {
        let mut t = tree();
        add_debug_tab(&mut t);
        let menu = session_menu(&t);
        assert!(menu.iter().any(|node| node.label == DEBUG_TAB));
        assert_eq!(session_refusal(AI_OBSERVATION_RATE), None);
        assert_eq!(session_refusal(LEAN_AI_SENSORS), None);
        assert_eq!(session_refusal(FRAME_LOG), None);
        let mut ui = FlightUi::default();
        ui.lock_ai_test(true);
        assert!(!ui.pause.dim.iter().any(|label| label == FRAME_LOG));
        assert!(turns_back(FRAME_LOG));
    }
}
